//! Retain the real GitHub queue authority throughout native delivery polling.
use super::*;
use crate::native_v2_candidate::test_support::commit_all;

const RUN: &str = "queue-conflict-recovery";

fn fixture(repo: &TempRepo, mode: &str) -> (Arc<FakeGitHub>, Arc<NativeV2DeliveryAdapter>) {
    commit_all(&repo.workspace, "fix: reviewed candidate");
    let head = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
    let branch = delivery_head_branch(&RunId::new(RUN));
    let source = GH_SCRIPT
        .replace("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", &head)
        .replace("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &repo.base)
        .replace("zeroshot/v2-test", &branch);
    let program = write_executable(repo.root.path(), "gh-queue-driver", &source);
    let output = std::process::Command::new(&program)
        .args(["api", "graphql"])
        .env_clear()
        .env("HOME", repo.root.path())
        .output()
        .assert_value();
    assert!(output.status.success());
    let mut ready: Value = serde_json::from_slice(&output.stdout).assert_value();
    ready[0]["data"]["repository"]["pullRequest"]["isMergeQueueEnabled"] = json!(true);
    ready[0]["data"]["repository"]["mergeQueue"] = json!({"id":"MQ_MAIN","entries":{
        "totalCount":0,"nodes":[],"queuePageInfo":{"hasNextPage":false,"endCursor":null}
    }});
    let mut predecessor = ready.clone();
    predecessor[0]["data"]["repository"]["mergeQueue"]["entries"]["totalCount"] = json!(1);
    predecessor[0]["data"]["repository"]["mergeQueue"]["entries"]["nodes"] = json!([
        {"id":"ENTRY_A","enqueuedAt":"2026-11-09T00:30:00Z","jump":false,
         "pullRequest":{"id":"PR_A","baseRefName":"main","repository":{"nameWithOwner":"acme/project"}}}
    ]);
    let mut conflict = ready.clone();
    conflict[0]["data"]["repository"]["pullRequest"]["mergeable"] = json!("CONFLICTING");
    conflict[0]["data"]["repository"]["pullRequest"]["mergeStateStatus"] = json!("DIRTY");
    let mut conflicting_predecessor = predecessor.clone();
    conflicting_predecessor[0]["data"]["repository"]["pullRequest"]["mergeable"] =
        json!("CONFLICTING");
    conflicting_predecessor[0]["data"]["repository"]["pullRequest"]["mergeStateStatus"] =
        json!("DIRTY");
    let mut merged = ready.clone();
    merged[0]["data"]["repository"]["pullRequest"]["merged"] = json!(true);
    merged[0]["data"]["repository"]["pullRequest"]["state"] = json!("MERGED");
    merged[0]["data"]["repository"]["pullRequest"]["mergeCommit"] = json!({"oid":"c".repeat(40)});
    for (name, value) in [
        ("ready", ready),
        ("predecessor", predecessor),
        ("conflicting-predecessor", conflicting_predecessor),
        ("conflict", conflict),
        ("merged", merged),
    ] {
        fs::write(
            repo.root.path().join(format!("{name}.json")),
            value.to_string(),
        )
        .assert_value();
    }
    fs::write(repo.root.path().join("enqueue.json"), json!({"data":{"enqueuePullRequest":{"mergeQueueEntry":{
        "id":"ENTRY_B","enqueuedAt":"2026-11-09T01:00:00Z","pullRequest":{
            "id":"PR_node_17","number":17,"baseRefName":"main","headRefName":branch,"headRefOid":head,"repository":{"nameWithOwner":"acme/project"}}
    }}}}).to_string()).assert_value();
    for index in 1..=4 {
        fs::write(repo.root.path().join(format!("removal-{index}.json")), json!([{"data":{"repository":{
            "nameWithOwner":"acme/project","pullRequest":{"id":"PR_node_17","number":17,"baseRefName":"main","headRefName":branch,"headRefOid":head,
                "timelineItems":{"nodes":[{"__typename":"RemovedFromMergeQueueEvent","id":format!("REMOVAL_{index}"),"reason":"merge_conflict","createdAt":"2026-11-09T01:30:00Z","beforeCommit":null}]}}
        }}}]).to_string()).assert_value();
    }
    let mut unavailable: Value = serde_json::from_str(
        &fs::read_to_string(repo.root.path().join("removal-1.json")).assert_value(),
    )
    .assert_value();
    let events = &mut unavailable[0]["data"]["repository"]["pullRequest"]["timelineItems"]["nodes"];
    if mode == "missing" {
        *events = json!([]);
    } else {
        events[0]["createdAt"] = json!("2026-11-09T00:59:59Z");
    }
    fs::write(
        repo.root.path().join("unavailable-removal.json"),
        unavailable.to_string(),
    )
    .assert_value();
    let prefix = format!(
        r#"
    count=0
    if [ -f "$HOME/enqueues" ]; then count=$(/usr/bin/wc -l < "$HOME/enqueues"); fi
    case "$*" in
    *enqueuePullRequest*) printf 'call\n' >> "$HOME/enqueues"; /bin/cat "$HOME/enqueue.json"; exit 0 ;;
    *'query QueueOutcome('*)
        if [ '{mode}' = missing ] || [ '{mode}' = stale_removal ]; then
            reads=$(/usr/bin/wc -l < "$HOME/queue-reads")
            if [ "$reads" -ge 3 ] && [ "$reads" -le 6 ]; then /bin/cat "$HOME/unavailable-removal.json"; exit 0; fi
        fi
        /bin/cat "$HOME/removal-$count.json"; exit 0 ;;
    esac
    if [ "$count" -eq 0 ]; then /bin/cat "$HOME/ready.json"; exit 0; fi
    printf 'call\n' >> "$HOME/queue-reads"
    reads=$(/usr/bin/wc -l < "$HOME/queue-reads")
    if [ '{mode}' = missing ] || [ '{mode}' = stale_removal ]; then
        if [ "$reads" -le 4 ]; then /bin/cat "$HOME/conflicting-predecessor.json"; else /bin/cat "$HOME/conflict.json"; fi
        exit 0
    fi
    if [ "$reads" -le 2 ] || [ '{mode}' = waiting ]; then /bin/cat "$HOME/predecessor.json"; exit 0; fi
    if [ '{mode}' = stale ]; then
        if [ "$reads" -eq 3 ]; then /bin/cat "$HOME/conflict.json"; else /bin/cat "$HOME/merged.json"; fi
        exit 0
    fi
    if [ '{mode}' = conflict ]; then /bin/cat "$HOME/conflict.json"; exit 0; fi
    if [ '{mode}' = withdrawn ] && [ "$count" -ge 2 ]; then /bin/cat "$HOME/merged.json"; exit 0; fi
    /bin/cat "$HOME/ready.json"; exit 0
"#
    );
    let program = write_executable(
        repo.root.path(),
        "gh-queue-driver",
        &source.replace("  graphql:GET)\n", &format!("  graphql:GET)\n{prefix}")),
    );
    let script = if mode == "stale" {
        Script::StaleConflictThenMerges
    } else {
        Script::NoCi
    };
    let mut authority = FakeGitHub::new(repo.remote.clone(), script);
    authority.policy_authority = Some(GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
        gh_program: program,
        api_deadline: Duration::from_secs(10),
        ..GhCliAuthorityConfig::hosted(repo.root.path().into())
    }));
    let authority = Arc::new(authority);
    let poll = DeliveryPollPolicy {
        interval: Duration::ZERO,
        ..DeliveryPollPolicy::default()
    };
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

fn enqueues(repo: &TempRepo) -> usize {
    fs::read_to_string(repo.root.path().join("enqueues"))
        .assert_value()
        .lines()
        .count()
}

#[tokio::test]
async fn queue_predecessor_withdrawal_readmits_and_requires_observed_merge() {
    let repo = TempRepo::delivery();
    let (authority, adapter) = fixture(&repo, "withdrawn");
    let execution = tokio::time::timeout(
        Duration::from_secs(20),
        run_with_adapter(request(&repo), adapter),
    )
    .await
    .assert_value();
    let output = assert_delivery_signal(&execution.outcome, DELIVERY_MERGED_LABEL);
    assert!(output.to_string().contains(&"c".repeat(40)));
    assert_eq!(enqueues(&repo), 2);
    assert_eq!(
        authority.conflict_materializations.load(Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn queue_predecessor_merge_materializes_target_and_review_base() {
    let repo = TempRepo::delivery();
    let (authority, adapter) = fixture(&repo, "conflict");
    let execution = tokio::time::timeout(
        Duration::from_secs(20),
        run_with_adapter(request(&repo), adapter.clone()),
    )
    .await
    .assert_value();
    assert_delivery_signal(&execution.outcome, DELIVERY_CONFLICT_LABEL);
    let target = git_output(&repo.remote, &["rev-parse", "main"]);
    let diagnostic = outcome_diagnostic(&execution.outcome);
    assert!(
        diagnostic.contains(&format!(
            "targetRevision={target}; conflictedPaths=[\"result.txt\"]"
        )),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains(&format!("reviewBaseRevision: {target}")),
        "{diagnostic}"
    );
    assert_eq!(
        git_output(&repo.workspace, &["rev-parse", "MERGE_HEAD"]),
        target
    );
    assert_eq!(enqueues(&repo), 1);
    assert_eq!(
        authority.conflict_materializations.load(Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn queue_conflict_missing_or_stale_removal_defers_materialization() {
    for mode in ["missing", "stale_removal"] {
        let repo = TempRepo::delivery();
        let (authority, adapter) = fixture(&repo, mode);
        let execution = tokio::time::timeout(
            Duration::from_secs(20),
            run_with_adapter(request(&repo), adapter),
        )
        .await
        .assert_value();
        assert_delivery_signal(&execution.outcome, DELIVERY_CONFLICT_LABEL);
        assert!(
            fs::read_to_string(repo.root.path().join("queue-reads"))
                .assert_value()
                .lines()
                .count()
                >= 7,
            "{mode}: materialization requires a current removal after predecessors drain"
        );
        assert_eq!(
            authority.conflict_materializations.load(Ordering::SeqCst),
            1
        );
        assert_eq!(enqueues(&repo), 1);
    }
}

#[tokio::test]
async fn queue_conflict_exhaustion_terminates_default_unlimited_delivery_polling() {
    let repo = TempRepo::delivery();
    let (authority, adapter) = fixture(&repo, "repeated");
    let execution = tokio::time::timeout(
        Duration::from_secs(20),
        run_with_adapter(request(&repo), adapter),
    )
    .await
    .assert_value();
    assert_eq!(
        execution.outcome,
        WorkerOutcome::declared_failure(WorkerErrorCode::Crash)
    );
    assert!(
        execution
            .output
            .iter()
            .any(|event| event.text.contains("recovery allowance exhausted"))
    );
    assert_eq!(enqueues(&repo), 4);
    assert_eq!(authority.withdrawn_reviews.lock().assert_value().len(), 1);
}

#[tokio::test]
async fn queue_conflict_changed_materialization_resumes_forge_observation() {
    let repo = TempRepo::delivery();
    let (authority, adapter) = fixture(&repo, "stale");
    let head = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
    let execution = tokio::time::timeout(
        Duration::from_secs(20),
        run_with_adapter(request(&repo), adapter),
    )
    .await
    .assert_value();
    assert_delivery_signal(&execution.outcome, DELIVERY_MERGED_LABEL);
    assert_eq!(
        authority.conflict_materializations.load(Ordering::SeqCst),
        1
    );
    assert_eq!(git_output(&repo.workspace, &["rev-parse", "HEAD"]), head);
    assert!(!repo.workspace.join(".git/MERGE_HEAD").exists());
    assert_eq!(enqueues(&repo), 1);
}

#[tokio::test]
async fn queue_conflict_waiting_observes_delivery_cancellation() {
    let repo = TempRepo::delivery();
    let (authority, adapter) = fixture(&repo, "waiting");
    let mut handle = start_with_adapter_token(
        request(&repo),
        adapter,
        PullRequestFeedback::Consider,
        Some("test-token"),
    )
    .await;
    let mut events = handle.take_initial_output().assert_value();
    let (completion, ()) = tokio::join!(
        async {
            tokio::time::timeout(Duration::from_secs(20), async {
                while fs::read_to_string(repo.root.path().join("queue-reads"))
                    .unwrap_or_default()
                    .lines()
                    .count()
                    < 3
                {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                handle.cancel();
                handle.completion().await
            })
            .await
            .assert_value()
        },
        async {
            while let Ok(event) = events.recv().await {
                if let crate::native_v2_runner::DurableNodeEvent::DeliveryState(state) = event {
                    events
                        .acknowledge_delivery_state(state.generation.get())
                        .assert_value();
                }
            }
        }
    );
    assert!(matches!(completion, Err(NodeRunnerError::Cancelled)));
    assert_eq!(enqueues(&repo), 1);
    assert_eq!(
        authority.conflict_materializations.load(Ordering::SeqCst),
        0
    );
}
