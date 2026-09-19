use super::*;
use crate::native_v2_candidate::test_support::{commit_all, path_text};

#[tokio::test]
async fn legacy_contract_stops_on_target_drift_without_publishing() {
    for mode in [
        DeliveryMode::PullRequest,
        DeliveryMode::MergeV1,
        DeliveryMode::Merge,
    ] {
        assert_legacy_delivery_stops_on_target_drift(mode).await;
    }
}

async fn assert_legacy_delivery_stops_on_target_drift(mode: DeliveryMode) {
    let repo = TempRepo::delivery();
    let upstream = repo.root.child("upstream");
    git(
        repo.root.path(),
        &["clone", path_text(&repo.remote), path_text(&upstream)],
    );
    fs::write(upstream.join("upstream.txt"), "preserve upstream work\n").assert_value();
    commit_all(&upstream, "advance target during worker execution");
    git(&upstream, &["push", "origin", "main"]);
    let captured = git_output(&upstream, &["rev-parse", "HEAD"]);
    let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), Script::NoCi));
    let adapter = retained_adapter(
        &repo,
        authority.clone(),
        DeliveryPollPolicy::new(3, Duration::ZERO).assert_value(),
    );
    let admitted = admitted_legacy_delivery(&repo, mode).await;
    let runner =
        Arc::new(NativeNodeRunner::new(&admitted, adapter.clone(), adapter).assert_value());
    let (run_id, ledger, environments) = routing::create_delivery_run(admitted).await;

    let terminal = NativeV2Supervisor::new(run_id.clone(), ledger.clone(), runner, environments)
        .drive()
        .await
        .assert_value();

    assert!(
        !authority.pushed.load(Ordering::SeqCst),
        "a contract without a repair route must not publish the integrated candidate"
    );
    assert!(matches!(terminal, TerminalResult::Failed { .. }));
    assert!(authority.review_requests().is_empty());
    assert_eq!(authority.target_reconciliations.load(Ordering::SeqCst), 1);
    let stored = ledger.get(&run_id).await.assert_value().assert_value();
    assert_eq!(stored.snapshot.executions.len(), 1);
    let execution = stored.snapshot.executions.values().next().assert_value();
    assert!(matches!(
        execution.outcome(),
        Some(WorkerOutcome::Error {
            code: WorkerErrorCode::Crash,
            ..
        })
    ));
    for revision in [&repo.base, &captured] {
        git(
            &repo.workspace,
            &["merge-base", "--is-ancestor", revision, "HEAD"],
        );
    }
    assert_eq!(
        fs::read_to_string(repo.workspace.join("upstream.txt")).assert_value(),
        "preserve upstream work\n"
    );
    assert!(repo.workspace.join("result.txt").is_file());
}

async fn admitted_legacy_delivery(
    repo: &TempRepo,
    mode: DeliveryMode,
) -> crate::native_v2_contract::AdmittedRun {
    let original = admitted(repo, mode).await;
    let mut node = delivery_node(mode);
    node["timeoutMs"] = json!(30_000);
    for path in ["/output/fields/outcome/type/values", "/signals/delivery"] {
        node.pointer_mut(path)
            .and_then(Value::as_array_mut)
            .assert_value()
            .retain(|label| label.as_str() != Some(DELIVERY_REPAIR_REQUIRED_LABEL));
    }
    NativeV2Admission
        .admit(RunSubmission {
            title: original.title,
            graph: full_graph(vec![node, success_node()]),
            initial_input: original.initial_input,
            runtime: original.runtime,
            source: original.source,
            submission_key: IdempotencyKey::new("legacy-target-review-required").assert_value(),
        })
        .await
        .assert_value()
}
