use super::*;

pub(super) fn enqueue_response() -> Value {
    json!({"data":{"enqueuePullRequest":{"mergeQueueEntry":{
        "id":"QUEUE_17","pullRequest":{
            "id":"PR_node_17","number":17,"repository":{"nameWithOwner":"acme/project"},
            "baseRefName":"main","headRefName":"zeroshot/v2-run","headRefOid":review().head_revision
        }
    }}}})
}

fn queued(in_queue: bool, auto: bool) -> Value {
    let mut page = policy_page_with_merge_capabilities(false, false, false);
    let pull = &mut page["data"]["repository"]["pullRequest"];
    pull["isMergeQueueEnabled"] = json!(true);
    pull["isInMergeQueue"] = json!(in_queue);
    pull["autoMergeRequest"] = if auto {
        json!({"enabledAt":"2026-10-15T00:00:00Z"})
    } else {
        Value::Null
    };
    pull["baseRef"]["rules"] = Value::Null;
    page
}

pub(super) fn queue_authority(
    root: &std::path::Path,
    states: &[Value],
    actions: &[(&str, &str)],
) -> GhCliDeliveryAuthority {
    assert!(!states.is_empty());
    for (index, state) in states.iter().enumerate() {
        std::fs::write(
            root.join(format!("policy-{index}.json")),
            if state.is_array() {
                state.clone()
            } else {
                json!([state])
            }
            .to_string(),
        )
        .assert_value();
    }
    let branches = actions
        .iter()
        .map(|(operation, action)| {
            format!("*'{operation}'*) printf '%s\n' \"$@\" >> \"$HOME/mutations\"; {action} ;;")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let program = root.join("gh-queue-fixture");
    write_executable(
        &program,
        format!(
            r#"#!/bin/sh
set -eu
case "$*" in
{branches}
*)
    index=0
    if [ -f "$HOME/reads" ]; then index=$(/bin/cat "$HOME/reads"); fi
    printf '%s' "$((index + 1))" > "$HOME/reads"
    if [ "$index" -gt 0 ] && [ -f "$HOME/require-completion" ]; then [ -f "$HOME/completed" ] || exit 31; fi
    if [ "$index" -gt {last} ]; then index={last}; fi
    /bin/cat "$HOME/policy-$index.json" ;;
esac
"#,
            last = states.len() - 1
        ),
    );
    authority(program, root)
}

fn admission(root: &std::path::Path, action: &str) -> GhCliDeliveryAuthority {
    queue_authority(
        root,
        &[queued(false, false)],
        &[("enqueuePullRequest", action)],
    )
}

async fn assert_withdrawn(provider: &GhCliDeliveryAuthority) {
    assert_eq!(
        provider
            .withdraw_merge_request(&review(), SourceDeliveryCredential::token("test-token"))
            .await
            .assert_value(),
        SourceMergeWithdrawal::Withdrawn
    );
}

fn response(value: &Value) -> String {
    format!("printf '%s' {}", shell_literal(&value.to_string()))
}

#[tokio::test]
async fn queue_admission_is_explicit_exact_head_and_keeps_direct_policy_out() {
    let root = tempfile::tempdir().assert_value();
    let action = response(&enqueue_response());
    let provider = queue_authority(
        root.path(),
        &[queued(false, false)],
        &[("enqueuePullRequest", &action)],
    );
    assert_eq!(
        provider
            .request_merge(&review(), SourceDeliveryCredential::token("test-token"))
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Accepted
    );
    let args = std::fs::read_to_string(root.path().join("mutations")).assert_value();
    assert!(args.contains("enqueuePullRequest(input:"));
    assert!(args.contains("pullRequestId=PR_node_17"));
    assert!(args.contains(&format!("expectedHeadOid={}", review().head_revision)));
    for forbidden in [
        "enablePullRequestAutoMerge",
        "mergePullRequest(",
        "--admin",
        "--auto",
        "--merge",
        "--squash",
        "--rebase",
        "--match-head-commit",
    ] {
        assert!(
            !args.contains(forbidden),
            "unexpected bypass or auto-merge: {forbidden}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(root.path().join("reads")).assert_value(),
        "1"
    );
}

#[tokio::test]
async fn malformed_queue_receipts_never_become_accepted_requests() {
    let good = enqueue_response();
    let root_path = "/data/enqueuePullRequest/mergeQueueEntry";
    let mut responses = vec![
        Value::Null,
        json!({}),
        json!({"data":{"enqueuePullRequest":null}}),
    ];
    for (path, value) in [
        ("", Value::Null),
        ("/id", json!("")),
        ("/pullRequest/id", json!("other")),
        ("/pullRequest/number", json!(18)),
        ("/pullRequest/repository/nameWithOwner", json!("other/repo")),
        ("/pullRequest/baseRefName", json!("other")),
        ("/pullRequest/headRefName", json!("other")),
        ("/pullRequest/headRefOid", json!("c".repeat(40))),
    ] {
        let mut value_root = good.clone();
        *value_root
            .pointer_mut(&format!("{root_path}{path}"))
            .assert_value() = value;
        responses.push(value_root);
    }
    for reply in responses {
        let root = tempfile::tempdir().assert_value();
        let action = response(&reply);
        let provider = admission(root.path(), &action);
        let error = provider
            .request_merge(&review(), SourceDeliveryCredential::token("test-token"))
            .await
            .assert_error();
        assert!(matches!(error, SourceDeliveryError::Api(_)), "{error}");
        assert!(error.to_string().contains("invalid merge queue"));
    }
}

#[tokio::test]
async fn queue_auth_policy_and_transport_failures_keep_the_native_error_boundary() {
    for (message, status, retryable) in [
        ("HTTP 401: Bad credentials", Some(401), false),
        ("HTTP 403: Resource not accessible", Some(403), false),
        ("HTTP 422: Validation Failed", Some(422), false),
        ("HTTP 503: Unavailable", Some(503), true),
        ("GraphQL: expectedHeadOid does not match", None, false),
    ] {
        let root = tempfile::tempdir().assert_value();
        let action = format!(
            "printf '%s\n' {} \"$GH_TOKEN\" >&2; exit 1",
            shell_literal(message)
        );
        let provider = admission(root.path(), &action);
        let error = provider
            .request_merge(&review(), SourceDeliveryCredential::token("test-token"))
            .await
            .assert_error();
        assert_eq!(error.api_status(), status, "{error}");
        assert_eq!(error.authentication_failed(), status == Some(401));
        assert_eq!(error.retryable_operation(), retryable);
        assert!(!error.to_string().contains("test-token"));
    }
}

#[tokio::test]
async fn withdrawal_disables_legacy_auto_merge_then_dequeues_and_confirms_exact_identity() {
    for disabled in [
        "printf '{}'",
        "printf 'error connecting to api.github.com\\n' >&2; exit 1",
    ] {
        let root = tempfile::tempdir().assert_value();
        let provider = queue_authority(
            root.path(),
            &[
                queued(true, true),
                queued(true, false),
                queued(false, false),
            ],
            &[
                ("disablePullRequestAutoMerge", disabled),
                ("dequeuePullRequest", "printf '{}'"),
            ],
        );
        assert_withdrawn(&provider).await;
        let args = std::fs::read_to_string(root.path().join("mutations")).assert_value();
        assert!(
            args.find("disablePullRequestAutoMerge").assert_value()
                < args.find("dequeuePullRequest").assert_value()
        );
        assert_eq!(args.matches("pullRequestId=PR_node_17").count(), 2);
        assert_eq!(
            std::fs::read_to_string(root.path().join("reads")).assert_value(),
            "3"
        );
    }
}

#[tokio::test]
async fn withdrawal_reconciles_lost_responses_but_refuses_live_or_changed_authority() {
    let mut changed = queued(false, false);
    changed["data"]["repository"]["pullRequest"]["headRefOid"] = json!("c".repeat(40));
    let mut merged = queued(false, false);
    set_review_state(&mut merged, "MERGED", true);
    merged["data"]["repository"]["pullRequest"]["mergeCommit"] = json!({"oid":"c".repeat(40)});
    for (after, expected) in [
        (queued(false, false), Some(SourceMergeWithdrawal::Withdrawn)),
        (
            merged,
            Some(SourceMergeWithdrawal::Merged {
                merge_revision: "c".repeat(40),
            }),
        ),
        (queued(true, false), None),
        (changed, None),
    ] {
        let root = tempfile::tempdir().assert_value();
        let provider = queue_authority(
            root.path(),
            &[queued(true, false), after],
            &[(
                "dequeuePullRequest",
                "printf 'error connecting to api.github.com\n' >&2; exit 1",
            )],
        );
        let result = provider
            .withdraw_merge_request(&review(), SourceDeliveryCredential::token("test-token"))
            .await;
        if let Some(expected) = expected {
            assert_eq!(result.assert_value(), expected);
        } else {
            result.assert_error();
        }
    }
}

#[tokio::test]
async fn unresolved_enqueue_cannot_turn_an_empty_observation_into_safe_withdrawal() {
    let root = tempfile::tempdir().assert_value();
    let provider = queue_authority(
        root.path(),
        &[queued(false, false)],
        &[(
            "enqueuePullRequest",
            "printf 'error connecting to api.github.com\n' >&2; exit 1",
        )],
    );
    provider
        .request_merge(&review(), SourceDeliveryCredential::token("test-token"))
        .await
        .assert_error();
    let error = provider
        .withdraw_merge_request(&review(), SourceDeliveryCredential::token("test-token"))
        .await
        .assert_error();
    assert!(error.to_string().contains("submission remains unconfirmed"));
}

#[tokio::test]
async fn cancelled_enqueue_drains_the_owned_submission_before_withdrawing() {
    let root = tempfile::tempdir().assert_value();
    let action = format!(
        "printf started > \"$HOME/started\"; while [ ! -f \"$HOME/allow-completion\" ]; do /bin/sleep 0.005; done; printf done > \"$HOME/completed\"; {}",
        response(&enqueue_response())
    );
    let provider = Arc::new(queue_authority(
        root.path(),
        &[
            queued(false, false),
            queued(true, false),
            queued(false, false),
        ],
        &[
            ("enqueuePullRequest", &action),
            ("dequeuePullRequest", "printf '{}'"),
        ],
    ));
    let owner = provider.clone();
    let task = tokio::spawn(async move {
        owner
            .request_merge(&review(), SourceDeliveryCredential::token("test-token"))
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !root.path().join("started").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .assert_value();
    std::fs::write(root.path().join("require-completion"), "required").assert_value();
    task.abort();
    task.await.assert_error();
    let owner = provider.clone();
    let mut withdrawal = tokio::spawn(async move {
        owner
            .withdraw_merge_request(&review(), SourceDeliveryCredential::token("test-token"))
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut withdrawal)
            .await
            .is_err(),
        "withdrawal must wait for its outstanding enqueue"
    );
    std::fs::write(root.path().join("allow-completion"), "finish").assert_value();
    assert_eq!(
        withdrawal.await.assert_value().assert_value(),
        SourceMergeWithdrawal::Withdrawn
    );
    let args = std::fs::read_to_string(root.path().join("mutations")).assert_value();
    assert_eq!(args.matches("expectedHeadOid=").count(), 1);
    assert!(args.contains("dequeuePullRequest"));
}

#[tokio::test]
async fn removed_entries_can_be_enqueued_again_and_rejected_auth_can_rotate() {
    let root = tempfile::tempdir().assert_value();
    let action = response(&enqueue_response());
    let provider = queue_authority(
        root.path(),
        &[
            queued(false, false),
            queued(true, false),
            queued(false, false),
        ],
        &[("enqueuePullRequest", &action)],
    );
    for expected in [
        SourceMergeRequestOutcome::Accepted,
        SourceMergeRequestOutcome::Pending,
        SourceMergeRequestOutcome::Accepted,
    ] {
        assert_eq!(
            provider
                .request_merge(&review(), SourceDeliveryCredential::token("test-token"))
                .await
                .assert_value(),
            expected
        );
    }
    let args = std::fs::read_to_string(root.path().join("mutations")).assert_value();
    assert_eq!(args.matches("expectedHeadOid=").count(), 2);

    let root = tempfile::tempdir().assert_value();
    let action = format!(
        "if [ \"$GH_TOKEN\" = test-token ]; then printf 'HTTP 401: Bad credentials\\n' >&2; exit 1; fi; {action}"
    );
    let provider = admission(root.path(), &action);
    assert!(
        provider
            .request_merge(&review(), SourceDeliveryCredential::token("test-token"))
            .await
            .assert_error()
            .authentication_failed()
    );
    assert_eq!(
        provider
            .request_merge(&review(), SourceDeliveryCredential::token("rotated-token"))
            .await
            .assert_value(),
        SourceMergeRequestOutcome::Accepted
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("mutations"))
            .assert_value()
            .matches("expectedHeadOid=")
            .count(),
        2
    );
}

#[tokio::test]
async fn changing_auto_merge_across_policy_pages_cannot_authorize_withdrawal() {
    let mut first = queued(false, false);
    first["data"]["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"] =
        json!({"contexts":{"nodes":[],"pageInfo":{"hasNextPage":true,"endCursor":"cursor"}}});
    let mut second = first.clone();
    second["data"]["repository"]["pullRequest"]["autoMergeRequest"] =
        json!({"enabledAt":"2026-10-15T00:00:00Z"});
    second["data"]["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"]
        ["contexts"]["pageInfo"] = json!({"hasNextPage":false,"endCursor":null});
    let root = tempfile::tempdir().assert_value();
    let provider = queue_authority(root.path(), &[json!([first, second])], &[]);
    provider
        .withdraw_merge_request(&review(), SourceDeliveryCredential::token("test-token"))
        .await
        .assert_error();
    assert!(!root.path().join("mutations").exists());
}

#[tokio::test]
async fn completed_transient_enqueues_retry_without_discarding_withdrawal_uncertainty() {
    for (message, accepted) in [
        ("HTTP 503: Unavailable", true),
        ("error connecting to api.github.com", true),
        ("error connecting to api.github.com", false),
    ] {
        let root = tempfile::tempdir().assert_value();
        let recovered = if accepted {
            response(&enqueue_response())
        } else {
            "printf 'HTTP 403: Resource not accessible\\n' >&2; exit 1".to_owned()
        };
        let action = format!(
            "if [ ! -f \"$HOME/attempted\" ]; then printf seen > \"$HOME/attempted\"; printf '%s\\n' {} >&2; exit 1; fi; {recovered}",
            shell_literal(message)
        );
        let states = if accepted {
            vec![
                queued(false, false),
                queued(false, false),
                queued(false, false),
                queued(false, false),
                queued(true, false),
                queued(false, false),
            ]
        } else {
            vec![queued(false, false)]
        };
        let provider = queue_authority(
            root.path(),
            &states,
            &[
                ("enqueuePullRequest", &action),
                ("dequeuePullRequest", "printf '{}'"),
            ],
        );
        assert!(
            provider
                .request_merge(&review(), SourceDeliveryCredential::token("test-token"))
                .await
                .assert_error()
                .retryable_operation()
        );
        let retried = provider
            .request_merge(&review(), SourceDeliveryCredential::token("test-token"))
            .await;
        if accepted {
            assert_eq!(retried.assert_value(), SourceMergeRequestOutcome::Accepted);
        } else {
            assert_eq!(retried.assert_error().api_status(), Some(403));
        }
        let error = provider
            .withdraw_merge_request(&review(), SourceDeliveryCredential::token("test-token"))
            .await
            .assert_error();
        assert!(error.to_string().contains("submission remains unconfirmed"));
        if accepted {
            assert_withdrawn(&provider).await;
            assert_eq!(
                std::fs::read_to_string(root.path().join("reads")).assert_value(),
                "6"
            );
        }
        let args = std::fs::read_to_string(root.path().join("mutations")).assert_value();
        assert_eq!(args.matches("expectedHeadOid=").count(), 2);
        assert_eq!(args.contains("dequeuePullRequest"), accepted);
    }
}

#[tokio::test]
async fn malformed_or_truncated_queue_output_is_redacted_and_never_resubmitted() {
    for (action, diagnostic) in [
        ("printf 'not-json test-token'", "invalid JSON"),
        (
            "/bin/dd if=/dev/zero bs=65536 count=32 2>/dev/null",
            "byte bound",
        ),
    ] {
        let root = tempfile::tempdir().assert_value();
        let provider = admission(root.path(), action);
        for _ in 0..2 {
            let error = provider
                .request_merge(&review(), SourceDeliveryCredential::token("test-token"))
                .await
                .assert_error();
            assert!(error.to_string().contains(diagnostic), "{error}");
            assert!(!error.to_string().contains("test-token"));
            assert!(!error.retryable_operation());
        }
        let args = std::fs::read_to_string(root.path().join("mutations")).assert_value();
        assert_eq!(args.matches("expectedHeadOid=").count(), 1);
        provider
            .withdraw_merge_request(&review(), SourceDeliveryCredential::token("test-token"))
            .await
            .assert_error();
    }
}

#[tokio::test]
async fn rejected_dequeue_keeps_the_live_candidate_under_delivery_authority() {
    for status in [401, 403] {
        let root = tempfile::tempdir().assert_value();
        let refusal = format!("printf 'HTTP {status}: queue withdrawal refused\\n' >&2; exit 1");
        let provider = queue_authority(
            root.path(),
            &[queued(true, false)],
            &[("dequeuePullRequest", &refusal)],
        );
        let error = provider
            .withdraw_merge_request(&review(), SourceDeliveryCredential::token("test-token"))
            .await
            .assert_error();
        assert_eq!(
            (
                error.api_status(),
                error.authentication_failed(),
                error.retryable_operation()
            ),
            (Some(status), status == 401, false)
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("reads")).assert_value(),
            "2"
        );
    }
}
