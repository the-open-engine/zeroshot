use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;

#[test]
fn plan_run_names_use_the_compact_ascii_identifier_contract() {
    for valid in ["backend", "frontend_2", "integrate.final"] {
        assert!(MergePlanRunName::new(valid).is_ok());
    }
    for invalid in ["", "-leading", "has space", "has/slash", &"x".repeat(65)] {
        assert!(MergePlanRunName::new(invalid).is_err());
    }
}

#[test]
fn plan_status_requires_explicit_nullable_lifecycle_fields() {
    let missing = serde_json::from_value::<MergePlanRunStatus>(json!({
        "name": "backend",
        "runId": "run-1",
        "state": "blocked",
        "needs": []
    }));
    assert!(missing.is_err());

    let status = serde_json::from_value::<MergePlanRunStatus>(json!({
        "name": "backend",
        "runId": "run-1",
        "state": "blocked",
        "needs": [],
        "sourceRevision": null,
        "readyAt": null,
        "queueExpiresAt": null,
        "terminalAt": null,
        "waitingReason": null,
        "errorCode": null
    }))
    .assert_value();
    assert_eq!(status.name.as_str(), "backend");
    assert_eq!(
        serde_json::to_value(status).assert_value(),
        json!({
            "name": "backend",
            "runId": "run-1",
            "state": "blocked",
            "needs": [],
            "sourceRevision": null,
            "readyAt": null,
            "queueExpiresAt": null,
            "terminalAt": null,
            "waitingReason": null,
            "errorCode": null
        })
    );
}

#[test]
fn plan_state_helpers_cover_every_terminal_contract() {
    for (state, terminal, succeeded) in [
        (MergePlanState::Queued, false, false),
        (MergePlanState::Running, false, false),
        (MergePlanState::Succeeded, true, true),
        (MergePlanState::Failed, true, false),
        (MergePlanState::Cancelled, true, false),
        (MergePlanState::Expired, true, false),
    ] {
        assert_eq!(state.is_terminal(), terminal, "state {state:?}");
        assert_eq!(state.succeeded(), succeeded, "state {state:?}");
    }

    for (state, terminal) in [
        (MergePlanRunState::Blocked, false),
        (MergePlanRunState::Materializing, false),
        (MergePlanRunState::Queued, false),
        (MergePlanRunState::Provisioning, false),
        (MergePlanRunState::Running, false),
        (MergePlanRunState::Cancelling, false),
        (MergePlanRunState::Succeeded, true),
        (MergePlanRunState::Failed, true),
        (MergePlanRunState::Cancelled, true),
        (MergePlanRunState::Expired, true),
    ] {
        assert_eq!(state.is_terminal(), terminal, "run state {state:?}");
    }
}
