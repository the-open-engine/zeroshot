use super::*;
use crate::native_v2_candidate::test_support::{commit_all, path_text};

#[derive(Clone, Copy)]
enum Interruption {
    Repair,
    Timeout,
    Cancellation,
}

#[tokio::test]
async fn observed_review_survives_target_integration_and_another_review_round() {
    interrupted_target_reconciliation(Interruption::Repair).await;
}

#[tokio::test]
async fn observed_review_survives_target_reconciliation_timeout() {
    interrupted_target_reconciliation(Interruption::Timeout).await;
}

#[tokio::test]
async fn observed_review_survives_target_reconciliation_cancellation() {
    interrupted_target_reconciliation(Interruption::Cancellation).await;
}

async fn interrupted_target_reconciliation(interruption: Interruption) {
    let repo = TempRepo::delivery();
    let branch = "feature/observed-review";
    git(&repo.workspace, &["checkout", "-b", branch]);
    commit_all(&repo.workspace, "existing committed candidate");
    git(&repo.workspace, &["push", "origin", branch]);
    if matches!(interruption, Interruption::Repair) {
        let upstream = repo.root.child("upstream");
        git(
            repo.root.path(),
            &["clone", path_text(&repo.remote), path_text(&upstream)],
        );
        fs::write(upstream.join("README.md"), "advanced target\n").assert_value();
        commit_all(&upstream, "advance target");
        git(&upstream, &["push", "origin", "main"]);
    }
    let authority = Arc::new(FakeGitHub::new(
        repo.remote.clone(),
        if matches!(interruption, Interruption::Repair) {
            Script::NoCi
        } else {
            Script::TargetUnavailable
        },
    ));
    // Model an existing active review, observed before this lineage's first publication.
    authority
        .reviews
        .lock()
        .assert_value()
        .push(SourceReviewRequest {
            include_terminal_reviews: false,
            known_review: None,
            target: target(&repo),
            head_branch: branch.to_owned(),
            head_revision: git_output(&repo.workspace, &["rev-parse", "HEAD"]),
            title: "existing review".to_owned(),
            description: "existing work".to_owned(),
            source_issue: None,
        });
    let storage = repo.root.child("controller");
    fs::create_dir(&storage).assert_value();
    let lineage = RunId::new("observed-review");
    let mode = DeliveryMode::PullRequestV2;
    let path = storage.join("workspace-recovery.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "submission": delivery_submission(&repo, mode, PullRequestFeedback::Consider),
            "workspace": repo.workspace, "localBranch": branch, "deliveryRunId": lineage,
        }))
        .assert_value(),
    )
    .assert_value();
    let recorder = crate::native_v2_cli::local::review_binding_recorder(
        &storage,
        &repo.workspace,
        Some(branch),
        &lineage,
    )
    .assert_value();
    let cancelling = matches!(interruption, Interruption::Cancellation);
    let config = NativeV2DeliveryConfig {
        local_branch: Some(branch.to_owned()),
        delivery_run_id: lineage,
        adopt_existing_delivery: false,
        git_identity: None,
        commit_identity: None,
        commit_co_author: None,
        workspace: repo.workspace.clone(),
        git_program: "/usr/bin/git".into(),
        target: target(&repo),
        poll: DeliveryPollPolicy::new(
            if cancelling { 100 } else { 2 },
            if cancelling {
                Duration::from_secs(5)
            } else {
                Duration::ZERO
            },
        )
        .assert_value(),
    };
    let adapter = Arc::new(
        NativeV2DeliveryAdapter::new(config.clone(), authority.clone())
            .with_review_binding_recorder(recorder.clone()),
    );
    let request = || DeliveryRunRequest {
        repo: &repo,
        attempts: 2,
        mode,
        run_id: "observed-review",
        refresh: None,
    };
    let mut handle = start_with_adapter_token(
        request(),
        adapter.clone(),
        PullRequestFeedback::Consider,
        Some("test-token"),
    )
    .await;
    let mut durable = handle.take_initial_output().assert_value();
    tokio::time::timeout(Duration::from_secs(10), async {
        while authority.target_reconciliations.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .assert_value();
    let document: Value = serde_json::from_slice(&fs::read(&path).assert_value()).assert_value();
    let saved = &document["deliveryReceipt"];
    assert_eq!(saved["receipt"]["pullRequestId"], "17");
    assert_eq!(saved["published"], false);
    if cancelling {
        handle.cancel();
        assert!(matches!(
            handle.completion().await,
            Err(NodeRunnerError::Cancelled)
        ));
    } else {
        let outcome = handle.completion().await.assert_value().outcome;
        if matches!(interruption, Interruption::Repair) {
            assert_delivery_signal(&outcome, DELIVERY_REPAIR_REQUIRED_LABEL);
        } else {
            assert_eq!(
                outcome,
                WorkerOutcome::declared_failure(WorkerErrorCode::Timeout)
            );
        }
    }
    let mut diagnostics = Vec::new();
    while let Ok(event) = durable.recv().await {
        if let crate::native_v2_runner::DurableNodeEvent::Output { output, .. } = event {
            diagnostics.push(output.text);
        }
    }
    if !cancelling {
        let diagnostic = diagnostics
            .iter()
            .find(|text| text.contains("Already published (last confirmed)"))
            .assert_value();
        for expected in [
            branch,
            saved["receipt"]["headRevision"].as_str().assert_value(),
            "acme/project#17",
        ] {
            assert!(diagnostic.contains(expected), "{diagnostic}");
        }
        assert!(!diagnostic.contains(&delivery_head_branch(&config.delivery_run_id)));
    }
    let recovered =
        contract::review_from_recovery(saved, mode, &target(&repo), branch).assert_value();
    assert!(!recovered.published);
    let resumed = || {
        Arc::new(
            NativeV2DeliveryAdapter::new(
                NativeV2DeliveryConfig {
                    adopt_existing_delivery: true,
                    ..config.clone()
                },
                authority.clone(),
            )
            .with_recovered_review(Some(recovered.clone()))
            .with_review_binding_recorder(recorder.clone()),
        )
    };
    if matches!(interruption, Interruption::Repair) {
        let before = authority.target_reconciliations.load(Ordering::SeqCst);
        let adapter = resumed();
        let execution = run_with_adapter(request(), adapter.clone()).await;
        assert_delivery_signal(&execution.outcome, DELIVERY_REPAIR_REQUIRED_LABEL);
        assert_eq!(
            authority.target_reconciliations.load(Ordering::SeqCst),
            before + 1
        );
        let document: Value =
            serde_json::from_slice(&fs::read(&path).assert_value()).assert_value();
        assert_eq!(document["deliveryReceipt"]["published"], false);
    }
    let head = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
    authority.closed.store(true, Ordering::SeqCst);
    let execution = run_with_adapter(request(), resumed()).await;
    assert_eq!(
        execution.outcome,
        WorkerOutcome::declared_failure(WorkerErrorCode::Refusal)
    );
    assert_eq!(
        authority.delivery_read_reviews.lock().assert_value().last(),
        Some(&Some("17".to_owned()))
    );
    assert_eq!(authority.review_requests().len(), 1);
    assert_eq!(authority.head_updates.load(Ordering::SeqCst), 0);
    assert_eq!(git_output(&repo.workspace, &["rev-parse", "HEAD"]), head);
}
