use super::*;

struct RetainedCleanupFailureAllocator {
    claims: FakeClaimAuthority,
    cleanup_calls: StdMutex<Vec<(RunId, RunRuntimeExit)>>,
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
        _run_id: &RunId,
        _admitted: &AdmittedRun,
        _github_token: Option<&str>,
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
        openengine_cluster_protocol::WorkspaceRecovery {
            recoverable: true,
            ..Default::default()
        }
    }
}

#[tokio::test]
async fn invalid_submission_has_no_durable_or_allocation_effect() {
    let harness = harness(Behavior::Complete).await;
    assert!(matches!(
        submit_test_request(&harness.controller, request(json!({}))).await,
        Err(NativeV2CloudError::Admission(_))
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
}

#[tokio::test]
async fn startup_reconciles_every_persisted_nonterminal_before_status_is_visible() {
    let ledger = Arc::new(FakeRunLedger::new());
    let run_id = seed_controller_reconstructed_run(&ledger, "run-restart").await;
    let driver = Arc::new(FakeDriver::new(Behavior::Complete));
    let cleanup = Arc::new(FakeCleanup::new(ledger.clone()));
    let allocator = Arc::new(FakeAllocator::new(driver, cleanup.clone()));
    let controller = NativeV2CloudController::new(ledger, allocator.clone())
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
async fn unconfirmed_retained_cleanup_stays_nonterminal_until_reconstruction() {
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
    let controller = NativeV2CloudController::new(ledger.clone(), allocator.clone())
        .await
        .assert_value_with("initial controller");
    let successor_run_id = RunId::new("resume-successor");

    let error = controller
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
        })
        .await
        .expect_err("cleanup failure must fail resume");
    assert!(
        matches!(
            error,
            NativeV2CloudError::Supervisor(NativeV2SupervisorError::RuntimeCleanup(_))
        ),
        "unexpected resume error: {error:?}"
    );
    let successor = ledger
        .get(&successor_run_id)
        .await
        .assert_value_with("successor lookup")
        .assert_value_with("successor run");
    assert!(successor.snapshot.terminal.is_none());
    let status = controller
        .status(RunStatusParams {
            run_id: successor_run_id.clone(),
        })
        .await
        .assert_value_with("nonterminal successor status");
    assert!(!status.workspace_recovery.recoverable);

    drop(controller);
    let replacement = NativeV2CloudController::new(ledger, allocator.clone())
        .await
        .assert_value_with("replacement controller");
    assert_eq!(
        allocator.cleanup_calls(),
        vec![(successor_run_id.clone(), RunRuntimeExit::RuntimeLost)]
    );
    let status = replacement
        .status(RunStatusParams {
            run_id: successor_run_id,
        })
        .await
        .assert_value_with("reconciled successor status");
    assert!(matches!(
        status.status,
        RunStatus::Finished {
            terminal_result: TerminalResult::Failed { ref reason },
            ..
        } if reason.as_str() == "runtime_lost"
    ));
    assert!(status.workspace_recovery.recoverable);
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
