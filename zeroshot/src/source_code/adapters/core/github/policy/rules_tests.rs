use super::*;

fn required_rule(contexts: &[&str]) -> Value {
    json!({
        "type": "required_status_checks",
        "parameters": {
            "required_status_checks": contexts.iter().map(|context| {
                json!({"context": context, "integration_id": null})
            }).collect::<Vec<_>>()
        }
    })
}

fn pinned_rule(contexts: &[&str], integration_id: u64) -> Value {
    let mut rule = required_rule(contexts);
    for check in rule["parameters"]["required_status_checks"]
        .as_array_mut()
        .assert_value()
    {
        check["integration_id"] = json!(integration_id);
    }
    rule
}

fn ruleset_policy(contexts: Option<Vec<Value>>) -> Value {
    let mut page = policy_page("MERGEABLE", "CLEAN", contexts, (false, None));
    page["data"]["repository"]["pullRequest"]["baseRef"]["refUpdateRule"] = Value::Null;
    page
}

fn assert_pending(snapshot: &PolicySnapshot) {
    assert_eq!(
        snapshot.state,
        SourceReviewState::Open {
            checks: SourceChecks::Pending
        }
    );
    assert!(!snapshot.pull_request_ready);
}

#[test]
fn branch_rules_request_paginates_and_encodes_slash_bearing_target_branch() {
    let mut review = review();
    review.target_branch = "release/2026.10".to_owned();
    let arguments = policy::branch_rules_arguments(&review).assert_value();
    assert_eq!(
        arguments.first().map(String::as_str),
        Some("repos/acme/project/rules/branches/release%2F2026.10?per_page=100")
    );
    assert!(arguments.iter().any(|argument| argument == "--paginate"));
    assert!(arguments.iter().any(|argument| argument == "--slurp"));
    assert!(arguments.windows(2).any(|pair| pair == ["--method", "GET"]));

    review.target_branch = "release/../main".to_owned();
    policy::branch_rules_arguments(&review).assert_error();
}

#[test]
fn missing_ruleset_check_stays_pending_when_classic_protection_is_null() {
    let mut snapshot = classify(ruleset_policy(Some(vec![check_run(
        "semantic",
        "COMPLETED",
        Some("SUCCESS"),
        true,
    )])));
    assert_eq!(
        snapshot.state,
        SourceReviewState::Open {
            checks: SourceChecks::Passed
        }
    );
    snapshot
        .apply_branch_rules(json!([[required_rule(&["semantic", "required"])]]))
        .assert_value();
    assert_pending(&snapshot);
}

#[test]
fn optional_same_name_checks_cannot_satisfy_a_required_ruleset_context() {
    for optional in [
        check_run("required", "COMPLETED", Some("SUCCESS"), false),
        status_context("required", "SUCCESS", false),
    ] {
        let mut snapshot = classify(ruleset_policy(Some(vec![
            check_run("semantic", "COMPLETED", Some("SUCCESS"), true),
            optional,
        ])));
        snapshot
            .apply_branch_rules(json!([[required_rule(&["semantic", "required"])]]))
            .assert_value();
        assert_pending(&snapshot);
    }
}

#[test]
fn same_name_required_checks_must_match_every_pinned_app() {
    let rules = json!([[
        pinned_rule(&["required"], 15368),
        pinned_rule(&["required"], 9000)
    ]]);
    for app_b_required in [None, Some(false), Some(true)] {
        let mut contexts = vec![check_run("required", "COMPLETED", Some("SUCCESS"), true)];
        if let Some(required) = app_b_required {
            let mut app_b = check_run("required", "COMPLETED", Some("SUCCESS"), required);
            app_b["checkSuite"]["app"]["databaseId"] = json!(9000);
            contexts.push(app_b);
        }
        let mut snapshot = classify(ruleset_policy(Some(contexts)));
        snapshot.apply_branch_rules(rules.clone()).assert_value();
        if app_b_required == Some(true) {
            assert_eq!(
                snapshot.state,
                SourceReviewState::Open {
                    checks: SourceChecks::Passed
                }
            );
        } else {
            assert_pending(&snapshot);
        }
    }
}

#[test]
fn legacy_status_proves_unbound_check_but_cannot_prove_a_pinned_app() {
    for integration_id in [None, Some(Value::Null), Some(json!(15368))] {
        let mut rule = required_rule(&["required"]);
        let check = &mut rule["parameters"]["required_status_checks"][0];
        if let Some(integration_id) = &integration_id {
            check["integration_id"] = integration_id.clone();
        } else {
            check
                .as_object_mut()
                .assert_value()
                .remove("integration_id");
        }
        let mut snapshot = classify(ruleset_policy(Some(vec![status_context(
            "required", "SUCCESS", true,
        )])));
        snapshot.apply_branch_rules(json!([[rule]])).assert_value();
        if integration_id.as_ref().is_some_and(Value::is_number) {
            assert_pending(&snapshot);
        } else {
            assert_eq!(
                snapshot.state,
                SourceReviewState::Open {
                    checks: SourceChecks::Passed
                }
            );
        }
    }
}

#[test]
fn missing_ruleset_check_also_blocks_an_approval_handoff() {
    let mut page = policy_page(
        "MERGEABLE",
        "BLOCKED",
        Some(vec![check_run(
            "semantic",
            "COMPLETED",
            Some("SUCCESS"),
            true,
        )]),
        (false, None),
    );
    let pull = &mut page["data"]["repository"]["pullRequest"];
    pull["reviewDecision"] = json!("REVIEW_REQUIRED");
    pull["baseRef"]["refUpdateRule"]["requiredApprovingReviewCount"] = json!(1);
    let mut snapshot = classify(page);
    assert!(snapshot.pull_request_ready);
    snapshot
        .apply_branch_rules(json!([[required_rule(&["semantic", "required"])]]))
        .assert_value();
    assert_pending(&snapshot);
}

#[test]
fn every_rules_page_contributes_required_checks() {
    let pages = json!([
        vec![json!({"type": "deletion"}); 100],
        [required_rule(&["semantic"])],
        [required_rule(&["required"])]
    ]);
    for required_registered in [false, true] {
        let mut contexts = vec![check_run("semantic", "COMPLETED", Some("SUCCESS"), true)];
        if required_registered {
            contexts.push(status_context("required", "SUCCESS", true));
        }
        let mut snapshot = classify(ruleset_policy(Some(contexts)));
        snapshot.apply_branch_rules(pages.clone()).assert_value();
        if required_registered {
            assert_eq!(
                snapshot.state,
                SourceReviewState::Open {
                    checks: SourceChecks::Passed
                }
            );
        } else {
            assert_pending(&snapshot);
        }
    }
}

#[test]
fn no_required_rules_preserves_genuine_no_ci_readiness() {
    for pages in [
        json!([[]]),
        json!([[{"type": "required_deployments"}, {"type": "future_rule", "parameters": null}]]),
        json!([[required_rule(&[])]]),
    ] {
        let mut snapshot = classify(ruleset_policy(None));
        snapshot.apply_branch_rules(pages).assert_value();
        assert_eq!(
            snapshot.state,
            SourceReviewState::Open {
                checks: SourceChecks::NotRequired
            }
        );
    }
}

#[test]
fn malformed_required_check_rules_fail_closed() {
    let malformed_parameters = [
        Value::Null,
        json!({}),
        json!({"required_status_checks": null}),
        json!({"required_status_checks": {"context": "required"}}),
        json!({"required_status_checks": [null]}),
        json!({"required_status_checks": [{}]}),
        json!({"required_status_checks": [{"context": null}]}),
        json!({"required_status_checks": [{"context": 17}]}),
        json!({"required_status_checks": [{"context": ""}]}),
        json!({"required_status_checks": [{"context": "  "}]}),
        json!({"required_status_checks": [{"context": "required", "integration_id": 0}]}),
        json!({"required_status_checks": [{"context": "required", "integration_id": -1}]}),
        json!({"required_status_checks": [{"context": "required", "integration_id": "15368"}]}),
    ];
    let mut payloads = vec![
        Value::Null,
        json!({}),
        json!([]),
        json!([null]),
        json!([[null]]),
        json!([[{"type": "required_status_checks"}]]),
    ];
    payloads.extend(
        malformed_parameters.into_iter().map(
            |parameters| json!([[{"type": "required_status_checks", "parameters": parameters}]]),
        ),
    );
    for payload in payloads {
        let mut snapshot = classify(ruleset_policy(None));
        let error = snapshot.apply_branch_rules(payload.clone()).assert_error();
        assert!(
            matches!(error, SourceDeliveryError::Api(_)),
            "{payload}: {error}"
        );
        assert!(!error.retryable_operation(), "{payload}: {error}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn queue_waits_for_ruleset_required_check_before_enqueueing() {
    let root = tempfile::tempdir().assert_value();
    std::fs::write(
        root.path().join("branch-rules.json"),
        json!([[pinned_rule(&["semantic", "required"], 15368)]]).to_string(),
    )
    .assert_value();
    let mut missing = ruleset_policy(Some(vec![check_run(
        "semantic",
        "COMPLETED",
        Some("SUCCESS"),
        true,
    )]));
    missing["data"]["repository"]["pullRequest"]["isMergeQueueEnabled"] = json!(true);
    missing["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("BLOCKED");
    let mut registered = missing.clone();
    registered["data"]["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]
        ["statusCheckRollup"]["contexts"]["nodes"]
        .as_array_mut()
        .assert_value()
        .push(check_run("required", "COMPLETED", Some("SUCCESS"), true));
    let response = format!(
        "printf '%s' {}",
        shell_literal(&queue_tests::enqueue_response().to_string())
    );
    let provider = queue_tests::queue_authority(
        root.path(),
        &[missing.clone(), missing, registered],
        &[("enqueuePullRequest", &response)],
    );
    let observation = provider
        .inspect_review(&review(), SourceDeliveryCredential::token("test-token"))
        .await
        .assert_value();
    assert_eq!(
        observation.state,
        SourceReviewState::Open {
            checks: SourceChecks::Pending
        }
    );
    assert!(!observation.pull_request_ready);
    assert_eq!(
        provider
            .request_merge(&review(), SourceDeliveryCredential::token("test-token"))
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Pending
    );
    assert!(!root.path().join("mutations").exists());
    assert_eq!(
        provider
            .request_merge(&review(), SourceDeliveryCredential::token("test-token"))
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Accepted
    );
    let mutations = std::fs::read_to_string(root.path().join("mutations")).assert_value();
    assert_eq!(mutations.matches("expectedHeadOid=").count(), 1);
    assert!(mutations.contains(&review().head_revision));
    let rules = std::fs::read_to_string(root.path().join("rules-reads")).assert_value();
    assert_eq!(
        rules
            .lines()
            .filter(|line| *line == "repos/acme/project/rules/branches/main?per_page=100")
            .count(),
        3
    );
}

#[cfg(unix)]
#[tokio::test]
async fn branch_rules_errors_propagate_through_authority_without_enqueueing() {
    let malformed = json!([
        [required_rule(&["semantic"])],
        [{"type": "required_status_checks", "parameters": {"required_status_checks": [{"context": null}]}}]
    ]);
    for (file, contents, status, retryable) in [
        ("branch-rules.json", malformed.to_string(), None, false),
        (
            "branch-rules-error",
            "HTTP 403: Resource not accessible".to_owned(),
            Some(403),
            false,
        ),
        (
            "branch-rules-error",
            "HTTP 503: Unavailable".to_owned(),
            Some(503),
            true,
        ),
    ] {
        let root = tempfile::tempdir().assert_value();
        std::fs::write(root.path().join(file), contents).assert_value();
        let mut page = ruleset_policy(Some(vec![check_run(
            "semantic",
            "COMPLETED",
            Some("SUCCESS"),
            true,
        )]));
        page["data"]["repository"]["pullRequest"]["isMergeQueueEnabled"] = json!(true);
        let response = format!(
            "printf '%s' {}",
            shell_literal(&queue_tests::enqueue_response().to_string())
        );
        let provider = queue_tests::queue_authority(
            root.path(),
            &[page],
            &[("enqueuePullRequest", &response)],
        );
        let inspection_error = provider
            .inspect_review(&review(), SourceDeliveryCredential::token("test-token"))
            .await
            .assert_error();
        let request_error = provider
            .request_merge(&review(), SourceDeliveryCredential::token("test-token"))
            .await
            .assert_error();
        for error in [inspection_error, request_error] {
            assert!(matches!(error, SourceDeliveryError::Api(_)), "{error}");
            assert_eq!(error.api_status(), status, "{error}");
            assert_eq!(error.retryable_operation(), retryable, "{error}");
        }
        assert!(root.path().join("rules-reads").exists());
        assert!(!root.path().join("mutations").exists());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn failed_required_check_keeps_ci_diagnostics_without_a_rules_read() {
    let root = tempfile::tempdir().assert_value();
    std::fs::write(
        root.path().join("branch-rules.json"),
        "invalid rules payload",
    )
    .assert_value();
    let page = ruleset_policy(Some(vec![
        check_run("semantic", "COMPLETED", Some("SUCCESS"), true),
        check_run("required", "COMPLETED", Some("FAILURE"), true),
    ]));
    let program = write_policy_fixture(
        root.path(),
        &page,
        "printf '%s\\n' 'required workflow assertion failed'",
    );
    let provider = authority(program, root.path());
    let observation = provider
        .inspect_review(&review(), SourceDeliveryCredential::token("test-token"))
        .await
        .assert_value();
    let SourceReviewState::Open {
        checks: SourceChecks::Failed { diagnostic },
    } = observation.state
    else {
        panic!("required check failure must retain the CI repair outcome");
    };
    assert!(diagnostic.contains("required concluded FAILURE"));
    assert!(diagnostic.contains("GitHub Actions job 91 log excerpt"));
    assert!(diagnostic.contains("required workflow assertion failed"));
    assert!(!root.path().join("rules-reads").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn pending_and_terminal_policy_do_not_depend_on_branch_rules() {
    let pending = ruleset_policy(Some(vec![check_run("required", "QUEUED", None, true)]));
    let conflict = policy_page("CONFLICTING", "DIRTY", None, (false, None));
    let mut closed = ruleset_policy(None);
    set_review_state(&mut closed, "CLOSED", false);
    let mut merged = ruleset_policy(None);
    set_review_state(&mut merged, "MERGED", true);
    let merge_revision = "c".repeat(40);
    merged["data"]["repository"]["pullRequest"]["mergeCommit"] = json!({"oid": merge_revision});
    for (page, expected) in [
        (
            pending,
            SourceReviewState::Open {
                checks: SourceChecks::Pending,
            },
        ),
        (conflict, SourceReviewState::Conflict),
        (closed, SourceReviewState::Closed),
        (merged, SourceReviewState::Merged { merge_revision }),
    ] {
        let root = tempfile::tempdir().assert_value();
        std::fs::write(
            root.path().join("branch-rules.json"),
            "invalid rules payload",
        )
        .assert_value();
        let program = write_policy_fixture(root.path(), &page, "exit 19");
        let observation = authority(program, root.path())
            .inspect_review(&review(), SourceDeliveryCredential::token("test-token"))
            .await
            .assert_value();
        assert_eq!(observation.state, expected);
        assert!(!root.path().join("rules-reads").exists());
    }
}
