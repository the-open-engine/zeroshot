//! Real GitHub policy reads inside the native delivery driver; Git transport stays local.
use super::*;

const RUN: &str = "policy-read-retry";
const MESSAGE: &str = "Something went wrong while executing your query on 2026-11-21T18:29:14Z. Please include `2790:35FB30:99F13F9:E90DBDA:6B01E2F9` when reporting this issue.";

fn calls(repo: &TempRepo, name: &str) -> usize {
    fs::read_to_string(repo.root.path().join(name))
        .unwrap_or_default()
        .lines()
        .count()
}

fn fixture(
    repo: &TempRepo,
    stage: &str,
    failures: usize,
    poll: DeliveryPollPolicy,
) -> (Arc<FakeGitHub>, Arc<NativeV2DeliveryAdapter>) {
    crate::native_v2_candidate::test_support::commit_all(
        &repo.workspace,
        "fix: reviewed candidate",
    );
    let head = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
    let branch = delivery_head_branch(&RunId::new(RUN));
    fs::write(
        repo.root.path().join("failure.json"),
        json!([{"errors":[{"message":MESSAGE}]}]).to_string(),
    )
    .unwrap();
    fs::write(
        repo.root.path().join("failure.stderr"),
        format!("gh: {MESSAGE}\n"),
    )
    .unwrap();
    let prefix = format!(
        r#"
    case "$*" in
    *'query MergePolicy'*) stage=method ;;
    *) stage=snapshot ;;
    esac
    printf '%s\n' call >> "$HOME/$stage-reads"
    count=$(/usr/bin/wc -l < "$HOME/$stage-reads")
    if [ '{stage}' = reconciliation ] && [ "$stage" = snapshot ] && [ -f "$HOME/reconciling" ]; then
        printf '%s\n' call >> "$HOME/reconciliation-reads"
        /bin/cat "$HOME/failure.json"
        /bin/cat "$HOME/failure.stderr" >&2
        if [ "$(/usr/bin/wc -l < "$HOME/reconciliation-reads")" -ge 3 ]; then /bin/rm "$HOME/reconciling"; fi
        exit 1
    fi
    if [ "$stage" = '{stage}' ] && [ "$count" -le {failures} ]; then
        /bin/cat "$HOME/failure.json"
        /bin/cat "$HOME/failure.stderr" >&2
        exit 1
    fi
    if [ "$stage" = snapshot ] && [ -f "$HOME/merged" ]; then
        /bin/cat "$HOME/merged.json"; exit 0
    fi
"#
    );
    let submission = if stage == "reconciliation" {
        r#"    printf '%s\n' call >> "$HOME/submissions"
    if [ "$(/usr/bin/wc -l < "$HOME/submissions")" -eq 1 ]; then
        /usr/bin/touch "$HOME/reconciling"
        printf 'HTTP 503: unavailable (https://api.github.com/graphql) test-token\n' >&2
        exit 1
    fi
    printf '%s\n' call >> "$HOME/merged"
"#
    } else {
        "    printf '%s\\n' call >> \"$HOME/merged\"\n"
    };
    let source = GH_SCRIPT
        .replace("  graphql:GET)\n", &format!("  graphql:GET)\n{prefix}"))
        .replace("  merge:GET)\n", &format!("  merge:GET)\n{submission}"))
        .replace("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", &head)
        .replace("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &repo.base)
        .replace("zeroshot/v2-test", &branch);
    let source = if stage == "pending" {
        source.replace(
            "\"mergeable\":\"MERGEABLE\",\"mergeStateStatus\":\"CLEAN\"",
            "\"mergeable\":\"UNKNOWN\",\"mergeStateStatus\":\"BLOCKED\"",
        )
    } else {
        source
    };
    let program = write_executable(repo.root.path(), "gh-policy-driver", &source);
    // Get the valid snapshot from the existing native gh fixture, then author a merged observation.
    let output = std::process::Command::new(&program)
        .args(["api", "graphql"])
        .env("HOME", repo.root.path())
        .output()
        .unwrap();
    // The scripted outage may be selected already; read a healthy snapshot through the other stage.
    let output = if output.status.success() {
        output
    } else {
        std::process::Command::new(&program)
            .args(["api", "graphql", "query MergePolicy"])
            .env("HOME", repo.root.path())
            .output()
            .unwrap()
    };
    let mut merged: Value = serde_json::from_slice(&output.stdout).unwrap();
    let pull = &mut merged[0]["data"]["repository"]["pullRequest"];
    pull["state"] = json!("MERGED");
    pull["merged"] = json!(true);
    pull["mergeCommit"] = json!({"oid":"c".repeat(40)});
    fs::write(repo.root.path().join("merged.json"), merged.to_string()).unwrap();
    for counter in ["snapshot-reads", "method-reads"] {
        let _ = fs::remove_file(repo.root.path().join(counter));
    }
    let mut authority = FakeGitHub::new(repo.remote.clone(), Script::NoCi);
    authority.policy_authority = Some(GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
        gh_program: program,
        api_deadline: Duration::from_secs(15),
        ..GhCliAuthorityConfig::hosted(repo.root.path().into())
    }));
    let authority = Arc::new(authority);
    let adapter = retained_adapter(
        repo,
        authority.clone(),
        poll,
        DeliveryLineage::original(RUN),
    );
    (authority, adapter)
}

fn request(repo: &TempRepo) -> DeliveryRunRequest<'_> {
    DeliveryRunRequest {
        repo,
        attempts: 1,
        mode: DeliveryMode::Merge,
        run_id: RUN,
        refresh: None,
    }
}

#[tokio::test]
async fn policy_read_exhaustion_cannot_restart_in_default_or_finite_delivery_polls() {
    for stage in ["snapshot", "method"] {
        for poll in [
            DeliveryPollPolicy::default(),
            DeliveryPollPolicy::new(8, Duration::ZERO).unwrap(),
        ] {
            let repo = TempRepo::delivery();
            let (authority, adapter) = fixture(&repo, stage, 3, poll);
            let started = std::time::Instant::now();
            let execution = tokio::time::timeout(
                Duration::from_secs(15),
                run_with_adapter(request(&repo), adapter),
            )
            .await
            .unwrap();
            assert_eq!(
                execution.outcome,
                WorkerOutcome::declared_failure(WorkerErrorCode::Crash)
            );
            assert!(
                execution
                    .output
                    .iter()
                    .any(|output| output.text.contains("retry allowance exhausted"))
            );
            assert!(
                execution
                    .output
                    .iter()
                    .all(|output| !output.text.contains("test-token"))
            );
            assert!(started.elapsed() >= Duration::from_secs(3));
            // A fresh successful read confirms mandatory withdrawal; the poll must never resubmit.
            assert_eq!(
                calls(&repo, &format!("{stage}-reads")),
                if stage == "snapshot" { 4 } else { 3 }
            );
            assert_eq!(calls(&repo, "merged"), 0);
            assert_eq!(authority.withdrawn_reviews.lock().unwrap().len(), 1);
            let review = &authority.review_requests()[0];
            assert_eq!(
                git_output(&repo.remote, &["rev-parse", &review.head_branch]),
                review.head_revision
            );
        }
    }
}

#[tokio::test]
async fn policy_read_recovery_submits_once_through_the_delivery_driver() {
    for stage in ["snapshot", "method"] {
        let repo = TempRepo::delivery();
        let (_, adapter) = fixture(
            &repo,
            stage,
            2,
            DeliveryPollPolicy::new(8, Duration::ZERO).unwrap(),
        );
        let execution = run_with_adapter(request(&repo), adapter).await;
        assert_delivery_signal(&execution.outcome, DELIVERY_MERGED_LABEL);
        assert_eq!(calls(&repo, "merged"), 1);
        assert_eq!(
            calls(&repo, "method-reads"),
            if stage == "method" { 3 } else { 1 }
        );
    }
}

#[tokio::test]
async fn policy_read_reconciliation_exhaustion_never_restarts_or_resubmits() {
    for poll in [
        DeliveryPollPolicy::default(),
        DeliveryPollPolicy::new(8, Duration::ZERO).unwrap(),
    ] {
        let repo = TempRepo::delivery();
        let (authority, adapter) = fixture(&repo, "reconciliation", 0, poll);
        let execution = tokio::time::timeout(
            Duration::from_secs(15),
            run_with_adapter(request(&repo), adapter),
        )
        .await
        .unwrap();
        assert_eq!(
            calls(&repo, "submissions"),
            1,
            "an exhausted reconciliation must not permit another mutation"
        );
        assert_eq!(calls(&repo, "reconciliation-reads"), 3);
        assert_eq!(
            execution.outcome,
            WorkerOutcome::declared_failure(WorkerErrorCode::Crash)
        );
        let diagnostic = execution
            .output
            .iter()
            .map(|output| output.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(diagnostic.contains("retry allowance exhausted"));
        assert!(diagnostic.contains("HTTP 503"));
        assert!(!diagnostic.contains("test-token"));
        assert_eq!(calls(&repo, "merged"), 0);
        assert_eq!(authority.withdrawn_reviews.lock().unwrap().len(), 1);
        let review = &authority.review_requests()[0];
        assert_eq!(
            git_output(&repo.remote, &["rev-parse", &review.head_branch]),
            review.head_revision
        );
    }
}

#[tokio::test]
async fn policy_read_backoff_observes_native_delivery_cancellation() {
    let repo = TempRepo::delivery();
    let (_, adapter) = fixture(&repo, "snapshot", 1, DeliveryPollPolicy::default());
    cancel_after_reads(&repo, adapter, 1).await;
    assert_eq!(
        calls(&repo, "snapshot-reads"),
        2,
        "one failed read and mandatory withdrawal, no backoff retry"
    );
    assert_eq!(calls(&repo, "merged"), 0);
}

async fn cancel_after_reads(repo: &TempRepo, adapter: Arc<NativeV2DeliveryAdapter>, count: usize) {
    let mut handle = start_with_adapter_token(
        request(repo),
        adapter,
        PullRequestFeedback::Consider,
        Some("test-token"),
    )
    .await;
    let mut events = handle.take_initial_output().unwrap();
    let (completion, ()) = tokio::join!(
        async {
            tokio::time::timeout(Duration::from_secs(30), async {
                while calls(repo, "snapshot-reads") < count {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                handle.cancel();
                handle.completion().await
            })
            .await
            .unwrap()
        },
        async {
            while let Ok(event) = events.recv().await {
                if let crate::native_v2_runner::DurableNodeEvent::DeliveryState(state) = event {
                    events
                        .acknowledge_delivery_state(state.generation.get())
                        .unwrap();
                }
            }
        }
    );
    assert!(matches!(completion, Err(NodeRunnerError::Cancelled)));
}

#[tokio::test]
async fn policy_read_allowance_preserves_ordinary_delivery_polling() {
    let repo = TempRepo::delivery();
    let (_, adapter) = fixture(&repo, "pending", 0, DeliveryPollPolicy::default());
    cancel_after_reads(&repo, adapter, 2).await;
    assert_eq!(calls(&repo, "merged"), 0);
    let repo = TempRepo::delivery();
    let (_, adapter) = fixture(
        &repo,
        "pending",
        0,
        DeliveryPollPolicy::new(5, Duration::ZERO).unwrap(),
    );
    let execution = run_with_adapter(request(&repo), adapter).await;
    assert_eq!(
        execution.outcome,
        WorkerOutcome::declared_failure(WorkerErrorCode::Timeout)
    );
    assert_eq!(
        calls(&repo, "snapshot-reads"),
        6,
        "five healthy polls and mandatory withdrawal"
    );
    assert_eq!(calls(&repo, "merged"), 0);
}
