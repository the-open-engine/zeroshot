use super::*;
use openengine_cluster_protocol::NonEmptyEnumSet;

#[tokio::test]
async fn locked_index_provides_raw_git_feedback_and_can_recover() {
    for mode in [DeliveryMode::PullRequest, DeliveryMode::Merge] {
        let repo = TempRepo::delivery();
        let lock = repo.workspace.join(".git/index.lock");
        fs::write(&lock, "").assert_value();
        let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), Script::NoCi));
        let failure = run_delivery(&repo, authority.clone(), 3, mode).await;
        let output = assert_delivery_signal(&failure, DELIVERY_REPAIR_REQUIRED_LABEL);
        assert_eq!(output["pullRequestId"], "");
        assert!(!authority.pushed.load(Ordering::SeqCst));
        let diagnostic = outcome_diagnostic(&failure);
        assert!(diagnostic.contains("index.lock"));
        assert!(diagnostic.contains("exitStatus: Some(128)"));
        assert!(diagnostic.contains("File exists"));
        assert!(diagnostic.contains("baseRevision:"));
        assert_receipt_match(output, mode, &repo, false);

        // The repair worker removes this stale lock; delivery itself never manages user files.
        fs::remove_file(lock).assert_value();
        let completed = run_delivery(&repo, authority, 3, mode).await;
        assert_delivery_signal(&completed, mode.success_outcome());
    }
}

#[tokio::test]
async fn repair_can_remove_tooling_from_unpublished_history_then_deliver() {
    let repo = TempRepo::delivery();
    let tools = repo.workspace.join(".tools");
    fs::create_dir(&tools).assert_value();
    fs::write(tools.join("downloaded-compiler"), "not part of the change").assert_value();
    let rejected = Arc::new(FakeGitHub::new(repo.remote.clone(), Script::PushRejected));
    let failure = run_delivery(&repo, rejected, 3, DeliveryMode::Merge).await;
    assert_delivery_signal(&failure, DELIVERY_REPAIR_REQUIRED_LABEL);
    assert!(
        git_output(&repo.workspace, &["ls-tree", "-r", "--name-only", "HEAD"]).contains(".tools/")
    );

    // Simulate agent diagnosis: rewriting only unpublished history removes blobs from the push.
    git(&repo.workspace, &["reset", "--mixed", &repo.base]);
    fs::rename(&tools, repo.root.path().join("downloaded-tools")).assert_value();
    let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), Script::NoCi));
    let completed = run_delivery(&repo, authority, 3, DeliveryMode::Merge).await;
    let output = assert_delivery_signal(&completed, DELIVERY_MERGED_LABEL);
    let revision = output["headRevision"].as_str().assert_value();
    let published = git_output(&repo.remote, &["rev-list", "--objects", revision]);
    assert!(!published.contains(".tools"));
    assert!(published.contains("result.txt"));
}

#[tokio::test]
async fn legacy_response_contract_keeps_receipt_validation_without_repair_opt_in() {
    for mode in [
        DeliveryMode::PullRequest,
        DeliveryMode::MergeV1,
        DeliveryMode::Merge,
    ] {
        // Work with the typed schema so the optional capability cannot alter any other field.
        let mut output = delivery_result_schema(mode).assert_value();
        if let openengine_cluster_protocol::PayloadType::Record { fields } = &mut output {
            let field = fields
                .get_mut(&FieldName::new("outcome").assert_value())
                .assert_value();
            if let openengine_cluster_protocol::PayloadType::Enum { values } = &mut field.value_type
            {
                *values = NonEmptyEnumSet::new(
                    values
                        .values()
                        .iter()
                        .filter(|v| v.as_str() != DELIVERY_REPAIR_REQUIRED_LABEL)
                        .cloned()
                        .collect(),
                )
                .assert_value();
            }
        }
        let labels = delivery_signal_labels(mode).assert_value();
        let response = NodeResponseContract::Verifier {
            output,
            signals: BTreeMap::from([(
                FieldName::new(DELIVERY_SIGNAL_FIELD).assert_value(),
                NonEmptyEnumSet::new(
                    labels
                        .values()
                        .iter()
                        .filter(|v| v.as_str() != DELIVERY_REPAIR_REQUIRED_LABEL)
                        .cloned()
                        .collect(),
                )
                .assert_value(),
            )]),
            diagnostic: delivery_diagnostic_schema().assert_value(),
        };
        validate_delivery_contract(mode, &response).assert_value();
        assert!(!contract::supports_repair(&response));
    }
}

#[tokio::test]
async fn later_delivery_errors_preserve_the_existing_review_in_repair_feedback() {
    for script in [Script::InspectFailed, Script::MergeFailed] {
        let repo = TempRepo::delivery();
        let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), script));
        let outcome = run_delivery(&repo, authority, 3, DeliveryMode::Merge).await;
        let output = assert_delivery_signal(&outcome, DELIVERY_REPAIR_REQUIRED_LABEL);
        assert!(!output["pullRequestId"].as_str().assert_value().is_empty());
        assert!(!output["headRevision"].as_str().assert_value().is_empty());
        assert!(outcome_diagnostic(&outcome).contains("remote"));
        assert_receipt_match(output, DeliveryMode::Merge, &repo, false);
    }
}

#[tokio::test]
async fn repair_retry_resumes_the_authorized_remote_head_before_pushing() {
    let repo = TempRepo::delivery();
    let authority = Arc::new(FakeGitHub::new(
        repo.remote.clone(),
        Script::HeadAdoptionAfterRepair,
    ));
    let adapter = Arc::new(NativeV2DeliveryAdapter::new(
        NativeV2DeliveryConfig {
            workspace: repo.workspace.clone(),
            git_program: "/usr/bin/git".into(),
            target: target(&repo),
            poll: DeliveryPollPolicy::new(3, Duration::ZERO).assert_value(),
        },
        authority.clone(),
    ));
    let request = || DeliveryRunRequest {
        repo: &repo,
        attempts: 3,
        mode: DeliveryMode::Merge,
        run_id: "resume-authorized-head",
        refresh: None,
    };
    let failure = run_with_adapter(request(), adapter.clone()).await.outcome;
    let output = assert_delivery_signal(&failure, DELIVERY_REPAIR_REQUIRED_LABEL);
    assert_ne!(
        git_output(&repo.workspace, &["rev-parse", "HEAD"]),
        output["headRevision"].as_str().assert_value()
    );
    let success = run_with_adapter(request(), adapter).await.outcome;
    assert_delivery_signal(&success, DELIVERY_MERGED_LABEL);
    assert_eq!(authority.head_updates.load(Ordering::SeqCst), 1);
    assert_eq!(authority.head_sync_attempts.load(Ordering::SeqCst), 2);
}
