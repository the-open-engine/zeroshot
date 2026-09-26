use super::*;

fn base_ref(page: &mut Value) -> &mut Value {
    page.pointer_mut("/data/repository/pullRequest/baseRef")
        .assert_value()
}

fn set_rules(page: &mut Value, rules: Vec<Value>) {
    base_ref(page)["rules"] = json!({
        "totalCount": rules.len(),
        "nodes": rules,
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

#[test]
fn every_repository_method_combination_respects_each_linear_history_source() {
    for mask in 0..8 {
        for source in ["none", "classic", "viewer", "ruleset"] {
            let (merge, squash, rebase) = (mask & 1 != 0, mask & 2 != 0, mask & 4 != 0);
            let mut page = policy_page_with_merge_capabilities(merge, squash, rebase);
            match source {
                "classic" => {
                    base_ref(&mut page)["branchProtectionRule"] =
                        json!({"requiresLinearHistory": true});
                }
                "viewer" => {
                    base_ref(&mut page)["refUpdateRule"]["requiresLinearHistory"] = json!(true)
                }
                "ruleset" => set_rules(&mut page, vec![json!({"type": "REQUIRED_LINEAR_HISTORY"})]),
                _ => {}
            }
            let expected = [
                (merge && source == "none", MergeMethod::Merge),
                (squash, MergeMethod::Squash),
                (rebase, MergeMethod::Rebase),
            ]
            .into_iter()
            .find_map(|(enabled, method)| enabled.then_some(method));
            let result = classify_policy(json!([page]), &review());
            if let Some(method) = expected {
                assert_eq!(
                    result.assert_value().merge_method,
                    Some(method),
                    "{mask} {source}"
                );
            } else {
                let error = result.assert_error();
                assert_policy_error(&error);
                assert!(error.to_string().contains("No merge method is allowed"));
            }
        }
    }
}

#[test]
fn all_applicable_method_restrictions_must_allow_the_selected_method() {
    let cases = [
        (
            vec![method_rule(json!(["REBASE"]))],
            Some(MergeMethod::Rebase),
        ),
        (
            vec![method_rule(json!(["SQUASH", "REBASE"]))],
            Some(MergeMethod::Squash),
        ),
        (
            vec![
                method_rule(json!(["MERGE", "SQUASH"])),
                method_rule(json!(["SQUASH", "REBASE"])),
            ],
            Some(MergeMethod::Squash),
        ),
        (
            vec![
                method_rule(json!(["MERGE"])),
                method_rule(json!(["REBASE"])),
            ],
            None,
        ),
        (vec![method_rule(json!([]))], None),
        (vec![method_rule(json!(["FUTURE_METHOD"]))], None),
        (
            vec![method_rule(json!(["FUTURE_METHOD", "REBASE"]))],
            Some(MergeMethod::Rebase),
        ),
        (
            vec![
                method_rule(Value::Null),
                json!({"type": "FUTURE_UNRELATED_RULE"}),
            ],
            Some(MergeMethod::Merge),
        ),
    ];
    for (rules, expected) in cases {
        let mut page = policy_page_with_merge_capabilities(true, true, true);
        set_rules(&mut page, rules);
        let result = classify_policy(json!([page]), &review());
        if let Some(method) = expected {
            assert_eq!(result.assert_value().merge_method, Some(method));
        } else {
            assert_policy_error(&result.assert_error());
        }
    }
}

#[test]
fn rules_cannot_enable_a_repository_disabled_or_non_linear_method() {
    for mut page in [
        policy_page_with_merge_capabilities(true, false, true),
        policy_page_with_merge_capabilities(true, true, true),
    ] {
        let squash = page["data"]["repository"]["squashMergeAllowed"] == true;
        base_ref(&mut page)["branchProtectionRule"] = json!({"requiresLinearHistory": true});
        set_rules(
            &mut page,
            vec![method_rule(if squash {
                json!(["MERGE"])
            } else {
                json!(["SQUASH"])
            })],
        );
        page["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("BLOCKED");
        assert_policy_error(&classify_policy(json!([page]), &review()).assert_error());
    }
}

#[test]
fn incomplete_or_malformed_rules_stop_delivery_instead_of_guessing() {
    for rules in [
        Value::Null,
        json!({"totalCount": 101, "nodes": []}),
        json!({"totalCount": 1, "nodes": []}),
        json!({"totalCount": 1, "nodes": [null]}),
        json!({"totalCount": 1, "pageInfo": {"hasNextPage": false},
            "nodes": [{"type": "PULL_REQUEST", "parameters": null}]}),
        json!({"totalCount": 1, "pageInfo": {"hasNextPage": false},
            "nodes": [{"type": "PULL_REQUEST", "parameters": {}}]}),
    ] {
        let mut page = policy_page_with_merge_capabilities(true, true, true);
        base_ref(&mut page)["rules"] = rules;
        assert_policy_error(&classify_policy(json!([page]), &review()).assert_error());
    }
}

#[test]
fn merge_queue_owns_method_selection_even_without_a_direct_method() {
    let mut page = policy_page_with_merge_capabilities(false, false, false);
    base_ref(&mut page)["branchProtectionRule"] = json!({"requiresLinearHistory": true});
    base_ref(&mut page)["rules"] = Value::Null;
    page["data"]["repository"]["pullRequest"]["isMergeQueueEnabled"] = json!(true);
    assert_eq!(classify(page).merge_method, Some(MergeMethod::Queue));
}

#[test]
fn branch_policy_changes_during_check_pagination_invalidate_the_snapshot() {
    let first = policy_page("MERGEABLE", "CLEAN", Some(vec![]), (true, Some("first")));
    let second = policy_page("MERGEABLE", "CLEAN", Some(vec![]), (false, Some("last")));
    for field in ["branchProtectionRule", "rules"] {
        let mut changed = second.clone();
        if field == "rules" {
            set_rules(&mut changed, vec![method_rule(json!(["REBASE"]))]);
        } else {
            base_ref(&mut changed)[field] = json!({"requiresLinearHistory": true});
        }
        assert_policy_pages_pending(vec![first.clone(), changed]);
    }
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
async fn queued_and_merged_reviews_do_not_submit_another_merge() {
    let mut queued = policy_page_with_merge_capabilities(false, false, false);
    queued["data"]["repository"]["pullRequest"]["isMergeQueueEnabled"] = json!(true);
    queued["data"]["repository"]["pullRequest"]["isInMergeQueue"] = json!(true);
    let mut merged = queued.clone();
    set_review_state(&mut merged, "MERGED", true);
    merged["data"]["repository"]["pullRequest"]["mergeCommit"] =
        json!({"oid": "cccccccccccccccccccccccccccccccccccccccc"});
    base_ref(&mut merged)["rules"] = Value::Null;
    for (page, expected) in [
        (queued, GitHubMergeRequestOutcome::Pending),
        (merged, GitHubMergeRequestOutcome::Accepted),
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
    ] {
        for rejection in [
            "GraphQL: Merge commits are not allowed on this repository. (mergePullRequest)",
            "gh: Resource not accessible (HTTP 403)",
            "gh: Validation Failed (HTTP 422)",
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
        let action = format!("printf '%s\\n' 'gh: request failed (HTTP {status})' >&2; exit 1");
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
            let action =
                format!("printf '%s\\n' 'gh: request rejected (HTTP {status})' >&2; exit 1");
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
    for (after, expected) in [
        (merged, GitHubMergeRequestOutcome::Accepted),
        (
            policy_page("CONFLICTING", "DIRTY", None, (false, None)),
            GitHubMergeRequestOutcome::Conflict,
        ),
        (
            policy_page("MERGEABLE", "BEHIND", None, (false, None)),
            GitHubMergeRequestOutcome::HeadUpdateRequired,
        ),
    ] {
        let root = tempfile::tempdir().assert_value();
        let outcome = merge_authority(root.path(), &ready, &after, "exit 1")
            .request_merge(&review(), GitHubCredential("test-token"))
            .await
            .assert_value();
        assert_eq!(outcome, expected);
    }
}
