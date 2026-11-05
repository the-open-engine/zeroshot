use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;
use crate::TargetDiscoveryExtensions;

#[test]
fn stack_run_names_use_the_compact_ascii_identifier_contract() {
    for valid in ["backend", "frontend_2", "integrate.final"] {
        assert!(StackRunName::new(valid).is_ok());
    }
    for invalid in ["", "-leading", "has space", "has/slash", &"x".repeat(65)] {
        assert!(StackRunName::new(invalid).is_err());
    }
}

fn submit_request() -> serde_json::Value {
    json!({
        "submissionKey": "stack-1",
        "title": "Checkout update",
        "expiresAt": "2026-09-11T00:00:00Z",
        "delivery": "stacked_pull_requests",
        "source": {"repository": "owner/repo", "branch": "main"},
        "profile": {"scope": "org", "name": "software-change"},
        "runs": [
            {"name": "api", "initialInput": {"task": "Update the API."}},
            {"name": "client", "needs": ["api"], "initialInput": {"task": "Update the client."}}
        ]
    })
}

#[test]
fn stack_submit_request_carries_delivery_and_round_trips() {
    let request = serde_json::from_value::<StackSubmitRequest>(submit_request()).assert_value();
    assert_eq!(request.delivery, StackDelivery::StackedPullRequests);
    assert_eq!(request.runs[1].needs[0].as_str(), "api");
    assert_eq!(
        serde_json::to_value(&request).assert_value(),
        submit_request()
    );

    let mut merge = submit_request();
    merge["delivery"] = json!("merge");
    let request = serde_json::from_value::<StackSubmitRequest>(merge).assert_value();
    assert_eq!(request.delivery, StackDelivery::Merge);
    for delivery in [StackDelivery::Merge, StackDelivery::StackedPullRequests] {
        assert_eq!(
            serde_json::to_value(delivery).assert_value(),
            json!(delivery.as_str())
        );
    }
}

#[test]
fn stack_submit_request_rejects_missing_unknown_and_unrecognized_fields() {
    let mut missing = submit_request();
    missing.as_object_mut().assert_value().remove("delivery");
    assert!(serde_json::from_value::<StackSubmitRequest>(missing).is_err());

    let mut misspelled = submit_request();
    misspelled["delivery"] = json!("stacked_pull_request");
    assert!(serde_json::from_value::<StackSubmitRequest>(misspelled).is_err());

    let mut unknown = submit_request();
    unknown["plan"] = json!(true);
    assert!(serde_json::from_value::<StackSubmitRequest>(unknown).is_err());

    let mut unknown_run = submit_request();
    unknown_run["runs"][0]["outputFrom"] = json!("client");
    assert!(serde_json::from_value::<StackSubmitRequest>(unknown_run).is_err());
}

fn run_status() -> serde_json::Value {
    json!({
        "name": "client",
        "runId": "run-2",
        "state": "running",
        "needs": ["api"],
        "sourceBranch": "zeroshot/v2-0123456789abcdef0123",
        "sourceRevision": "0123456789abcdef0123456789abcdef01234567",
        "readyAt": "2026-09-10T01:00:00Z",
        "queueExpiresAt": "2026-09-11T00:00:00Z",
        "terminalAt": null,
        "waitingReason": null,
        "errorCode": null
    })
}

#[test]
fn stack_run_status_reports_source_branch_next_to_source_revision() {
    let status = serde_json::from_value::<StackRunStatus>(run_status()).assert_value();
    assert_eq!(
        status.source_branch.as_ref().map(SourceBranchId::as_str),
        Some("zeroshot/v2-0123456789abcdef0123")
    );
    assert_eq!(serde_json::to_value(status).assert_value(), run_status());

    let mut unknown = run_status();
    unknown["headBranch"] = json!("zeroshot/v2-other");
    assert!(serde_json::from_value::<StackRunStatus>(unknown).is_err());
}

#[test]
fn stack_status_requires_explicit_nullable_lifecycle_fields() {
    for field in [
        "sourceBranch",
        "sourceRevision",
        "readyAt",
        "queueExpiresAt",
        "terminalAt",
        "waitingReason",
        "errorCode",
    ] {
        let mut missing = run_status();
        missing.as_object_mut().assert_value().remove(field);
        assert!(
            serde_json::from_value::<StackRunStatus>(missing).is_err(),
            "{field} must be explicit"
        );
    }

    let blocked = json!({
        "name": "api",
        "runId": "run-1",
        "state": "blocked",
        "needs": [],
        "sourceBranch": null,
        "sourceRevision": null,
        "readyAt": null,
        "queueExpiresAt": null,
        "terminalAt": null,
        "waitingReason": null,
        "errorCode": null
    });
    let stack = json!({
        "stackId": "stack-1",
        "title": "Checkout update",
        "state": "queued",
        "repository": "owner/repo",
        "branch": "main",
        "submittedAt": "2026-09-10T00:00:00Z",
        "expiresAt": "2026-09-11T00:00:00Z",
        "runs": [blocked]
    });
    let status = serde_json::from_value::<Stack>(stack.clone()).assert_value();
    assert_eq!(status.stack_id.as_str(), "stack-1");
    assert_eq!(status.runs[0].source_branch, None);
    assert_eq!(serde_json::to_value(status).assert_value(), stack);

    let mut renamed = stack;
    let id = renamed
        .as_object_mut()
        .assert_value()
        .remove("stackId")
        .assert_value();
    renamed["planId"] = id;
    assert!(serde_json::from_value::<Stack>(renamed).is_err());
}

#[test]
fn stack_state_helpers_cover_every_terminal_contract() {
    for (state, terminal, succeeded) in [
        (StackState::Queued, false, false),
        (StackState::Running, false, false),
        (StackState::Succeeded, true, true),
        (StackState::Failed, true, false),
        (StackState::Cancelled, true, false),
        (StackState::Expired, true, false),
    ] {
        assert_eq!(state.is_terminal(), terminal, "state {state:?}");
        assert_eq!(state.succeeded(), succeeded, "state {state:?}");
    }

    for (state, terminal) in [
        (StackRunState::Blocked, false),
        (StackRunState::Materializing, false),
        (StackRunState::Queued, false),
        (StackRunState::Provisioning, false),
        (StackRunState::Running, false),
        (StackRunState::Cancelling, false),
        (StackRunState::Succeeded, true),
        (StackRunState::Failed, true),
        (StackRunState::Cancelled, true),
        (StackRunState::Expired, true),
    ] {
        assert_eq!(state.is_terminal(), terminal, "run state {state:?}");
    }
}

#[test]
fn stacks_discovery_uses_its_own_strict_extension() {
    let discovery = json!({
        "kind": STACKS_KIND,
        "baseUrl": "https://target.example/api/",
        "routeTemplates": {
            "create": "/stacks",
            "status": "/stacks/{stack_id}",
            "force": "/stacks/{stack_id}/force"
        }
    });
    let extensions = serde_json::from_value::<TargetDiscoveryExtensions>(json!({
        "stacks": discovery.clone()
    }))
    .assert_value();
    assert!(!extensions.is_empty());
    let stacks = extensions.stacks.as_ref().assert_value();
    assert_eq!(stacks.kind, "zeroshot.stacks/v1");
    assert_eq!(stacks.route_templates.status, "/stacks/{stack_id}");
    assert_eq!(
        serde_json::to_value(&extensions).assert_value(),
        json!({"stacks": discovery})
    );

    let mut extra_route = discovery.clone();
    extra_route["routeTemplates"]["list"] = json!("/stacks");
    assert!(
        serde_json::from_value::<TargetDiscoveryExtensions>(json!({"stacks": extra_route}))
            .is_err()
    );

    let retired = serde_json::from_value::<TargetDiscoveryExtensions>(json!({
        "merge_plans": discovery
    }))
    .assert_value();
    assert!(retired.stacks.is_none());
    assert!(retired.is_empty());
}
