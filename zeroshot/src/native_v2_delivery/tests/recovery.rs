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
        assert!(diagnostic.contains("sourceRevision:"));
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
async fn later_api_failure_does_not_request_code_repair() {
    for script in [Script::MergeFailed] {
        let repo = TempRepo::delivery();
        let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), script));
        let outcome = run_delivery(&repo, authority, 3, DeliveryMode::Merge).await;
        assert_eq!(
            outcome,
            WorkerOutcome::declared_failure(WorkerErrorCode::Crash)
        );
    }
}

#[tokio::test]
async fn later_execution_resumes_the_authorized_remote_head_after_infrastructure_failure() {
    let repo = TempRepo::delivery();
    let (authority, adapter) = retained_delivery(&repo, Script::HeadAdoptionAfterRepair);
    let request = || DeliveryRunRequest {
        repo: &repo,
        attempts: 3,
        mode: DeliveryMode::Merge,
        run_id: "resume-authorized-head",
        refresh: None,
    };
    let failure = run_with_adapter(request(), adapter.clone()).await.outcome;
    assert_eq!(
        failure,
        WorkerOutcome::declared_failure(WorkerErrorCode::Crash)
    );
    let remote_head = git_output(
        &repo.remote,
        &[
            "rev-parse",
            &format!("refs/heads/{}", delivery_branch("delivery-run")),
        ],
    );
    assert_ne!(
        git_output(&repo.workspace, &["rev-parse", "HEAD"]),
        remote_head
    );
    let success = run_with_adapter(request(), adapter).await.outcome;
    head_update::assert_retried_head_adoption(&authority, &success);
}

#[tokio::test]
async fn transient_inspection_failure_exhausts_explicit_poll_policy_without_code_repair() {
    let repo = TempRepo::delivery();
    let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), Script::InspectFailed));
    let outcome = run_delivery(&repo, authority, 3, DeliveryMode::Merge).await;
    assert_eq!(
        outcome,
        WorkerOutcome::declared_failure(WorkerErrorCode::Timeout)
    );
}

#[tokio::test]
async fn already_merged_pr_preserves_dirty_or_newer_local_work_without_republishing() {
    for committed in [false, true] {
        let repo = TempRepo::delivery();
        let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), Script::NoCi));
        let initial = run_delivery(&repo, authority.clone(), 3, DeliveryMode::Merge).await;
        let receipt = assert_delivery_signal(&initial, DELIVERY_MERGED_LABEL);
        let published = receipt["headRevision"].as_str().assert_value();
        fs::write(repo.workspace.join("unshipped.txt"), "keep this work\n").assert_value();
        if committed {
            git(&repo.workspace, &["add", "unshipped.txt"]);
            git(
                &repo.workspace,
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "commit",
                    "--no-verify",
                    "-m",
                    "new local work",
                ],
            );
        }
        let before = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
        let outcome = run_delivery(&repo, authority.clone(), 3, DeliveryMode::Merge).await;
        assert_eq!(
            outcome,
            WorkerOutcome::declared_failure(WorkerErrorCode::Refusal)
        );
        assert_eq!(git_output(&repo.workspace, &["rev-parse", "HEAD"]), before);
        assert_eq!(
            git_output(
                &repo.remote,
                &[
                    "rev-parse",
                    &format!("refs/heads/{}", delivery_branch("delivery-run"))
                ]
            ),
            published
        );
        assert_eq!(
            fs::read_to_string(repo.workspace.join("unshipped.txt")).assert_value(),
            "keep this work\n"
        );
        assert_eq!(authority.review_requests().len(), 1);
    }
}

#[tokio::test]
async fn completed_reconciliation_followed_by_retry_still_requires_current_work_review() {
    let repo = TempRepo::delivery();
    let (authority, adapter) = retained_delivery(&repo, Script::ReconcileCompletesThenUnavailable);
    let request = || DeliveryRunRequest {
        repo: &repo,
        attempts: 3,
        mode: DeliveryMode::Merge,
        run_id: "completed-reconciliation",
        refresh: None,
    };
    let initial = run_with_adapter(request(), adapter.clone()).await.outcome;
    assert_delivery_signal(&initial, DELIVERY_CI_FAILED_LABEL);
    let branch = delivery_branch("delivery-run");
    let external = repo.root.child("external-reconciliation");
    git(
        repo.root.path(),
        &[
            "clone",
            "--branch",
            &branch,
            repo.remote.to_str().assert_value(),
            external.to_str().assert_value(),
        ],
    );
    fs::write(external.join("remote.txt"), "external change\n").assert_value();
    git(&external, &["add", "remote.txt"]);
    git(
        &external,
        &[
            "-c",
            "user.name=External",
            "-c",
            "user.email=external@example.invalid",
            "commit",
            "--no-verify",
            "-m",
            "external change",
        ],
    );
    git(&external, &["push", "origin", &branch]);
    let external_head = git_output(&external, &["rev-parse", "HEAD"]);
    let outcome = run_with_adapter(request(), adapter).await.outcome;
    assert_delivery_signal(&outcome, DELIVERY_REPAIR_REQUIRED_LABEL);
    assert!(
        outcome_diagnostic(&outcome).contains("workspace changed during trusted reconciliation")
    );
    assert_eq!(
        git_output(&repo.workspace, &["rev-parse", "HEAD"]),
        external_head
    );
    assert_eq!(authority.head_sync_attempts.load(Ordering::SeqCst), 2);
    assert_eq!(authority.review_sync_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(authority.merge_requests.load(Ordering::SeqCst), 0);
}

fn retained_delivery(
    repo: &TempRepo,
    script: Script,
) -> (Arc<FakeGitHub>, Arc<NativeV2DeliveryAdapter>) {
    let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), script));
    let adapter = retained_adapter(
        repo,
        authority.clone(),
        DeliveryPollPolicy::new(3, Duration::ZERO).assert_value(),
        DeliveryLineage::original("delivery-run"),
    );
    (authority, adapter)
}
