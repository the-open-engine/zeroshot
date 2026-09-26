use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::{Value, json};

use super::*;
#[cfg(unix)]
use super::observation::test_support::{authority, shell_literal, write_executable};
use super::merge_policy::MergeMethod;
use crate::native_v2_delivery::GitHubChecks;

#[cfg(unix)]
#[path = "merge_tests.rs"]
mod merge_tests;

pub(super) fn review() -> GitHubReviewReceipt {
    GitHubReviewReceipt {
        review_id: "17".to_owned(),
        repository: "acme/project".to_owned(),
        target_branch: "main".to_owned(),
        head_branch: "zeroshot/v2-run".to_owned(),
        head_revision: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
    }
}

fn check_run(name: &str, status: &str, conclusion: Option<&str>, required: bool) -> Value {
    json!({
        "__typename": "CheckRun",
        "name": name,
        "status": status,
        "conclusion": conclusion,
        "detailsUrl": "https://github.com/acme/project/actions/runs/17",
        "databaseId": 91,
        "isRequired": required
    })
}

fn status_context(context: &str, state: &str, required: bool) -> Value {
    json!({
        "__typename": "StatusContext",
        "context": context,
        "state": state,
        "description": "legacy CI rejected the revision",
        "targetUrl": "https://ci.example.invalid/build/17",
        "isRequired": required
    })
}

fn policy_page(
    mergeable: &str,
    merge_state_status: &str,
    contexts: Option<Vec<Value>>,
    pagination: (bool, Option<&str>),
) -> Value {
    let (has_next_page, end_cursor) = pagination;
    let rollup = contexts.map(|nodes| {
        json!({
            "contexts": {
                "pageInfo": {
                    "hasNextPage": has_next_page,
                    "endCursor": end_cursor
                },
                "nodes": nodes
            }
        })
    });
    json!({
        "data": {
            "repository": {
                "nameWithOwner": "acme/project",
                "mergeCommitAllowed": true,
                "squashMergeAllowed": true,
                "rebaseMergeAllowed": true,
                "pullRequest": {
                    "id": "PR_node_17",
                    "number": 17,
                    "state": "OPEN",
                    "merged": false,
                    "mergeCommit": null,
                    "mergeable": mergeable,
                    "mergeStateStatus": merge_state_status,
                    "isDraft": false,
                    "isInMergeQueue": false,
                    "isMergeQueueEnabled": false,
                    "baseRefName": "main",
                    "baseRef": {
                        "name": "main",
                        "branchProtectionRule": null,
                        "rules": {"totalCount": 0, "nodes": [], "pageInfo": {"hasNextPage": false, "endCursor": null}},
                        "refUpdateRule": {
                            "requiredApprovingReviewCount": 0,
                            "requiredStatusCheckContexts": [],
                            "requiresCodeOwnerReviews": false,
                            "requiresConversationResolution": false,
                            "requiresLinearHistory": false,
                            "requiresSignatures": false
                        }
                    },
                    "headRefName": "zeroshot/v2-run",
                    "headRefOid": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "commits": {
                        "nodes": [{
                            "commit": {
                                "statusCheckRollup": rollup
                            }
                        }]
                    }
                }
            }
        }
    })
}

fn classify(page: Value) -> PolicySnapshot {
    classify_policy(json!([page]), &review()).assert_value()
}

fn policy_page_with_merge_capabilities(merge: bool, squash: bool, rebase: bool) -> Value {
    let mut page = policy_page("MERGEABLE", "CLEAN", None, (false, None));
    let repository = page.pointer_mut("/data/repository").assert_value();
    repository["mergeCommitAllowed"] = json!(merge);
    repository["squashMergeAllowed"] = json!(squash);
    repository["rebaseMergeAllowed"] = json!(rebase);
    page
}

#[cfg(unix)]
fn set_review_state(page: &mut Value, state: &str, merged: bool) {
    let review = page
        .pointer_mut("/data/repository/pullRequest")
        .assert_value();
    review["state"] = json!(state);
    review["merged"] = json!(merged);
}

#[cfg(unix)]
fn review_wire(request: &GitHubReviewRequest, title: Option<&str>, body: Option<&str>) -> Value {
    json!({
        "number": 17,
        "title": title,
        "body": body,
        "base": {
            "ref": request.target.target_branch,
            "sha": request.target.base_revision,
            "repo": {"full_name": request.target.repository}
        },
        "head": {
            "ref": request.head_branch,
            "sha": request.head_revision,
            "repo": {"full_name": request.target.repository}
        }
    })
}

#[cfg(unix)]
struct ReviewFixtureResponses {
    listed: Value,
    current: Value,
    created: Value,
    patched: Value,
}

#[cfg(unix)]
fn uniform_review_responses(listed: Value, exact: &Value) -> ReviewFixtureResponses {
    ReviewFixtureResponses {
        listed,
        current: exact.clone(),
        created: exact.clone(),
        patched: exact.clone(),
    }
}

#[cfg(unix)]
fn write_review_fixture(
    root: &std::path::Path,
    responses: ReviewFixtureResponses,
) -> std::path::PathBuf {
    let program = root.join("gh-review-fixture");
    let reference = json!({
        "ref": "refs/heads/zeroshot/v2-run",
        "object": {
            "sha": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "type": "commit"
        }
    });
    let source = format!(
        "#!/bin/sh\ncase \"$2:$4\" in\n\
         repos/acme/project/pulls:GET) printf '%s\\n' {} ;;\n\
         repos/acme/project/pulls:POST) printf '%s\\n' {} ;;\n\
         repos/acme/project/pulls/17:GET) printf '%s\\n' {} ;;\n\
         repos/acme/project/pulls/17:PATCH) printf '%s\\n' {} ;;\n\
         repos/acme/project/git/ref/heads/zeroshot/v2-run:) \
           printf '%s\\n' {} ;;\n\
         *) exit 19 ;;\n\
         esac\n",
        shell_literal(&responses.listed.to_string()),
        shell_literal(&responses.created.to_string()),
        shell_literal(&responses.current.to_string()),
        shell_literal(&responses.patched.to_string()),
        shell_literal(&reference.to_string()),
    );
    write_executable(&program, source);
    program
}

#[cfg(unix)]
fn review_authority(
    root: &std::path::Path,
    responses: ReviewFixtureResponses,
) -> GhCliDeliveryAuthority {
    authority(write_review_fixture(root, responses), root)
}

#[cfg(unix)]
fn write_policy_fixture(
    root: &std::path::Path,
    page: &Value,
    log_action: &str,
) -> std::path::PathBuf {
    let program = root.join("gh-policy-fixture");
    let source = format!(
        "#!/bin/sh\ncase \"$2\" in\n\
         graphql) printf '%s\\n' {} ;;\n\
         repos/acme/project/actions/jobs/91/logs) {} ;;\n\
         *) exit 19 ;;\n\
         esac\n",
        shell_literal(&json!([page]).to_string()),
        log_action,
    );
    write_executable(&program, source);
    program
}

fn classify_conclusion(conclusion: &str, merge_state: &str) -> PolicySnapshot {
    classify(policy_page(
        "MERGEABLE",
        merge_state,
        Some(vec![check_run(
            "required-ci",
            "COMPLETED",
            Some(conclusion),
            true,
        )]),
        (false, Some(conclusion)),
    ))
}

#[test]
fn policy_query_is_repository_generic_and_paginates_required_contexts() {
    let arguments = query_arguments(&review()).assert_value();
    assert_eq!(arguments.first().map(String::as_str), Some("graphql"));
    assert!(arguments.iter().any(|argument| argument == "--paginate"));
    assert!(arguments.iter().any(|argument| argument == "--slurp"));
    assert!(arguments.iter().any(|argument| argument == "owner=acme"));
    assert!(arguments.iter().any(|argument| argument == "name=project"));
    assert!(arguments.iter().any(|argument| argument == "number=17"));
    let query = arguments
        .iter()
        .find(|argument| argument.starts_with("query="))
        .assert_value();
    assert!(query.contains("isRequired(pullRequestNumber: $number)"));
    assert!(query.contains("statusCheckRollup"));
    assert!(query.contains("isMergeQueueEnabled"));
    assert!(!query.contains("mergeCommitAllowed"));
    assert!(query.contains("requiredApprovingReviewCount"));
    assert!(query.contains("requiresConversationResolution"));
    assert!(!query.contains("branchProtectionRule"));
    assert!(!query.contains("rules(first:"));
    assert_eq!(
        query.matches("pageInfo").count(),
        1,
        "gh paginates only check contexts"
    );
    assert!(!query.contains("allowedMergeMethods"));
}

#[test]
fn delayed_required_workflow_registration_stays_pending_until_github_is_ready() {
    let mut approval_blocked = policy_page("MERGEABLE", "BLOCKED", None, (false, None));
    approval_blocked["data"]["repository"]["pullRequest"]["reviewDecision"] =
        json!("REVIEW_REQUIRED");
    approval_blocked["data"]["repository"]["pullRequest"]["baseRef"]["refUpdateRule"]["requiredStatusCheckContexts"] =
        json!(["required-ci"]);
    let absent = classify(approval_blocked);
    assert_eq!(
        absent.state,
        GitHubReviewState::Open {
            checks: GitHubChecks::Pending
        }
    );
    assert!(!absent.pull_request_ready);

    let mut no_ci = policy_page("MERGEABLE", "BLOCKED", None, (false, None));
    no_ci["data"]["repository"]["pullRequest"]["reviewDecision"] = json!("REVIEW_REQUIRED");
    no_ci["data"]["repository"]["pullRequest"]["baseRef"]["refUpdateRule"]["requiredApprovingReviewCount"] =
        json!(1);
    let no_ci = classify(no_ci);
    assert_eq!(
        no_ci.state,
        GitHubReviewState::Open {
            checks: GitHubChecks::NotRequired
        }
    );
    assert!(no_ci.pull_request_ready);

    let queued = classify(policy_page(
        "MERGEABLE",
        "BLOCKED",
        Some(vec![check_run("required-ci", "QUEUED", None, true)]),
        (false, Some("queued")),
    ));
    assert_eq!(
        queued.state,
        GitHubReviewState::Open {
            checks: GitHubChecks::Pending
        }
    );

    let mut approval_blocked_ready = policy_page(
        "MERGEABLE",
        "BLOCKED",
        Some(vec![check_run(
            "required-ci",
            "COMPLETED",
            Some("SUCCESS"),
            true,
        )]),
        (false, Some("ready")),
    );
    approval_blocked_ready["data"]["repository"]["pullRequest"]["reviewDecision"] =
        json!("REVIEW_REQUIRED");
    *approval_blocked_ready
        .pointer_mut(
            "/data/repository/pullRequest/baseRef/refUpdateRule/requiredApprovingReviewCount",
        )
        .assert_value() = json!(1);
    *approval_blocked_ready
        .pointer_mut(
            "/data/repository/pullRequest/baseRef/refUpdateRule/requiredStatusCheckContexts",
        )
        .assert_value() = json!(["required-ci"]);
    let approval_blocked_ready = classify(approval_blocked_ready);
    assert_eq!(
        approval_blocked_ready.state,
        GitHubReviewState::Open {
            checks: GitHubChecks::Passed
        }
    );
    assert!(approval_blocked_ready.pull_request_ready);

    let ready = classify(policy_page(
        "MERGEABLE",
        "CLEAN",
        Some(vec![check_run(
            "required-ci",
            "COMPLETED",
            Some("SUCCESS"),
            true,
        )]),
        (false, Some("ready")),
    ));
    assert_eq!(
        ready.state,
        GitHubReviewState::Open {
            checks: GitHubChecks::Passed
        }
    );
    assert!(!ready.pull_request_ready);
}

#[test]
fn approval_exception_requires_positive_review_handoff_policy_evidence() {
    for pointer in [
        "/data/repository/pullRequest/baseRef/refUpdateRule/requiresConversationResolution",
        "/data/repository/pullRequest/baseRef/refUpdateRule/requiresLinearHistory",
        "/data/repository/pullRequest/baseRef/refUpdateRule/requiresSignatures",
    ] {
        let mut page = policy_page("MERGEABLE", "BLOCKED", None, (false, None));
        page["data"]["repository"]["pullRequest"]["reviewDecision"] = json!("REVIEW_REQUIRED");
        page["data"]["repository"]["pullRequest"]["baseRef"]["refUpdateRule"]["requiredApprovingReviewCount"] =
            json!(1);
        *page.pointer_mut(pointer).assert_value() = json!(true);
        let snapshot = classify(page);
        assert_eq!(
            snapshot.state,
            GitHubReviewState::Open {
                checks: GitHubChecks::Pending
            }
        );
        assert!(!snapshot.pull_request_ready);
    }

    let mut missing_rule = policy_page("MERGEABLE", "BLOCKED", None, (false, None));
    missing_rule["data"]["repository"]["pullRequest"]["reviewDecision"] = json!("REVIEW_REQUIRED");
    missing_rule["data"]["repository"]["pullRequest"]["baseRef"]["refUpdateRule"] = Value::Null;
    assert!(!classify(missing_rule).pull_request_ready);
}

#[test]
fn optional_checks_never_block_or_fail_delivery() {
    let snapshot = classify(policy_page(
        "MERGEABLE",
        "UNSTABLE",
        Some(vec![
            check_run("optional-failure", "COMPLETED", Some("FAILURE"), false),
            check_run("optional-pending", "IN_PROGRESS", None, false),
            status_context("optional-legacy", "FAILURE", false),
        ]),
        (false, Some("optional")),
    ));
    assert_eq!(
        snapshot.state,
        GitHubReviewState::Open {
            checks: GitHubChecks::NotRequired
        }
    );
    assert!(snapshot.failed_job_ids.is_empty());
}

#[test]
fn required_check_failures_win_and_preserve_diagnostics() {
    let mut snapshot = classify(policy_page(
        "MERGEABLE",
        "BLOCKED",
        Some(vec![
            check_run("required-actions", "COMPLETED", Some("FAILURE"), true),
            status_context("required-legacy", "ERROR", true),
            check_run("still-running", "IN_PROGRESS", None, true),
        ]),
        (false, Some("failed")),
    ));
    assert_eq!(snapshot.failed_job_ids, vec![91]);
    include_check_logs(
        &mut snapshot,
        &[(
            91,
            "setup passed\nAssertionError: validation failed".to_owned(),
        )],
    );
    let diagnostic = match snapshot.state {
        GitHubReviewState::Open {
            checks: GitHubChecks::Failed { diagnostic },
        } => Some(diagnostic),
        _ => None,
    }
    .assert_value_with("expected required CI failure");
    assert!(diagnostic.contains("required-actions concluded FAILURE"));
    assert!(diagnostic.contains("required-legacy concluded ERROR"));
    assert!(diagnostic.contains("legacy CI rejected the revision"));
    assert!(diagnostic.contains("AssertionError: validation failed"));
}

#[test]
fn every_known_terminal_check_run_conclusion_is_classified() {
    for conclusion in ["SUCCESS", "NEUTRAL", "SKIPPED"] {
        let snapshot = classify_conclusion(conclusion, "CLEAN");
        assert!(matches!(
            snapshot.state,
            GitHubReviewState::Open {
                checks: GitHubChecks::Passed
            }
        ));
    }
    for conclusion in [
        "FAILURE",
        "CANCELLED",
        "TIMED_OUT",
        "ACTION_REQUIRED",
        "STALE",
        "STARTUP_FAILURE",
    ] {
        let snapshot = classify_conclusion(conclusion, "BLOCKED");
        assert!(matches!(
            snapshot.state,
            GitHubReviewState::Open {
                checks: GitHubChecks::Failed { .. }
            }
        ));
    }
}

#[test]
fn unknown_or_nonterminal_provider_states_fail_closed_as_pending() {
    for (mergeable, merge_state) in [
        ("UNKNOWN", "UNKNOWN"),
        ("MERGEABLE", "BLOCKED"),
        ("MERGEABLE", "DRAFT"),
        ("MERGEABLE", "UNKNOWN"),
    ] {
        let snapshot = classify(policy_page(mergeable, merge_state, None, (false, None)));
        assert_eq!(
            snapshot.state,
            GitHubReviewState::Open {
                checks: GitHubChecks::Pending
            }
        );
    }
    let snapshot = classify(policy_page(
        "MERGEABLE",
        "CLEAN",
        Some(vec![check_run(
            "future-conclusion",
            "COMPLETED",
            Some("FUTURE_VALUE"),
            true,
        )]),
        (false, Some("future")),
    ));
    assert_eq!(
        snapshot.state,
        GitHubReviewState::Open {
            checks: GitHubChecks::Pending
        }
    );
}

#[test]
fn clean_repository_without_required_checks_is_ready() {
    for merge_state in ["BEHIND", "CLEAN", "HAS_HOOKS", "UNSTABLE"] {
        let snapshot = classify(policy_page("MERGEABLE", merge_state, None, (false, None)));
        assert_eq!(
            snapshot.state,
            GitHubReviewState::Open {
                checks: GitHubChecks::NotRequired
            }
        );
        assert_eq!(snapshot.head_update.is_some(), merge_state == "BEHIND");
    }
}

#[test]
fn drafts_and_queued_reviews_wait_for_github() {
    for field in ["isDraft", "isInMergeQueue"] {
        let mut page = policy_page("MERGEABLE", "CLEAN", None, (false, None));
        page.pointer_mut(&format!("/data/repository/pullRequest/{field}"))
            .assert_value()
            .clone_from(&json!(true));
        assert_eq!(
            classify(page).state,
            GitHubReviewState::Open {
                checks: GitHubChecks::Pending
            }
        );
    }
}

#[test]
fn merge_queue_policy_reaches_native_submission_without_ci_special_cases() {
    for merge_state in ["BLOCKED", "BEHIND", "UNKNOWN"] {
        let mut page = policy_page("UNKNOWN", merge_state, None, (false, None));
        page.pointer_mut("/data/repository/pullRequest/isMergeQueueEnabled")
            .assert_value()
            .clone_from(&json!(true));
        let snapshot = classify(page);
        assert_eq!(
            snapshot.state,
            GitHubReviewState::Open {
                checks: GitHubChecks::NotRequired
            }
        );
        assert!(snapshot.is_merge_queue_enabled);
        assert!(snapshot.head_update.is_none());
    }
}

#[test]
fn authoritative_merge_revision_is_preserved_for_every_merge_method() {
    for (merge, squash, rebase) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let mut page = policy_page_with_merge_capabilities(merge, squash, rebase);
        let repository = page.pointer_mut("/data/repository").assert_value();
        let pull_request = repository.pointer_mut("/pullRequest").assert_value();
        pull_request["state"] = json!("MERGED");
        pull_request["merged"] = json!(true);
        pull_request["mergeCommit"] = json!({"oid":"cccccccccccccccccccccccccccccccccccccccc"});

        assert_eq!(
            classify(page).state,
            GitHubReviewState::Merged {
                merge_revision: "cccccccccccccccccccccccccccccccccccccccc".to_owned()
            }
        );
    }
}

#[test]
fn pagination_must_be_complete_and_policy_stable() {
    let first = policy_page(
        "MERGEABLE",
        "CLEAN",
        Some(vec![check_run(
            "required-one",
            "COMPLETED",
            Some("SUCCESS"),
            true,
        )]),
        (true, Some("one")),
    );
    let second = policy_page(
        "MERGEABLE",
        "CLEAN",
        Some(vec![status_context("required-two", "SUCCESS", true)]),
        (false, Some("two")),
    );
    assert_eq!(
        classify_policy(json!([first.clone(), second]), &review())
            .assert_value()
            .state,
        GitHubReviewState::Open {
            checks: GitHubChecks::Passed
        }
    );
    assert_policy_pages_pending(vec![first]);

    let stable = policy_page("MERGEABLE", "CLEAN", None, (true, Some("one")));
    let changed = policy_page("MERGEABLE", "BLOCKED", None, (false, Some("two")));
    assert_policy_pages_pending(vec![stable, changed]);

    let stable = policy_page("MERGEABLE", "CLEAN", None, (true, Some("one")));
    let mut changed = policy_page("MERGEABLE", "CLEAN", None, (false, Some("two")));
    changed
        .pointer_mut("/data/repository/pullRequest/isMergeQueueEnabled")
        .assert_value()
        .clone_from(&json!(true));
    assert_policy_pages_pending(vec![stable, changed]);
}

fn assert_policy_pages_pending(pages: Vec<Value>) {
    assert_eq!(
        classify_policy(json!(pages), &review())
            .assert_value()
            .state,
        GitHubReviewState::Open {
            checks: GitHubChecks::Pending
        }
    );
}

#[test]
fn terminal_state_and_exact_identity_are_authoritative() {
    let mut conflict = policy_page("CONFLICTING", "DIRTY", None, (false, None));
    assert_eq!(
        classify(conflict.clone()).state,
        GitHubReviewState::Conflict
    );

    let mut closed = conflict.clone();
    let pull_request = closed
        .pointer_mut("/data/repository/pullRequest")
        .assert_value();
    pull_request["state"] = json!("CLOSED");
    pull_request["mergeable"] = json!("UNKNOWN");
    pull_request["mergeStateStatus"] = json!("UNKNOWN");
    assert_eq!(classify(closed).state, GitHubReviewState::Closed);

    let pull_request = conflict
        .pointer_mut("/data/repository/pullRequest")
        .assert_value();
    pull_request["state"] = json!("MERGED");
    pull_request["merged"] = json!(true);
    pull_request["mergeable"] = json!("MERGEABLE");
    pull_request["mergeStateStatus"] = json!("CLEAN");
    pull_request["mergeCommit"] = json!({"oid":"cccccccccccccccccccccccccccccccccccccccc"});
    assert_eq!(
        classify(conflict.clone()).state,
        GitHubReviewState::Merged {
            merge_revision: "cccccccccccccccccccccccccccccccccccccccc".to_owned()
        }
    );

    let pull_request = conflict
        .pointer_mut("/data/repository/pullRequest")
        .assert_value();
    pull_request["headRefOid"] = json!("dddddddddddddddddddddddddddddddddddddddd");
    assert!(classify_policy(json!([conflict]), &review()).is_err());
}

#[test]
fn receipt_rejects_changed_authority() {
    let request = test_review_request();
    let wire = serde_json::from_value(json!({
        "number": 17,
        "body": null,
        "base": {
            "ref": "other",
            "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "repo": {"full_name": "acme/project"}
        },
        "head": {
            "ref": "zeroshot/v2-run",
            "sha": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "repo": {"full_name": "acme/project"}
        }
    }))
    .assert_value();
    assert!(review_receipt(wire, &request).is_err());
}

#[test]
fn required_gate_failure_includes_supporting_job_failures() {
    let mut child = check_run("frontend-tests", "COMPLETED", Some("FAILURE"), false);
    child["databaseId"] = json!(92);
    let snapshot = classify(policy_page(
        "MERGEABLE",
        "BLOCKED",
        Some(vec![
            check_run("required-gate", "COMPLETED", Some("FAILURE"), true),
            child,
            status_context("external-tests", "ERROR", false),
        ]),
        (false, None),
    ));
    assert_eq!(snapshot.failed_job_ids, vec![91, 92]);
    let GitHubReviewState::Open {
        checks: GitHubChecks::Failed { diagnostic },
    } = snapshot.state
    else {
        panic!("expected failed required gate");
    };
    assert!(diagnostic.contains("required-gate concluded FAILURE"));
    assert!(diagnostic.contains("Supporting check: frontend-tests concluded FAILURE"));
    assert!(diagnostic.contains("Supporting check: external-tests concluded ERROR"));
}

#[test]
fn supporting_failures_do_not_change_pending_or_passed_required_policy() {
    for (status, conclusion, merge_state, expected) in [
        ("IN_PROGRESS", None, "BLOCKED", GitHubChecks::Pending),
        ("COMPLETED", Some("SUCCESS"), "CLEAN", GitHubChecks::Passed),
    ] {
        let snapshot = classify(policy_page(
            "MERGEABLE",
            merge_state,
            Some(vec![
                check_run("required", status, conclusion, true),
                check_run("optional", "COMPLETED", Some("FAILURE"), false),
            ]),
            (false, None),
        ));
        assert_eq!(snapshot.state, GitHubReviewState::Open { checks: expected });
        assert!(snapshot.failed_job_ids.is_empty());
    }
}

#[test]
fn failed_job_log_limit_is_explicit_and_preserves_failure_names() {
    let mut contexts = vec![check_run("required", "COMPLETED", Some("FAILURE"), true)];
    for index in 0..10 {
        let mut context = check_run(
            &format!("child-{index}"),
            "COMPLETED",
            Some("FAILURE"),
            false,
        );
        context["databaseId"] = json!(100 + index);
        contexts.push(context);
    }
    let snapshot = classify(policy_page(
        "MERGEABLE",
        "BLOCKED",
        Some(contexts),
        (false, None),
    ));
    assert_eq!(snapshot.failed_job_ids.len(), 8);
    let GitHubReviewState::Open {
        checks: GitHubChecks::Failed { diagnostic },
    } = snapshot.state
    else {
        panic!("expected failed required gate");
    };
    assert!(diagnostic.contains("child-9 concluded FAILURE"));
    assert!(diagnostic.contains("Log excerpts limited to 8 of 11 failed jobs"));
}

#[test]
fn large_first_log_cannot_hide_other_failed_job_excerpts() {
    let mut snapshot = classify_conclusion("FAILURE", "BLOCKED");
    let logs = (91..99)
        .map(|job| {
            (
                job,
                format!("{}\nassertion failed in job {job}", "界".repeat(70_000)),
            )
        })
        .collect::<Vec<_>>();
    include_check_logs(&mut snapshot, &logs);
    let GitHubReviewState::Open {
        checks: GitHubChecks::Failed { diagnostic },
    } = snapshot.state
    else {
        panic!("expected failed required gate");
    };
    assert!(diagnostic.chars().count() <= 64 * 1024);
    for job in 91..99 {
        assert!(diagnostic.contains(&format!("GitHub Actions job {job} log excerpt")));
        assert!(diagnostic.contains(&format!("assertion failed in job {job}")));
    }
    assert_eq!(
        diagnostic.matches("[earlier log output truncated]").count(),
        8
    );
}

#[test]
fn oversized_failure_summary_marks_omitted_text() {
    let contexts = (0..12)
        .map(|_| check_run(&"界".repeat(2000), "COMPLETED", Some("FAILURE"), true))
        .collect();
    let snapshot = classify(policy_page(
        "MERGEABLE",
        "BLOCKED",
        Some(contexts),
        (false, None),
    ));
    let GitHubReviewState::Open {
        checks: GitHubChecks::Failed { diagnostic },
    } = snapshot.state
    else {
        panic!("expected failed required checks");
    };
    assert_eq!(diagnostic.chars().count(), 8 * 1024);
    assert!(diagnostic.ends_with("[diagnostic truncated]"));
}

#[test]
fn merge_actions_preserve_terminal_authority_and_queue_ownership() {
    let snapshot = |state, queued| PolicySnapshot {
        state,
        failed_job_ids: Vec::new(),
        is_merge_queue_enabled: queued,
        head_update: None,
        pull_request_ready: false,
    };
    for (state, expected) in [
        (
            GitHubReviewState::Merged {
                merge_revision: "cccccccccccccccccccccccccccccccccccccccc".to_owned(),
            },
            GitHubMergeRequestOutcome::Accepted,
        ),
        (
            GitHubReviewState::Conflict,
            GitHubMergeRequestOutcome::Conflict,
        ),
        (
            GitHubReviewState::Open {
                checks: GitHubChecks::Pending,
            },
            GitHubMergeRequestOutcome::Pending,
        ),
    ] {
        let MergeAction::Complete(actual) = merge_action(snapshot(state, false)).assert_value()
        else {
            panic!("terminal policy must complete without submitting");
        };
        assert_eq!(actual, expected);
    }
    for queued in [false, true] {
        for checks in [GitHubChecks::NotRequired, GitHubChecks::Passed] {
            let MergeAction::Submit { queued: actual } =
                merge_action(snapshot(GitHubReviewState::Open { checks }, queued)).assert_value()
            else {
                panic!("ready policy must permit submission");
            };
            assert_eq!(actual, queued);
        }
    }
    assert_eq!(
        merge_action(snapshot(GitHubReviewState::Closed, false)).assert_error(),
        GitHubAuthorityError::Rejected
    );
}

#[test]
fn merge_flags_leave_method_selection_to_queues() {
    for (method, argument) in [
        (MergeMethod::Queue, None),
        (MergeMethod::Merge, Some("--merge")),
        (MergeMethod::Squash, Some("--squash")),
        (MergeMethod::Rebase, Some("--rebase")),
    ] {
        assert_eq!(merge_method_argument(method), argument);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn review_opening_rediscovery_and_metadata_refresh_are_identity_fenced() {
    let request = test_review_request();
    let expected_body = pull_request_body(&request).assert_value();
    let exact = review_wire(&request, Some(&request.title), Some(expected_body.as_str()));

    let existing_root = tempfile::tempdir().assert_value();
    let existing = review_authority(
        existing_root.path(),
        uniform_review_responses(json!([exact.clone()]), &exact),
    )
    .open_or_update_review(&request, GitHubCredential("test-token"))
    .await
    .assert_value();
    assert_eq!(existing, review());

    let created_root = tempfile::tempdir().assert_value();
    let created = review_authority(
        created_root.path(),
        uniform_review_responses(json!([]), &exact),
    )
    .open_or_update_review(&request, GitHubCredential("test-token"))
    .await
    .assert_value();
    assert_eq!(created, review());

    let refreshed_body = refresh_pull_request_body(Some("Human context."), &request).assert_value();
    let current = review_wire(&request, Some("stale title"), Some("Human context."));
    let refreshed = review_wire(
        &request,
        Some(&request.title),
        Some(refreshed_body.as_str()),
    );
    let refreshed_root = tempfile::tempdir().assert_value();
    review_authority(
        refreshed_root.path(),
        ReviewFixtureResponses {
            listed: json!([current.clone()]),
            current,
            created: refreshed.clone(),
            patched: refreshed,
        },
    )
    .refresh_review_metadata(&request, &review(), GitHubCredential("test-token"))
    .await
    .assert_value();

    let ambiguous_root = tempfile::tempdir().assert_value();
    let ambiguous = review_authority(
        ambiguous_root.path(),
        uniform_review_responses(json!([exact.clone(), exact.clone()]), &exact),
    )
    .find_review(&request, GitHubCredential("test-token"))
    .await
    .assert_error();
    assert_eq!(ambiguous, GitHubAuthorityError::Rejected);

    let mismatched = review_wire(
        &request,
        Some("provider changed title"),
        Some(&expected_body),
    );
    let mismatched_root = tempfile::tempdir().assert_value();
    let mismatched = review_authority(
        mismatched_root.path(),
        ReviewFixtureResponses {
            listed: json!([]),
            current: exact.clone(),
            created: mismatched.clone(),
            patched: mismatched,
        },
    )
    .create_review(&request, GitHubCredential("test-token"))
    .await
    .assert_error();
    assert_eq!(mismatched, GitHubAuthorityError::Rejected);
}

#[cfg(unix)]
#[tokio::test]
async fn failed_check_logs_are_enriched_but_transport_failures_remain_typed() {
    let page = policy_page(
        "MERGEABLE",
        "BLOCKED",
        Some(vec![check_run(
            "required-ci",
            "COMPLETED",
            Some("FAILURE"),
            true,
        )]),
        (false, None),
    );
    let root = tempfile::tempdir().assert_value();
    let program = write_policy_fixture(
        root.path(),
        &page,
        "printf '%s\\n' 'setup passed' 'assertion failed at boundary'",
    );
    let observation = authority(program, root.path())
        .inspect_review(&review(), GitHubCredential("test-token"))
        .await
        .assert_value();
    let GitHubReviewState::Open {
        checks: GitHubChecks::Failed { diagnostic },
    } = observation.state
    else {
        panic!("required failed check must remain failed");
    };
    assert!(diagnostic.contains("assertion failed at boundary"));

    let unavailable = job_log_excerpt(Err(GitHubAuthorityError::api(
        Some(403),
        "resource not accessible",
    )))
    .assert_value();
    assert!(unavailable.contains("GitHub job log unavailable"));

    for status in [401, 429] {
        let error = job_log_excerpt(Err(GitHubAuthorityError::api(
            Some(status),
            "transport refusal",
        )))
        .assert_error();
        assert_eq!(error.api_status(), Some(status));
        assert_eq!(error.authentication_failed(), status == 401);
        assert_eq!(error.retryable_operation(), status == 429);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn transient_merge_failure_is_reclassified_from_the_latest_authoritative_policy() {
    let mut merged = policy_page("MERGEABLE", "CLEAN", None, (false, None));
    set_review_state(&mut merged, "MERGED", true);
    merged["data"]["repository"]["pullRequest"]["mergeCommit"] =
        json!({"oid": "cccccccccccccccccccccccccccccccccccccccc"});
    let conflict = policy_page("CONFLICTING", "DIRTY", None, (false, None));
    let behind = policy_page("MERGEABLE", "BEHIND", None, (false, None));
    let pending = policy_page("UNKNOWN", "UNKNOWN", None, (false, None));
    let ready = policy_page("MERGEABLE", "CLEAN", None, (false, None));
    let mut closed = policy_page("UNKNOWN", "UNKNOWN", None, (false, None));
    set_review_state(&mut closed, "CLOSED", false);

    let cases = [
        (merged, Ok(GitHubMergeRequestOutcome::Accepted)),
        (conflict, Ok(GitHubMergeRequestOutcome::Conflict)),
        (behind, Ok(GitHubMergeRequestOutcome::HeadUpdateRequired)),
        (pending, Err(GitHubAuthorityError::Rejected)),
        (ready, Err(GitHubAuthorityError::Rejected)),
        (closed, Err(GitHubAuthorityError::Rejected)),
    ];
    for (page, expected) in cases {
        let root = tempfile::tempdir().assert_value();
        let program = write_policy_fixture(root.path(), &page, "exit 19");
        let actual = authority(program, root.path())
            .classify_rejected_merge(
                &review(),
                GitHubCredential("test-token"),
                &GitHubAuthorityError::api(None, "merge transport failed").temporary(),
            )
            .await;
        assert_eq!(actual, expected);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn bounded_git_output_refuses_truncated_success() {
    let mut command = tokio::process::Command::new("/bin/sh");
    command.args(["-c", "printf 'candidate-output'"]);
    let error = bounded_git_output(&mut command, Duration::from_secs(1), 8)
        .await
        .assert_error();
    assert!(error.to_string().contains("exceeded 8 bytes"));
    assert!(error.to_string().contains("candidate-output"));
}
