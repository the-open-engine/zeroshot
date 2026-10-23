use super::*;

struct RetainedCleanupFailureAllocator {
    claims: FakeClaimAuthority,
    cleanup_calls: StdMutex<Vec<(RunId, RunRuntimeExit)>>,
}

async fn assert_reconstructed_discard_contracts(
    controller: &NativeV2PlatformController,
    successor_run_id: RunId,
) {
    assert!(matches!(
        controller
            .discard_workspace(RunDiscardWorkspaceParams {
                run_id: RunId::new("missing-run"),
            })
            .await,
        Err(NativeV2PlatformError::Ledger(RunLedgerError::RunNotFound))
    ));
    assert!(matches!(
        controller
            .checkpoints(openengine_cluster_protocol::RunCheckpointsParams {
                run_id: RunId::new("missing-run"),
                after: None,
                limit: None,
            })
            .await,
        Err(NativeV2PlatformError::Ledger(RunLedgerError::RunNotFound))
    ));
    let discarded = controller
        .discard_workspace(RunDiscardWorkspaceParams {
            run_id: successor_run_id,
        })
        .await
        .assert_value_with("workspace discard");
    assert!(!discarded.discarded);
}

impl RetainedCleanupFailureAllocator {
    fn new() -> Self {
        Self {
            claims: FakeClaimAuthority::default(),
            cleanup_calls: StdMutex::new(Vec::new()),
        }
    }

    fn cleanup_calls(&self) -> Vec<(RunId, RunRuntimeExit)> {
        self.cleanup_calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl CapsuleAllocator for RetainedCleanupFailureAllocator {
    async fn claim_controller(
        &self,
        run_id: &RunId,
    ) -> Result<Arc<dyn ExclusiveControllerClaim>, ControllerClaimUnavailable> {
        self.claims.acquire(run_id)
    }

    async fn allocate(
        &self,
        _request: CapsuleAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        Err(CapsuleAllocationUnavailable::Runtime)
    }

    async fn allocate_from_retained(
        &self,
        _request: RetainedAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, RetainedAllocationUnavailable> {
        Err(RetainedAllocationUnavailable::CleanupUnconfirmed(
            CapsuleCleanupUnavailable,
        ))
    }

    async fn destroy_or_confirm_absent(
        &self,
        run_id: &RunId,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        self.cleanup_calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((run_id.clone(), exit));
        Ok(CapsuleDestroyed::confirmed())
    }

    async fn workspace_recovery(
        &self,
        _run_id: &RunId,
    ) -> openengine_cluster_protocol::WorkspaceRecovery {
        recoverable_workspace()
    }
}

#[tokio::test]
async fn allocator_defaults_are_explicit_fail_closed_and_side_effect_free() {
    assert_eq!(
        CapsuleAllocationUnavailable::Runtime.failure_code(),
        "runtime_unavailable"
    );
    assert_eq!(
        CapsuleAllocationUnavailable::SourceCheckout.failure_code(),
        "source_checkout_unavailable"
    );
    let submission = request(Value::Null).submission;
    let admitted = NativeV2Admission
        .admit(submission)
        .await
        .assert_value_with("default allocator fixture admission");
    let ledger = Arc::new(FakeRunLedger::new());
    let cleanup = Arc::new(FakeCleanup::new(ledger));
    let driver = Arc::new(FakeDriver::new(Behavior::Complete));
    let allocator = TestAllocator::<ImmediateAllocation>::new(driver.clone(), cleanup.clone());
    let source_run_id = RunId::new("default-source");
    let run_id = RunId::new("default-successor");

    assert!(matches!(
        allocator
            .allocate_from_retained(RetainedAllocationRequest {
                preparation: test_preparation(),
                selection:
                    crate::native_v2_supervisor::checkpoints::CheckpointRestoreSelection::Latest,
                source_run_id: &source_run_id,
                run_id: &run_id,
                admitted: &admitted,
                github_token: None,
                git_credentials: None,
            })
            .await,
        Err(RetainedAllocationUnavailable::Settled(
            CapsuleAllocationUnavailable::Runtime
        ))
    ));
    assert_eq!(
        allocator.workspace_recovery(&run_id).await,
        openengine_cluster_protocol::WorkspaceRecovery::default()
    );
    let checkpoint_error = allocator
        .checkpoints(openengine_cluster_protocol::RunCheckpointsParams {
            run_id: run_id.clone(),
            after: None,
            limit: None,
        })
        .await
        .expect_err("default allocator must reject workspace checkpoints");
    assert_eq!(
        checkpoint_error.to_string(),
        "workspace checkpoint is unavailable: workspace checkpoints are unavailable"
    );
    assert!(!allocator.discard_workspace(&run_id).await.assert_value());

    let runner = Arc::new(
        NativeNodeRunner::new(&admitted, driver, Arc::new(FakeSessionFactory::default()))
            .assert_value_with("default allocated capsule runner"),
    );
    let (_loss_sender, loss) = watch::channel(false);
    let allocated = AllocatedCapsule::new(runner, loss, cleanup);
    assert!(allocated.checkpoints.is_none());
    assert!(allocated.execution_seed.is_empty());
    assert!(!*allocated.loss.borrow());
}

#[tokio::test]
async fn wave8_cli_contract_submission_rejects_missing_secrets_and_invalid_graphs_before_effects() {
    let harness = harness(Behavior::Complete).await;
    assert!(matches!(
        harness.controller.submit(request(Value::Null)).await,
        Err(NativeV2PlatformError::Environment(_))
    ));
    assert!(matches!(
        submit_test_request(&harness.controller, request(json!({}))).await,
        Err(NativeV2PlatformError::Admission(_))
    ));
    assert_eq!(harness.allocator.allocation_count(), 0);
    assert!(
        harness
            .controller
            .list()
            .await
            .assert_value_with("list")
            .is_empty()
    );

    let submission = request(Value::Null).submission;
    let admitted = NativeV2Admission
        .admit(submission)
        .await
        .assert_value_with("admitted resolver fixture");
    let error = resume_secret_envelope(ResumeSecretRequest {
        admitted: &admitted,
        successor_run_id: &RunId::new("resolver-successor"),
        connections: BTreeMap::new(),
        connection_resolver: Some(openengine_cluster_protocol::TargetConnectionResolver {
            endpoint: "http://resolver.example".to_owned(),
            bearer_token: "private-token".to_owned(),
            keys: vec![ConnectionKey::new("test").assert_value_with("resolver key")],
            source_connection: None,
        }),
        github_token: None,
        git_credentials: None,
    })
    .err()
    .expect("insecure resolver authority must fail closed");
    assert!(matches!(error, NativeV2PlatformError::ResumeCredentials));
}

#[tokio::test]
async fn startup_reconciles_every_persisted_nonterminal_before_status_is_visible() {
    let ledger = Arc::new(FakeRunLedger::new());
    let run_id = seed_controller_reconstructed_run(&ledger, "run-restart").await;
    let driver = Arc::new(FakeDriver::new(Behavior::Complete));
    let cleanup = Arc::new(FakeCleanup::new(ledger.clone()));
    let allocator = Arc::new(FakeAllocator::new(driver, cleanup.clone()));
    let controller = NativeV2PlatformController::new(ledger, allocator.clone())
        .await
        .assert_value_with("reconciled startup");

    let status = controller
        .status(RunStatusParams {
            run_id: run_id.clone(),
        })
        .await
        .assert_value_with("status after startup");
    assert_eq!(
        status.status,
        RunStatus::Finished {
            terminal_result: TerminalResult::Failed {
                context: None,
                reason: EnumLabel::new("runtime_lost").assert_value_with("label")
            },
            metadata: Default::default(),
        }
    );
    assert_eq!(controller.list().await.assert_value_with("list").len(), 1);
    assert_eq!(allocator.allocation_count(), 0);
    assert_eq!(cleanup.exits(), vec![RunRuntimeExit::RuntimeLost]);
    assert_eq!(cleanup.terminal_seen(), vec![false]);
}

#[tokio::test]
async fn wave8_cli_contract_retained_cleanup_and_discard_fail_closed_across_reconstruction() {
    let ledger = Arc::new(FakeRunLedger::new());
    let source_run_id = seed_controller_reconstructed_run(&ledger, "resume-source").await;
    let source = ledger
        .get(&source_run_id)
        .await
        .assert_value_with("source lookup")
        .assert_value_with("source run");
    append_terminal_failure(ledger.as_ref(), &source, "worker_failed")
        .await
        .assert_value_with("source failure");
    let allocator = Arc::new(RetainedCleanupFailureAllocator::new());
    let controller = NativeV2PlatformController::new(ledger.clone(), allocator.clone())
        .await
        .assert_value_with("initial controller");
    let successor_run_id = RunId::new("resume-successor");

    controller
        .resume(RunResumeParams {
            from: None,
            run_id: source_run_id,
            successor_run_id: successor_run_id.clone(),
            connections: BTreeMap::from([(
                ConnectionKey::new("test").assert_value_with("connection key"),
                test_connection_values("replacement-secret"),
            )]),
            connection_resolver: None,
            github_token: None,
            git_credentials: None,
        })
        .await
        .assert_value_with("accepted resume");
    let terminal = terminal(&controller, &successor_run_id).await;
    assert!(
        matches!(terminal, TerminalResult::Failed { reason, .. } if reason.as_str() == "runtime_unavailable")
    );
    assert_eq!(
        allocator.cleanup_calls(),
        vec![(successor_run_id.clone(), RunRuntimeExit::RuntimeLost)]
    );
    drop(controller);
    let replacement = NativeV2PlatformController::new(ledger, allocator.clone())
        .await
        .assert_value();
    assert_reconstructed_discard_contracts(&replacement, successor_run_id).await;
}

#[tokio::test]
async fn allocator_claims_are_exclusive_per_run_not_per_target() {
    let driver = Arc::new(FakeDriver::new(Behavior::Complete));
    let cleanup = Arc::new(FakeCleanup::new(Arc::new(FakeRunLedger::new())));
    let allocator = Arc::new(FakeAllocator::new(driver, cleanup));
    let first_id = RunId::new("run-first");
    let second_id = RunId::new("run-second");
    let first = allocator
        .claim_controller(&first_id)
        .await
        .assert_value_with("first run claim");
    allocator
        .claim_controller(&second_id)
        .await
        .assert_value_with("different run claim");
    assert!(allocator.claim_controller(&first_id).await.is_err());
    drop(first);
    allocator
        .claim_controller(&first_id)
        .await
        .assert_value_with("released run claim");
}

use openengine_cluster_testkit::assertions::{AssertValue};

fn recoverable_workspace() -> openengine_cluster_protocol::WorkspaceRecovery {
    openengine_cluster_protocol::WorkspaceRecovery {
        recoverable: true,
        ..Default::default()
    }
}

struct SourceRecoveryAllocator {
    runtime: FakeAllocator,
    resumed_credentials:
        StdMutex<Vec<Option<openengine_cluster_protocol::GitTransportCredentials>>>,
}

#[async_trait]
impl CapsuleAllocator for SourceRecoveryAllocator {
    async fn claim_controller(
        &self,
        run_id: &RunId,
    ) -> Result<Arc<dyn ExclusiveControllerClaim>, ControllerClaimUnavailable> {
        self.runtime.claim_controller(run_id).await
    }

    async fn allocate(
        &self,
        request: CapsuleAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        self.runtime.allocate(request).await
    }

    async fn allocate_from_retained(
        &self,
        request: RetainedAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, RetainedAllocationUnavailable> {
        self.resumed_credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.git_credentials.cloned());
        self.runtime
            .allocate(CapsuleAllocationRequest {
                run_id: request.run_id,
                admitted: request.admitted,
                github_token: request.github_token,
                git_credentials: request.git_credentials,
                preparation: request.preparation,
            })
            .await
            .map_err(Into::into)
    }

    async fn destroy_or_confirm_absent(
        &self,
        run_id: &RunId,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        self.runtime.destroy_or_confirm_absent(run_id, exit).await
    }

    async fn workspace_recovery(
        &self,
        _run_id: &RunId,
    ) -> openengine_cluster_protocol::WorkspaceRecovery {
        recoverable_workspace()
    }
}

#[tokio::test]
async fn recoverable_status_preserves_source_authorization_and_fresh_resume_isolates_node_values() {
    use openengine_cluster_protocol::GitTransportCredentials;

    for (remote, credentials) in [
        (
            "https://github.com/owner/repo.git",
            GitTransportCredentials::Https {
                username: "fresh-source-user".to_owned(),
                password: "fresh-source-password".to_owned(),
            },
        ),
        (
            "git@github.com:owner/repo.git",
            GitTransportCredentials::Ssh {
                private_key: "fresh-source-private-key".to_owned(),
                known_hosts: "fresh-source-known-hosts".to_owned(),
            },
        ),
    ] {
        assert_recoverable_source_resume(remote, credentials).await;
    }
}

async fn seed_failed_source_run(submitted: &RunSubmitParams, ledger: &Arc<FakeRunLedger>) {
    let admitted = NativeV2Admission
        .admit(submitted.submission.clone())
        .await
        .assert_value();
    ledger
        .create_or_get(CreateRun {
            run_id: submitted.run_id.clone(),
            submission_key: submitted.submission.submission_key.clone(),
            submission_digest: submission_digest(&submitted.submission).assert_value(),
            admitted,
        })
        .await
        .assert_value();
    let failed = ledger
        .get(&submitted.run_id)
        .await
        .assert_value()
        .assert_value();
    append_terminal_failure(ledger.as_ref(), &failed, "worker_failed")
        .await
        .assert_value();
}

fn source_recovery_request(remote: &str) -> RunSubmitParams {
    let mut submitted = request(Value::Null);
    submitted.submission.source = Some(
        serde_json::from_value(json!({
            "repository": "owner/repo",
            "branch": "main",
            "revision": "1111111111111111111111111111111111111111",
            "remote": {
                "url": remote,
                "connection": "test",
                "forge": {"provider": "source.github", "repository": "owner/repo"}
            }
        }))
        .assert_value(),
    );
    submitted
}

async fn assert_recoverable_source_resume(
    remote: &str,
    credentials: openengine_cluster_protocol::GitTransportCredentials,
) {
    let harness = harness(Behavior::Complete).await;
    let ledger = &harness.ledger;
    let submitted = source_recovery_request(remote);
    let authorized = submitted
        .submission
        .connection_requirements()
        .into_iter()
        .map(|(key, fields)| (key, fields.into_iter().collect::<Vec<_>>()))
        .collect::<openengine_cluster_protocol::RunConnectionRequirements>();
    let source_key = ConnectionKey::new("test").assert_value();
    let mut expected_fields = credentials.connection_values().assert_value().field_names();
    expected_fields.push(EnvironmentVariableName::new("NODE_TOKEN").assert_value());
    expected_fields.sort();
    assert_eq!(
        authorized,
        BTreeMap::from([(source_key.clone(), expected_fields)])
    );

    seed_failed_source_run(&submitted, ledger).await;

    let allocator = Arc::new(SourceRecoveryAllocator {
        runtime: FakeAllocator::new(harness.driver.clone(), harness.cleanup.clone()),
        resumed_credentials: StdMutex::new(Vec::new()),
    });
    let controller = NativeV2PlatformController::new(ledger.clone(), allocator.clone())
        .await
        .assert_value();
    let status = controller
        .status(RunStatusParams {
            run_id: submitted.run_id.clone(),
        })
        .await
        .assert_value();
    assert!(status.workspace_recovery.recoverable);
    assert_eq!(
        status.workspace_recovery.connection_requirements,
        authorized
    );

    let mut fresh = credentials
        .connection_values()
        .assert_value()
        .as_map()
        .clone();
    fresh.insert(
        EnvironmentVariableName::new("NODE_TOKEN").assert_value(),
        "fresh-node-token".to_owned(),
    );
    let successor_run_id = RunId::new("source-authorized-successor");
    controller
        .resume(RunResumeParams {
            run_id: submitted.run_id,
            successor_run_id: successor_run_id.clone(),
            from: None,
            connections: BTreeMap::from([(
                source_key,
                StaticConnectionValues::new(fresh).assert_value(),
            )]),
            connection_resolver: None,
            github_token: None,
            git_credentials: None,
        })
        .await
        .assert_value();
    assert!(matches!(
        terminal(&controller, &successor_run_id).await,
        TerminalResult::Succeeded { .. }
    ));
    assert_eq!(
        allocator
            .resumed_credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_slice(),
        &[Some(credentials)]
    );
    assert_eq!(
        harness.driver.environments().as_slice(),
        &[
            BTreeMap::from([("NODE_TOKEN".to_owned(), "fresh-node-token".to_owned())]),
            BTreeMap::new(),
        ]
    );
    let successor = ledger
        .get(&successor_run_id)
        .await
        .assert_value()
        .assert_value();
    assert_public_resume_admission(&successor.admitted, submitted.submission.source);
}

fn assert_public_resume_admission(admitted: &AdmittedRun, source: Option<ResolvedSource>) {
    assert_eq!(admitted.source, source);
    let durable = serde_json::to_string(admitted).assert_value();
    for secret in [
        "fresh-source-user",
        "fresh-source-password",
        "fresh-source-private-key",
        "fresh-source-known-hosts",
        "fresh-node-token",
    ] {
        assert!(!durable.contains(secret));
    }
}
