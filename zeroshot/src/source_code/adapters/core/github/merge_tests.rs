use crate::source_code_provider::TestReviewCheckpoint;
use crate::source_code_provider::adapters::core::http_fixture::{
    self, IntoResponse, Json, Request as FixtureRequest, Response as FixtureResponse,
};
use reqwest::StatusCode;
use std::io::Write;
use super::*;
use super::super::http::Http;

fn base_ref(page: &mut Value) -> &mut Value {
    page.pointer_mut("/data/repository/pullRequest/baseRef")
        .assert_value()
}

fn set_rules(page: &mut Value, rules: Vec<Value>) {
    let rules = rules
        .into_iter()
        .enumerate()
        .map(|(index, mut rule)| {
            rule["id"] = json!(format!("RULE_{index}"));
            rule
        })
        .collect::<Vec<_>>();
    base_ref(page)["rules"] = json!({
        "totalCount": rules.len(),
        "nodes": rules,
        "pageInfo": {"hasNextPage": false, "endCursor": null},
    });
}

fn method_rule(methods: Value) -> Value {
    json!({"type": "PULL_REQUEST", "parameters": {"allowedMergeMethods": methods}})
}

fn assert_policy_error(error: &SourceDeliveryError) {
    assert!(matches!(error, SourceDeliveryError::Api(_)), "{error}");
    assert!(!error.retryable_operation(), "{error}");
    assert!(!error.authentication_failed(), "{error}");
}

fn merge_authority(
    root: &std::path::Path,
    before: &Value,
    after: &Value,
    response: impl Fn() -> FixtureResponse + Send + Sync + 'static,
) -> GhCliDeliveryAuthority {
    let before = before.clone();
    let after = after.clone();
    let recorded = root.join("merge-request.json");
    http_authority(root, move |request| {
        if request.method == Method::GET {
            assert_eq!(
                request.uri.path(),
                "/repos/acme/project/rules/branches/main"
            );
            return Json(json!([])).into_response();
        }
        let body = graph_body(request);
        if body["query"]
            .as_str()
            .assert_value()
            .contains("mergePullRequest")
        {
            let mut requests = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&recorded)
                .assert_value();
            writeln!(requests, "{body}").assert_value();
            return response();
        }
        Json(if recorded.exists() { &after } else { &before }).into_response()
    })
}

pub(super) fn http_authority(
    root: &std::path::Path,
    handler: impl Fn(FixtureRequest) -> FixtureResponse + Send + Sync + 'static,
) -> GhCliDeliveryAuthority {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").assert_value();
    let base = format!("http://{}/", listener.local_addr().assert_value());
    listener.set_nonblocking(true).assert_value();
    let listener = tokio::net::TcpListener::from_std(listener).assert_value();
    tokio::spawn(http_fixture::serve(listener, handler));
    let mut config = GhCliAuthorityConfig::hosted(root.to_owned());
    config.api_deadline = Duration::from_secs(5);
    let result = GhCliDeliveryAuthority::new(config);
    result
        .http
        .set(Http::loopback(&base, "2026-03-10").assert_value())
        .assert_value();
    result
}

fn graph_body(request: FixtureRequest) -> Value {
    assert_eq!(request.method, Method::POST);
    assert_eq!(request.uri.path(), "/graphql");
    assert_eq!(request.headers["authorization"], "Bearer test-token");
    serde_json::from_slice(&request.body).assert_value()
}

fn merge_receipt() -> FixtureResponse {
    Json(super::queue_tests::direct_merge_response(
        "cccccccccccccccccccccccccccccccccccccccc",
    ))
    .into_response()
}

fn merge_rejection(message: &str) -> FixtureResponse {
    Json(json!({
        "data":{"mergePullRequest":null},
        "errors":[{"type":"UNPROCESSABLE","path":["mergePullRequest"],"message":message}],
    }))
    .into_response()
}

#[cfg(unix)]
#[tokio::test]
async fn merge_requests_use_the_selected_method_and_exact_head_without_bypass() {
    for method in ["MERGE", "SQUASH", "REBASE"] {
        let root = tempfile::tempdir().assert_value();
        let mut page = policy_page_with_merge_capabilities(true, true, true);
        set_rules(&mut page, vec![method_rule(json!([method]))]);
        let outcome = merge_authority(root.path(), &page, &page, merge_receipt)
            .request_merge(
                &review(),
                SourceDeliveryCredential::token("test-token"),
                &TestReviewCheckpoint,
            )
            .await
            .assert_value();
        assert_eq!(outcome, SourceMergeRequestOutcome::Accepted);
        let sent: Value = serde_json::from_slice(
            &std::fs::read(root.path().join("merge-request.json")).assert_value(),
        )
        .assert_value();
        assert_eq!(
            sent["variables"],
            json!({
                "pullRequestId":"PR_node_17",
                "expectedHeadOid":review().head_revision,
                "method":method,
            })
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn unavailable_method_or_changed_head_never_invokes_merge() {
    let unavailable = policy_page_with_merge_capabilities(false, false, false);
    let mut changed = policy_page_with_merge_capabilities(true, true, true);
    changed["data"]["repository"]["pullRequest"]["headRefOid"] =
        json!("cccccccccccccccccccccccccccccccccccccccc");
    for page in [unavailable, changed] {
        let root = tempfile::tempdir().assert_value();
        merge_authority(root.path(), &page, &page, merge_receipt)
            .request_merge(
                &review(),
                SourceDeliveryCredential::token("test-token"),
                &TestReviewCheckpoint,
            )
            .await
            .assert_error();
        assert!(!root.path().join("merge-request.json").exists());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn merge_submission_rechecks_terminal_approval_queue_and_freshness_state() {
    let mut queued = policy_page_with_merge_capabilities(false, false, false);
    queued["data"]["repository"]["pullRequest"]["isMergeQueueEnabled"] = json!(true);
    queued["data"]["repository"]["pullRequest"]["isInMergeQueue"] = json!(true);
    let mut merged = queued.clone();
    set_review_state(&mut merged, "MERGED", true);
    merged["data"]["repository"]["pullRequest"]["mergeCommit"] =
        json!({"oid": "cccccccccccccccccccccccccccccccccccccccc"});
    base_ref(&mut merged)["rules"] = Value::Null;
    let mut review_required = policy_page_with_merge_capabilities(false, false, false);
    review_required["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("BLOCKED");
    review_required["data"]["repository"]["pullRequest"]["reviewDecision"] =
        json!("REVIEW_REQUIRED");
    base_ref(&mut review_required)["refUpdateRule"]["requiredApprovingReviewCount"] = json!(1);
    base_ref(&mut review_required)["rules"] = Value::Null;
    let mut behind = policy_page("MERGEABLE", "BEHIND", None, (false, None));
    base_ref(&mut behind)["rules"] = Value::Null;
    for (page, expected) in [
        (queued, SourceMergeRequestOutcome::Pending),
        (merged, SourceMergeRequestOutcome::Accepted),
        (review_required, SourceMergeRequestOutcome::Pending),
        (behind, SourceMergeRequestOutcome::HeadUpdateRequired),
    ] {
        let root = tempfile::tempdir().assert_value();
        let actual = merge_authority(root.path(), &page, &page, || panic!("unexpected mutation"))
            .request_merge(
                &review(),
                SourceDeliveryCredential::token("test-token"),
                &TestReviewCheckpoint,
            )
            .await
            .assert_value();
        assert_eq!(actual, expected);
        assert!(!root.path().join("merge-request.json").exists());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn queue_removal_between_inspection_and_submission_requires_fresh_reconciliation() {
    let root = tempfile::tempdir().assert_value();
    let mut page = policy_page("MERGEABLE", "CLEAN", None, (false, None));
    base_ref(&mut page)["compare"]["behindBy"] = json!(2);
    page["data"]["repository"]["pullRequest"]["isMergeQueueEnabled"] = json!(true);
    let current_policy = root.path().join("policy.json");
    std::fs::write(&current_policy, page.to_string()).assert_value();
    let unexpected = root.path().join("unexpected-write");
    let authority = http_authority(root.path(), {
        let current_policy = current_policy.clone();
        let unexpected = unexpected.clone();
        move |request| {
            if request.method == Method::GET {
                return Json(json!([])).into_response();
            }
            let body = graph_body(request);
            if body["query"]
                .as_str()
                .assert_value()
                .trim_start()
                .starts_with("mutation")
            {
                std::fs::write(&unexpected, body.to_string()).assert_value();
                return StatusCode::BAD_REQUEST.into_response();
            }
            FixtureResponse::raw(std::fs::read(&current_policy).assert_value())
        }
    });
    let credential = SourceDeliveryCredential::token("test-token");
    let inspected = authority
        .inspect_review(&review(), credential)
        .await
        .assert_value();
    assert!(!inspected.head_update_required);

    page["data"]["repository"]["pullRequest"]["isMergeQueueEnabled"] = json!(false);
    std::fs::write(&current_policy, page.to_string()).assert_value();
    // Both the existing authority and a recovered one must use current ancestry/policy.
    let recovered = GhCliDeliveryAuthority::new(authority.config.clone());
    recovered
        .http
        .set(authority.http.get().assert_value().clone())
        .assert_value();
    for authority in [authority, recovered] {
        for _ in 0..2 {
            assert_eq!(
                authority
                    .request_merge(&review(), credential, &TestReviewCheckpoint)
                    .await
                    .assert_value(),
                SourceMergeRequestOutcome::HeadUpdateRequired
            );
        }
    }
    assert!(!unexpected.exists());
}

#[tokio::test]
async fn current_queue_setting_selects_one_exact_head_mutation() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    for initial_queue in [false, true] {
        let root = tempfile::tempdir().assert_value();
        let queue = Arc::new(AtomicBool::new(initial_queue));
        let mutations = Arc::new(AtomicUsize::new(0));
        let authority = http_authority(root.path(), {
            let queue = queue.clone();
            let mutations = mutations.clone();
            move |request| {
                if request.method == Method::GET {
                    return Json(json!([])).into_response();
                }
                let body = graph_body(request);
                let query = body["query"].as_str().assert_value();
                let queue_enabled = queue.load(Ordering::Relaxed);
                if query.trim_start().starts_with("mutation") {
                    mutations.fetch_add(1, Ordering::Relaxed);
                    assert_eq!(query.contains("enqueuePullRequest"), queue_enabled);
                    assert_eq!(query.contains("mergePullRequest"), !queue_enabled);
                    assert_eq!(body["variables"]["expectedHeadOid"], review().head_revision);
                    return if queue_enabled {
                        Json(super::queue_tests::enqueue_response()).into_response()
                    } else {
                        merge_receipt()
                    };
                }
                let mut page = policy_page_with_merge_capabilities(true, true, true);
                page["data"]["repository"]["pullRequest"]["isMergeQueueEnabled"] =
                    json!(queue_enabled);
                Json(page).into_response()
            }
        });
        let credential = SourceDeliveryCredential::token("test-token");
        authority
            .inspect_review(&review(), credential)
            .await
            .assert_value();
        queue.store(!initial_queue, Ordering::Relaxed);
        assert_eq!(
            authority
                .request_merge(&review(), credential, &TestReviewCheckpoint)
                .await
                .assert_value(),
            SourceMergeRequestOutcome::Accepted,
        );
        assert_eq!(mutations.load(Ordering::Relaxed), 1);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn permanent_merge_rejection_preserves_diagnostics_and_cannot_become_pending_or_git_repair() {
    let ready = policy_page_with_merge_capabilities(true, true, true);
    for after in [
        ready.clone(),
        policy_page("UNKNOWN", "BLOCKED", None, (false, None)),
        policy_page("CONFLICTING", "DIRTY", None, (false, None)),
        policy_page("MERGEABLE", "BEHIND", None, (false, None)),
    ] {
        for (status, rejection) in [
            (None, "Merge commits are not allowed on this repository."),
            (Some(StatusCode::FORBIDDEN), "Resource not accessible"),
            (Some(StatusCode::UNPROCESSABLE_ENTITY), "Validation Failed"),
        ] {
            let root = tempfile::tempdir().assert_value();
            let error = merge_authority(root.path(), &ready, &after, move || {
                let diagnostic = format!("{rejection} test-token");
                match status {
                    Some(status) => (status, Json(json!({"message":diagnostic}))).into_response(),
                    None => merge_rejection(&diagnostic),
                }
            })
            .request_merge(
                &review(),
                SourceDeliveryCredential::token("test-token"),
                &TestReviewCheckpoint,
            )
            .await
            .assert_error();
            assert_policy_error(&error);
            assert!(error.to_string().contains(rejection));
            assert!(error.to_string().contains("acme/project#17 into main"));
            assert!(!error.to_string().contains("test-token"));
            let requests =
                std::fs::read_to_string(root.path().join("merge-request.json")).assert_value();
            assert_eq!(requests.lines().count(), 1);
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn merge_transport_errors_preserve_authentication_and_retry_classification() {
    let page = policy_page_with_merge_capabilities(true, true, true);
    for (status, reason) in [
        (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable"),
        (StatusCode::TOO_MANY_REQUESTS, "Too Many Requests"),
        (StatusCode::UNAUTHORIZED, "Unauthorized"),
    ] {
        for json_body in [false, true] {
            let root = tempfile::tempdir().assert_value();
            let error = merge_authority(root.path(), &page, &page, move || {
                if json_body {
                    (status, Json(json!({"message":reason}))).into_response()
                } else {
                    status.into_response()
                }
            })
            .request_merge(
                &review(),
                SourceDeliveryCredential::token("test-token"),
                &TestReviewCheckpoint,
            )
            .await
            .assert_error();
            assert_eq!(error.api_status(), Some(status.as_u16()), "{reason}");
            assert_eq!(
                error.authentication_failed(),
                status == StatusCode::UNAUTHORIZED,
                "{reason}"
            );
            assert_eq!(
                error.retryable_operation(),
                status != StatusCode::UNAUTHORIZED,
                "{reason}"
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn permanent_http_rejection_cannot_request_workspace_repair_or_head_update() {
    let ready = policy_page_with_merge_capabilities(true, true, true);
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::UNPROCESSABLE_ENTITY,
    ] {
        for after in [
            policy_page("CONFLICTING", "DIRTY", None, (false, None)),
            policy_page("MERGEABLE", "BEHIND", None, (false, None)),
        ] {
            let root = tempfile::tempdir().assert_value();
            let error =
                merge_authority(root.path(), &ready, &after, move || status.into_response())
                    .request_merge(
                        &review(),
                        SourceDeliveryCredential::token("test-token"),
                        &TestReviewCheckpoint,
                    )
                    .await
                    .assert_error();
            assert!(matches!(error, SourceDeliveryError::Api(_)));
            assert_eq!(error.api_status(), Some(status.as_u16()));
            assert!(!error.retryable_operation());
            assert_eq!(
                error.authentication_failed(),
                status == StatusCode::UNAUTHORIZED
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn merge_failure_reconciles_authoritative_success_conflict_and_freshness() {
    let ready = policy_page_with_merge_capabilities(true, true, true);
    let mut merged = ready.clone();
    set_review_state(&mut merged, "MERGED", true);
    merged["data"]["repository"]["pullRequest"]["mergeCommit"] =
        json!({"oid": "cccccccccccccccccccccccccccccccccccccccc"});
    let transient: fn() -> FixtureResponse = || StatusCode::SERVICE_UNAVAILABLE.into_response();
    let rejected: fn() -> FixtureResponse = || merge_rejection("already merged");
    let cases = [
        (
            merged.clone(),
            SourceMergeRequestOutcome::Accepted,
            rejected,
        ),
        (merged, SourceMergeRequestOutcome::Accepted, transient),
        (
            policy_page("CONFLICTING", "DIRTY", None, (false, None)),
            SourceMergeRequestOutcome::Conflict,
            transient,
        ),
        (
            policy_page("MERGEABLE", "BEHIND", None, (false, None)),
            SourceMergeRequestOutcome::HeadUpdateRequired,
            transient,
        ),
    ];
    for (after, expected, action) in cases {
        let root = tempfile::tempdir().assert_value();
        let authority = merge_authority(root.path(), &ready, &after, action);
        let credential = SourceDeliveryCredential::token("test-token");
        let outcome = authority
            .request_merge(&review(), credential, &TestReviewCheckpoint)
            .await
            .unwrap_or_else(|error| panic!("expected {expected:?}, got {error}"));
        assert_eq!(outcome, expected);
        if outcome == SourceMergeRequestOutcome::HeadUpdateRequired {
            assert_eq!(
                authority
                    .update_review_head(root.path(), &review(), credential)
                    .await
                    .assert_value(),
                SourceHeadUpdateOutcome::Pending,
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn review_observation_and_queue_submission_do_not_read_direct_merge_policy() {
    for queued in [false, true] {
        let root = tempfile::tempdir().assert_value();
        let mut page = policy_page_with_merge_capabilities(false, false, false);
        base_ref(&mut page)["rules"] = Value::Null;
        base_ref(&mut page)
            .as_object_mut()
            .assert_value()
            .remove("branchProtectionRule");
        page["data"]["repository"]["pullRequest"]["isMergeQueueEnabled"] = json!(queued);
        let response = format!(
            "printf '%s' {}",
            shell_literal(&super::queue_tests::enqueue_response().to_string())
        );
        let authority = if queued {
            super::queue_tests::queue_authority(
                root.path(),
                std::slice::from_ref(&page),
                &[("enqueuePullRequest", &response)],
            )
        } else {
            merge_authority(root.path(), &page, &page, merge_receipt)
        };
        let observation = authority
            .inspect_review(&review(), SourceDeliveryCredential::token("test-token"))
            .await
            .assert_value();
        assert!(matches!(
            observation.state,
            SourceReviewState::Open {
                checks: SourceChecks::NotRequired
            }
        ));
        if queued {
            assert_eq!(
                authority
                    .request_merge(
                        &review(),
                        SourceDeliveryCredential::token("test-token"),
                        &TestReviewCheckpoint
                    )
                    .await
                    .assert_value(),
                SourceMergeRequestOutcome::Accepted
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn conversation_merge_rejection_race_refreshes_blockers_without_masking_other_failures() {
    use super::super::conversations::test_support::{fixture, save, state};
    let root = tempfile::tempdir().unwrap();
    let program = fixture(root.path(), &review(), "Bot");
    let mut data = state(&program);
    let mut policy = policy_page("MERGEABLE", "CLEAN", None, (false, None));
    policy["data"]["repository"]["pullRequest"]["isMergeQueueEnabled"] = json!(true);
    data["policy"] = policy;
    data["threads"][0]["resolved"] = json!(true);
    save(&program, &data);
    let provider = authority(program.clone(), root.path());
    let credential = SourceDeliveryCredential::token("test-token");
    assert!(
        provider
            .inspect_review_feedback(&review(), credential, &TestReviewCheckpoint)
            .await
            .unwrap()
            .conversations
            .is_empty()
    );
    assert_eq!(
        provider
            .request_merge(&review(), credential, &TestReviewCheckpoint)
            .await
            .unwrap(),
        SourceMergeRequestOutcome::ReviewBlocked
    );
    assert_eq!(
        provider
            .withdraw_merge_request(&review(), credential)
            .await
            .unwrap(),
        SourceMergeWithdrawal::Withdrawn
    );
    assert_eq!(
        provider
            .inspect_review_feedback(&review(), credential, &TestReviewCheckpoint)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    for failure in [
        SourceDeliveryError::api(Some(403), "Resource not accessible"),
        SourceDeliveryError::api(None, "Unknown schema field"),
        SourceDeliveryError::api(Some(401), "A conversation must be resolved"),
    ] {
        assert!(
            provider
                .classify_rejected_merge(
                    &review(),
                    credential,
                    (
                        merge_policy::MergeMethod::Queue,
                        &api::MutationFailure::unknown(failure)
                    ),
                )
                .await
                .is_err()
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn merge_rejection_routing_ignores_command_paths_and_credential_redaction() {
    use super::super::conversations::test_support::{fixture, save, state};
    let rejected = json!({"data":{"enqueuePullRequest":null},"errors":[{
        "type":"UNPROCESSABLE","path":["enqueuePullRequest"],
        "message":"A conversation must be resolved before this pull request can be merged."}]});
    let schema = json!({"errors":[{"type":"GRAPHQL_VALIDATION_FAILED",
        "message":"Unknown schema field enqueuePullRequest"}]});
    let mut incomplete = rejected.clone();
    incomplete.as_object_mut().unwrap().remove("data");
    for (directory, token, response, repair) in [
        ("ordinary", "test-token", rejected.clone(), true),
        (
            "credential-collision",
            "conversation",
            rejected.clone(),
            true,
        ),
        ("type-collision", "UNPROCESSABLE", rejected.clone(), true),
        (
            "mutation-collision",
            "enqueuePullRequest",
            rejected.clone(),
            true,
        ),
        ("ordinary-schema", "test-token", schema.clone(), false),
        (
            "schema-A conversation must be resolved",
            "test-token",
            schema,
            false,
        ),
        (
            "incomplete-A conversation must be resolved",
            "test-token",
            incomplete,
            false,
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join(directory);
        std::fs::create_dir(&home).unwrap();
        let program = fixture(&home, &review(), "Bot");
        let mut data = state(&program);
        let mut policy = policy_page("MERGEABLE", "CLEAN", None, (false, None));
        policy["data"]["repository"]["pullRequest"]["isMergeQueueEnabled"] = json!(true);
        data["policy"] = policy;
        data["expected_token"] = json!(token);
        data["merge_rejection"] = response;
        data["threads"][0]["resolved"] = json!(true);
        save(&program, &data);
        let provider = authority(program.clone(), &home);
        let outcome = provider
            .request_merge(
                &review(),
                SourceDeliveryCredential::token(token),
                &TestReviewCheckpoint,
            )
            .await;
        if repair {
            assert_eq!(outcome.unwrap(), SourceMergeRequestOutcome::ReviewBlocked);
            // Retained queue rejection evidence must still allow confirmed withdrawal.
            assert_eq!(
                provider
                    .withdraw_merge_request(&review(), SourceDeliveryCredential::token(token))
                    .await
                    .unwrap(),
                SourceMergeWithdrawal::Withdrawn
            );
        } else {
            let failure = outcome.unwrap_err();
            assert!(matches!(failure, SourceDeliveryError::Api(_)));
            assert!(!failure.to_string().contains(token));
        }
        assert_eq!(
            state(&program)["threads"][0]["resolved"],
            json!(false),
            "each rejection races with the same authoritative unresolved blocker"
        );
    }
}
