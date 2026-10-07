use super::*;

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

fn assert_policy_error(error: &GitHubAuthorityError) {
    assert!(matches!(error, GitHubAuthorityError::Api(_)), "{error}");
    assert!(!error.retryable_operation(), "{error}");
    assert!(!error.authentication_failed(), "{error}");
}

#[cfg(unix)]
fn merge_authority(
    root: &std::path::Path,
    before: &Value,
    after: &Value,
    action: &str,
) -> GhCliDeliveryAuthority {
    let program = root.join("gh-merge-fixture");
    write_executable(
        &program,
        format!(
            "#!/bin/sh\ncase \"$2\" in\n\
         graphql) if [ -f \"$HOME/merge-args\" ]; then printf '%s\\n' {}; else printf '%s\\n' {}; fi ;;\n\
         merge) printf '%s\\n' \"$@\" >> \"$HOME/merge-args\"; {action} ;;\n\
         *) exit 19 ;;\nesac\n",
            shell_literal(&json!([after]).to_string()),
            shell_literal(&json!([before]).to_string()),
        ),
    );
    authority(program, root)
}

#[cfg(unix)]
#[tokio::test]
async fn merge_commands_use_the_selected_method_and_exact_head_without_bypass() {
    for (method, flag) in [
        ("MERGE", Some("--merge")),
        ("SQUASH", Some("--squash")),
        ("REBASE", Some("--rebase")),
        ("QUEUE", None),
    ] {
        let root = tempfile::tempdir().assert_value();
        let mut page = policy_page_with_merge_capabilities(true, true, true);
        set_rules(&mut page, vec![method_rule(json!([method]))]);
        page["data"]["repository"]["pullRequest"]["isMergeQueueEnabled"] = json!(method == "QUEUE");
        let outcome = merge_authority(root.path(), &page, &page, "exit 0")
            .request_merge(&review(), GitHubCredential("test-token"))
            .await
            .assert_value();
        assert_eq!(outcome, GitHubMergeRequestOutcome::Accepted);
        let args = std::fs::read_to_string(root.path().join("merge-args")).assert_value();
        assert!(args.contains(&format!(
            "--match-head-commit\n{}\n",
            review().head_revision
        )));
        assert!(!args.contains("--admin"));
        for possible in ["--merge", "--squash", "--rebase"] {
            assert_eq!(
                args.lines().any(|arg| arg == possible),
                flag == Some(possible)
            );
        }
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
        merge_authority(root.path(), &page, &page, "exit 0")
            .request_merge(&review(), GitHubCredential("test-token"))
            .await
            .assert_error();
        assert!(!root.path().join("merge-args").exists());
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
        (queued, GitHubMergeRequestOutcome::Pending),
        (merged, GitHubMergeRequestOutcome::Accepted),
        (review_required, GitHubMergeRequestOutcome::Pending),
        (behind, GitHubMergeRequestOutcome::HeadUpdateRequired),
    ] {
        let root = tempfile::tempdir().assert_value();
        let actual = merge_authority(root.path(), &page, &page, "exit 19")
            .request_merge(&review(), GitHubCredential("test-token"))
            .await
            .assert_value();
        assert_eq!(actual, expected);
        assert!(!root.path().join("merge-args").exists());
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
        for rejection in [
            "GraphQL: Merge commits are not allowed on this repository. (mergePullRequest)",
            "HTTP 403: Resource not accessible (https://api.github.com/graphql)",
            "HTTP 422: Validation Failed (https://api.github.com/graphql)",
        ] {
            let root = tempfile::tempdir().assert_value();
            let action = format!(
                "printf '%s\\n' {} \"$GH_TOKEN\" >&2; exit 1",
                shell_literal(rejection)
            );
            let error = merge_authority(root.path(), &ready, &after, &action)
                .request_merge(&review(), GitHubCredential("test-token"))
                .await
                .assert_error();
            assert_policy_error(&error);
            assert!(error.to_string().contains(rejection));
            assert!(error.to_string().contains("acme/project#17 into main"));
            assert!(!error.to_string().contains("test-token"));
            let args = std::fs::read_to_string(root.path().join("merge-args")).assert_value();
            assert_eq!(args.lines().filter(|arg| *arg == "merge").count(), 1);
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn merge_transport_errors_preserve_authentication_and_retry_classification() {
    let page = policy_page_with_merge_capabilities(true, true, true);
    for status in [401, 429, 503] {
        let root = tempfile::tempdir().assert_value();
        let action = format!(
            "printf '%s\\n' 'HTTP {status}: request failed (https://api.github.com/graphql)' >&2; exit 1"
        );
        let error = merge_authority(root.path(), &page, &page, &action)
            .request_merge(&review(), GitHubCredential("test-token"))
            .await
            .assert_error();
        assert_eq!(error.api_status(), Some(status));
        assert_eq!(error.authentication_failed(), status == 401);
        assert_eq!(error.retryable_operation(), status != 401);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn permanent_http_rejection_cannot_request_workspace_repair_or_head_update() {
    let ready = policy_page_with_merge_capabilities(true, true, true);
    for status in [401, 403, 422] {
        for after in [
            policy_page("CONFLICTING", "DIRTY", None, (false, None)),
            policy_page("MERGEABLE", "BEHIND", None, (false, None)),
        ] {
            let root = tempfile::tempdir().assert_value();
            let action = format!(
                "printf '%s\\n' 'HTTP {status}: request rejected (https://api.github.com/graphql)' >&2; exit 1"
            );
            let error = merge_authority(root.path(), &ready, &after, &action)
                .request_merge(&review(), GitHubCredential("test-token"))
                .await
                .assert_error();
            assert!(matches!(error, GitHubAuthorityError::Api(_)));
            assert_eq!(error.api_status(), Some(status));
            assert!(!error.retryable_operation());
            assert_eq!(error.authentication_failed(), status == 401);
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
    let transient = "printf 'error connecting to api.github.com\\n' >&2; exit 1";
    for (after, expected, action) in [
        (
            merged.clone(),
            GitHubMergeRequestOutcome::Accepted,
            "printf 'GraphQL: already merged (mergePullRequest)\\n' >&2; exit 1",
        ),
        (merged, GitHubMergeRequestOutcome::Accepted, transient),
        (
            policy_page("CONFLICTING", "DIRTY", None, (false, None)),
            GitHubMergeRequestOutcome::Conflict,
            transient,
        ),
        (
            policy_page("MERGEABLE", "BEHIND", None, (false, None)),
            GitHubMergeRequestOutcome::HeadUpdateRequired,
            transient,
        ),
    ] {
        let root = tempfile::tempdir().assert_value();
        let outcome = merge_authority(root.path(), &ready, &after, action)
            .request_merge(&review(), GitHubCredential("test-token"))
            .await
            .assert_value();
        assert_eq!(outcome, expected);
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
        let authority = merge_authority(root.path(), &page, &page, "exit 0");
        let observation = authority
            .inspect_review(&review(), GitHubCredential("test-token"))
            .await
            .assert_value();
        assert!(matches!(
            observation.state,
            GitHubReviewState::Open {
                checks: GitHubChecks::NotRequired
            }
        ));
        if queued {
            assert_eq!(
                authority
                    .request_merge(&review(), GitHubCredential("test-token"))
                    .await
                    .assert_value(),
                GitHubMergeRequestOutcome::Accepted
            );
        }
    }
}
