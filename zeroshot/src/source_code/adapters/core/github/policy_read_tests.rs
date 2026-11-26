use super::*;
use crate::source_code_provider::TestReviewCheckpoint;
use std::path::Path;

const EXECUTION_ERROR: &str = "Something went wrong while executing your query on 2026-11-21T18:29:14Z. Please include `2790:35FB30:99F13F9:E90DBDA:6B01E2F9` when reporting this issue.";

fn read_count(root: &Path, name: &str) -> usize {
    std::fs::read_to_string(root.join(name))
        .unwrap_or_default()
        .lines()
        .count()
}

fn fixture(root: &Path, stage: &str, failures: usize) -> GhCliDeliveryAuthority {
    let ready = policy_page_with_merge_capabilities(true, true, true);
    std::fs::write(root.join("ready.json"), json!([ready]).to_string()).unwrap();
    std::fs::write(
        root.join("failure.json"),
        json!([{"errors":[{"message":EXECUTION_ERROR}]}]).to_string(),
    )
    .unwrap();
    std::fs::write(
        root.join("failure.stderr"),
        format!("gh: {EXECUTION_ERROR}\n"),
    )
    .unwrap();
    std::fs::write(
        root.join("enqueue.json"),
        super::queue_tests::enqueue_response().to_string(),
    )
    .unwrap();
    std::fs::write(root.join("queue-outcome.json"), json!([{"data":{"repository":{
        "nameWithOwner":"acme/project","pullRequest":{"id":"PR_node_17","number":17,
            "baseRefName":"main","headRefName":review().head_branch,"headRefOid":review().head_revision,
            "timelineItems":{"nodes":[]}}
    }}}]).to_string()).unwrap();
    let mut receipt = super::queue_tests::enqueue_response()["data"]["enqueuePullRequest"]["mergeQueueEntry"]["pullRequest"].clone();
    receipt["merged"] = json!(true);
    receipt["mergeCommit"] = json!({"oid": "c".repeat(40)});
    std::fs::write(
        root.join("merged.json"),
        json!({"data":{"mergePullRequest":{"pullRequest":receipt}}}).to_string(),
    )
    .unwrap();
    let program = root.join("gh-policy-read");
    write_executable(
        &program,
        format!(
            r#"#!/bin/sh
set -eu
case "$2" in
graphql)
    case "$*" in
    *'query QueueOutcome('*) /bin/cat "$HOME/queue-outcome.json"; exit 0 ;;
    *mergePullRequest*) printf '%s\n' merge >> "$HOME/mutations"; /bin/cat "$HOME/merged.json"; exit 0 ;;
    *enqueuePullRequest*) printf '%s\n' call >> "$HOME/mutations"; printf '%s\n' "$@" >> "$HOME/enqueue-args"; /bin/cat "$HOME/enqueue.json"; exit 0 ;;
    *'query MergePolicy'*) stage=method ;;
    *'query($owner:'*) stage=snapshot ;;
    *) printf '%s\n' "$@" >> "$HOME/mutations"; exit 19 ;;
    esac
    printf '%s\n' call >> "$HOME/$stage-reads"
    printf '%s\n' "$@" >> "$HOME/read-args"
    count=$(/usr/bin/wc -l < "$HOME/$stage-reads")
    if [ "$stage" = '{stage}' ] && [ "$count" -le {failures} ]; then
        /bin/cat "$HOME/failure.json"
        /bin/cat "$HOME/failure.stderr" >&2
        if [ -f "$HOME/after-failure" ]; then /bin/sh "$HOME/after-failure"; fi
        exit 1
    fi
    /bin/cat "$HOME/ready.json" ;;
'repos/acme/project/rules/branches/main?per_page=100') printf '[[]]\n' ;;
*) exit 19 ;;
esac
"#
        ),
    );
    authority(program, root)
}

#[tokio::test]
async fn policy_reads_recover_native_execution_errors_before_merge() {
    for stage in ["snapshot", "method"] {
        let root = tempfile::tempdir().unwrap();
        let provider = fixture(root.path(), stage, 2);
        let started = std::time::Instant::now();
        let expected_review = review();
        let observe_reads = async {
            let mut times = Vec::new();
            while times.len() < 3 {
                if read_count(root.path(), &format!("{stage}-reads")) > times.len() {
                    times.push(std::time::Instant::now());
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            times
        };
        let (result, times) = tokio::time::timeout(Duration::from_secs(15), async {
            tokio::join!(
                provider.request_merge(
                    &expected_review,
                    SourceDeliveryCredential::token("test-token"),
                    &TestReviewCheckpoint
                ),
                observe_reads
            )
        })
        .await
        .unwrap();
        assert_eq!(result.unwrap(), SourceMergeRequestOutcome::Accepted);
        assert!(started.elapsed() >= Duration::from_secs(3));
        assert!(times[1].duration_since(times[0]) >= Duration::from_millis(950));
        assert!(times[2].duration_since(times[1]) >= Duration::from_millis(1950));
        assert_eq!(read_count(root.path(), &format!("{stage}-reads")), 3);
        let mutations = std::fs::read_to_string(root.path().join("mutations")).unwrap();
        assert_eq!(mutations.lines().filter(|line| *line == "merge").count(), 1);
        let arguments = std::fs::read_to_string(root.path().join("read-args")).unwrap();
        assert!(!arguments.contains("endCursor="));
        assert_eq!(
            arguments
                .lines()
                .filter(|line| *line == "--paginate")
                .count(),
            4
        );
    }
}

#[tokio::test]
async fn policy_read_exhaustion_is_terminal_and_redacted_at_both_gates() {
    for stage in ["snapshot", "method"] {
        for token in [
            "test-token",
            "errors",
            "Something went wrong",
            "2790:35FB30",
        ] {
            let root = tempfile::tempdir().unwrap();
            let provider = fixture(root.path(), stage, 99);
            let error = provider
                .request_merge(
                    &review(),
                    SourceDeliveryCredential::token(token),
                    &TestReviewCheckpoint,
                )
                .await
                .unwrap_err();
            assert_eq!(error.api_status(), None);
            assert!(!error.retryable_operation());
            assert!(!error.retryable_review_sync());
            let error = error.with_context("merge preflight");
            assert!(!error.retryable_operation());
            let diagnostic = format!("{error:?}\n{error}");
            assert!(diagnostic.contains("retry allowance exhausted"));
            assert!(!diagnostic.contains(token));
            assert!(diagnostic.len() < 32 * 1024);
            assert_eq!(read_count(root.path(), &format!("{stage}-reads")), 3);
            assert_eq!(read_count(root.path(), "mutations"), 0);
        }
    }
}

#[tokio::test]
async fn policy_read_restarts_after_a_failed_later_page_and_revalidates_all_pages() {
    for stage in ["snapshot", "method"] {
        let root = tempfile::tempdir().unwrap();
        let provider = fixture(root.path(), stage, 1);
        let mut first = policy_page(
            "MERGEABLE",
            "CLEAN",
            Some(vec![check_run(
                "required",
                "COMPLETED",
                Some("SUCCESS"),
                true,
            )]),
            (true, Some("page1")),
        );
        first["data"]["repository"]["pullRequest"]["baseRef"]["rules"]["pageInfo"] =
            json!({"hasNextPage":true,"endCursor":"page1"});
        let partial = json!([first.clone(), {"errors":[{"message":EXECUTION_ERROR}]}]);
        std::fs::write(root.path().join("failure.json"), partial.to_string()).unwrap();
        let mut second = first.clone();
        if stage == "method" {
            first["data"]["repository"]["pullRequest"]["baseRef"]["rules"]["totalCount"] = json!(1);
            first["data"]["repository"]["pullRequest"]["baseRef"]["rules"]["nodes"] =
                json!([{"id":"RULE_1","type":"NON_FAST_FORWARD"}]);
            second["data"]["repository"]["pullRequest"]["baseRef"]["rules"]["totalCount"] =
                json!(1);
        }
        second["data"]["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"]
            ["contexts"]["pageInfo"] = json!({"hasNextPage":false,"endCursor":"page2"});
        second["data"]["repository"]["pullRequest"]["baseRef"]["rules"]["pageInfo"] =
            json!({"hasNextPage":false,"endCursor":"page2"});
        // Each query owns only its own paginator.
        if stage == "snapshot" {
            first["data"]["repository"]["pullRequest"]["baseRef"]["rules"]["pageInfo"] =
                json!({"hasNextPage":false,"endCursor":null});
            second["data"]["repository"]["pullRequest"]["baseRef"]["rules"]["pageInfo"] =
                json!({"hasNextPage":false,"endCursor":null});
        }
        std::fs::write(
            root.path().join("ready.json"),
            json!([first, second]).to_string(),
        )
        .unwrap();
        // Observe only the corresponding read, avoiding the other query's independent paginator.
        if stage == "snapshot" {
            assert!(matches!(
                provider
                    .inspect_review(&review(), SourceDeliveryCredential::token("test-token"))
                    .await
                    .unwrap()
                    .state,
                SourceReviewState::Open {
                    checks: SourceChecks::Passed
                }
            ));
        } else {
            assert_eq!(
                provider
                    .merge_method(
                        &review(),
                        SourceDeliveryCredential::token("test-token"),
                        false
                    )
                    .await
                    .unwrap(),
                MergeMethod::Merge
            );
        }
        assert_eq!(read_count(root.path(), &format!("{stage}-reads")), 2);
        assert_eq!(read_count(root.path(), "mutations"), 0);
        assert!(
            !std::fs::read_to_string(root.path().join("read-args"))
                .unwrap()
                .contains("endCursor=")
        );
    }
}

#[tokio::test]
async fn recovered_policy_reads_never_authorize_changed_heads_or_blocking_policy() {
    for stage in ["snapshot", "method"] {
        for change in ["head", "blocking", "incomplete"] {
            let root = tempfile::tempdir().unwrap();
            let provider = fixture(root.path(), stage, 1);
            let mut page = policy_page_with_merge_capabilities(true, true, true);
            match change {
                "head" => {
                    page["data"]["repository"]["pullRequest"]["headRefOid"] = json!("d".repeat(40))
                }
                "blocking" if stage == "snapshot" => {
                    page["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("BLOCKED")
                }
                "blocking" => page["data"]["repository"]["mergeCommitAllowed"] = json!(false),
                _ => {}
            }
            if stage == "method" && change == "blocking" {
                page["data"]["repository"]["squashMergeAllowed"] = json!(false);
                page["data"]["repository"]["rebaseMergeAllowed"] = json!(false);
            }
            if change == "incomplete" {
                page = policy_page(
                    "MERGEABLE",
                    "CLEAN",
                    Some(vec![]),
                    (true, Some("missing-page")),
                );
                page["data"]["repository"]["pullRequest"]["baseRef"]["rules"]["pageInfo"]["hasNextPage"] =
                    json!(true);
            }
            // Replace the response only after the failed read so method failures still pass snapshot preflight.
            std::fs::write(root.path().join("changed.json"), json!([page]).to_string()).unwrap();
            std::fs::write(
                root.path().join("after-failure"),
                "/bin/cp \"$HOME/changed.json\" \"$HOME/ready.json\"\n",
            )
            .unwrap();
            let result = provider
                .request_merge(
                    &review(),
                    SourceDeliveryCredential::token("test-token"),
                    &TestReviewCheckpoint,
                )
                .await;
            if stage == "snapshot" && change == "blocking" {
                assert_eq!(result.unwrap(), SourceMergeRequestOutcome::Pending);
            } else {
                assert!(result.is_err());
            }
            assert_eq!(read_count(root.path(), "mutations"), 0);
            let reads = read_count(root.path(), &format!("{stage}-reads"));
            if change == "incomplete" {
                assert!(reads >= 2);
            } else {
                assert_eq!(reads, 2);
            }
        }
    }
}

#[tokio::test]
async fn policy_reads_leave_unknown_permanent_authentication_and_rate_limits_unchanged() {
    for stage in ["snapshot", "method"] {
        for (message, stderr, status, retryable) in [
            ("unknown failure", "gh: unknown failure", None, false),
            (
                "Unknown schema field",
                "gh: Unknown schema field",
                None,
                false,
            ),
            (
                "Bad credentials",
                "gh: Bad credentials (HTTP 401)",
                Some(401),
                false,
            ),
            (
                "Resource not accessible by integration",
                "gh: Resource not accessible by integration (HTTP 403)",
                Some(403),
                false,
            ),
            (
                "Validation failed",
                "gh: Validation failed (HTTP 422)",
                Some(422),
                false,
            ),
            (
                "API rate limit exceeded",
                "gh: API rate limit exceeded (HTTP 403)",
                Some(403),
                true,
            ),
            (
                "Too Many Requests",
                "gh: Too Many Requests (HTTP 429)",
                Some(429),
                true,
            ),
            (
                "Service Unavailable",
                "gh: Service Unavailable (HTTP 503)",
                Some(503),
                true,
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let provider = fixture(root.path(), stage, 99);
            std::fs::write(
                root.path().join("failure.json"),
                json!([{"errors":[{"message":message}]}]).to_string(),
            )
            .unwrap();
            std::fs::write(root.path().join("failure.stderr"), format!("{stderr}\n")).unwrap();
            let error = provider
                .request_merge(
                    &review(),
                    SourceDeliveryCredential::token("test-token"),
                    &TestReviewCheckpoint,
                )
                .await
                .unwrap_err();
            assert_eq!(error.api_status(), status);
            assert_eq!(error.retryable_operation(), retryable);
            assert_eq!(error.authentication_failed(), status == Some(401));
            assert_eq!(read_count(root.path(), &format!("{stage}-reads")), 1);
            assert_eq!(read_count(root.path(), "mutations"), 0);
        }
    }
}

#[tokio::test]
async fn policy_read_retry_waits_share_the_original_api_deadline() {
    for stage in ["snapshot", "method"] {
        let root = tempfile::tempdir().unwrap();
        let mut provider = fixture(root.path(), stage, 99);
        provider.config.api_deadline = Duration::from_secs(2);
        let started = std::time::Instant::now();
        let error = provider
            .request_merge(
                &review(),
                SourceDeliveryCredential::token("test-token"),
                &TestReviewCheckpoint,
            )
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("retry allowance exhausted at the API deadline")
        );
        assert!(!error.retryable_operation());
        assert_eq!(error.api_status(), None);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!((1..=2).contains(&read_count(root.path(), &format!("{stage}-reads"))));
        assert_eq!(read_count(root.path(), "mutations"), 0);
    }
}

#[tokio::test]
async fn failed_policy_read_never_authorizes_a_head_update() {
    let root = tempfile::tempdir().unwrap();
    let provider = fixture(root.path(), "snapshot", 99);
    provider
        .update_review_head(
            root.path(),
            &review(),
            SourceDeliveryCredential::token("test-token"),
        )
        .await
        .unwrap_err();
    assert_eq!(read_count(root.path(), "snapshot-reads"), 3);
    assert_eq!(read_count(root.path(), "mutations"), 0);
}

#[tokio::test]
async fn policy_read_recovery_preserves_exact_head_queue_admission() {
    for outcome in ["ready", "head", "blocked", "exhausted"] {
        let root = tempfile::tempdir().unwrap();
        let provider = fixture(
            root.path(),
            "snapshot",
            if outcome == "exhausted" { 99 } else { 1 },
        );
        let mut page = policy_page_with_merge_capabilities(false, false, false);
        page["data"]["repository"]["pullRequest"]["isMergeQueueEnabled"] = json!(true);
        if outcome == "head" {
            page["data"]["repository"]["pullRequest"]["headRefOid"] = json!("d".repeat(40));
        } else if outcome == "blocked" {
            page["data"]["repository"]["pullRequest"]["isDraft"] = json!(true);
        }
        std::fs::write(root.path().join("ready.json"), json!([page]).to_string()).unwrap();
        let result = provider
            .request_merge(
                &review(),
                SourceDeliveryCredential::token("test-token"),
                &TestReviewCheckpoint,
            )
            .await;
        match outcome {
            "ready" => {
                assert_eq!(result.unwrap(), SourceMergeRequestOutcome::Accepted);
                assert_eq!(
                    provider
                        .request_merge(
                            &review(),
                            SourceDeliveryCredential::token("test-token"),
                            &TestReviewCheckpoint
                        )
                        .await
                        .unwrap(),
                    SourceMergeRequestOutcome::Pending
                );
            }
            "blocked" => assert_eq!(result.unwrap(), SourceMergeRequestOutcome::Pending),
            _ => {
                assert!(result.is_err());
            }
        }
        assert_eq!(
            read_count(root.path(), "mutations"),
            usize::from(outcome == "ready")
        );
        if outcome == "ready" {
            assert!(
                std::fs::read_to_string(root.path().join("enqueue-args"))
                    .unwrap()
                    .contains(&format!("expectedHeadOid={}", review().head_revision))
            );
        }
        assert_eq!(read_count(root.path(), "method-reads"), 0);
    }
}

#[tokio::test]
async fn healthy_pending_ci_and_queue_reads_do_not_consume_a_retry_allowance() {
    for queued in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let provider = fixture(root.path(), "snapshot", 0);
        let mut page = policy_page("UNKNOWN", "BLOCKED", None, (false, None));
        page["data"]["repository"]["pullRequest"]["isMergeQueueEnabled"] = json!(queued);
        page["data"]["repository"]["pullRequest"]["isInMergeQueue"] = json!(queued);
        std::fs::write(root.path().join("ready.json"), json!([page]).to_string()).unwrap();
        for _ in 0..5 {
            assert_eq!(
                provider
                    .request_merge(
                        &review(),
                        SourceDeliveryCredential::token("test-token"),
                        &TestReviewCheckpoint
                    )
                    .await
                    .unwrap(),
                SourceMergeRequestOutcome::Pending
            );
        }
        assert_eq!(read_count(root.path(), "snapshot-reads"), 5);
        assert_eq!(read_count(root.path(), "mutations"), 0);
    }
}
