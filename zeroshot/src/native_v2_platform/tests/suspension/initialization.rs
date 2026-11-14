use super::*;
use crate::v2_run_ledger::{AppendResult, SnapshotAndTail};
use openengine_cluster_protocol::Cursor;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy)]
enum FailurePoint {
    Version,
    Vector,
    PriorExecution,
    Reload,
}

struct InitializationLedger {
    inner: Arc<FakeRunLedger>,
    successor: RunId,
    point: FailurePoint,
    failed: AtomicBool,
}

impl InitializationLedger {
    fn fail_once(&self, run_id: &RunId, matches: bool) -> Result<(), RunLedgerError> {
        if run_id == &self.successor && matches && !self.failed.swap(true, Ordering::SeqCst) {
            return Err(RunLedgerError::Storage);
        }
        Ok(())
    }
}

#[async_trait]
impl RunLedger for InitializationLedger {
    async fn create_or_get(&self, request: CreateRun) -> Result<CreateRunOutcome, RunLedgerError> {
        self.inner.create_or_get(request).await
    }

    async fn get(&self, run_id: &RunId) -> Result<Option<StoredRun>, RunLedgerError> {
        self.fail_once(run_id, matches!(self.point, FailurePoint::Reload))?;
        self.inner.get(run_id).await
    }

    async fn get_by_submission_key(
        &self,
        key: &IdempotencyKey,
    ) -> Result<Option<StoredRun>, RunLedgerError> {
        self.inner.get_by_submission_key(key).await
    }

    async fn list(&self) -> Result<Vec<RunSummary>, RunLedgerError> {
        self.inner.list().await
    }

    async fn append(
        &self,
        run_id: &RunId,
        events: Vec<RunEvent>,
    ) -> Result<AppendResult, RunLedgerError> {
        self.fail_once(
            run_id,
            events.iter().any(|event| match self.point {
                FailurePoint::Version => matches!(event, RunEvent::SafeLog { line, .. }
                if line.as_str().starts_with("Zeroshot version: ")),
                FailurePoint::Vector => matches!(event, RunEvent::Vector { .. }),
                FailurePoint::PriorExecution => matches!(event, RunEvent::PriorExecution { .. }),
                FailurePoint::Reload => false,
            }),
        )?;
        self.inner.append(run_id, events).await
    }

    async fn request_force_stop(&self, run_id: &RunId) -> Result<AppendResult, RunLedgerError> {
        self.inner.request_force_stop(run_id).await
    }

    async fn request_suspend(&self, run_id: &RunId) -> Result<AppendResult, RunLedgerError> {
        self.inner.request_suspend(run_id).await
    }

    async fn vector_cursor(
        &self,
        run_id: &RunId,
        id: &openengine_cluster_protocol::VectorId,
    ) -> Result<Cursor, RunLedgerError> {
        self.inner.vector_cursor(run_id, id).await
    }

    async fn snapshot_and_tail(
        &self,
        run_id: &RunId,
        after: Option<&Cursor>,
    ) -> Result<SnapshotAndTail, RunLedgerError> {
        self.inner.snapshot_and_tail(run_id, after).await
    }
}

async fn rejected_initialization_leaves_source_resumable(point: FailurePoint, suspended: bool) {
    let driver = Arc::new(FakeDriver::new(Behavior::Complete));
    let (inner, cleanup, runtime) = runtime_parts(driver.clone());
    let source = seed_controller_reconstructed_run(&inner, "initialization-source").await;
    // Keep one settled prerequisite in the suspended source's continuation.
    let execution = crate::full_v1_reducer::DurableExecution {
        delivery_state: None,
        dispatch_position: crate::full_v1_reducer::HistoryPosition::new(1).assert_value(),
        node_instance: crate::full_v1_reducer::NodeInstanceId::new(1).assert_value(),
        execution: crate::full_v1_reducer::ExecutionId::new(1).assert_value(),
        occurrence: crate::full_v1_reducer::StructuralOccurrence {
            node: NodeName::new("worker").assert_value(),
            map_indices: Vec::new(),
        },
        attempt: openengine_cluster_protocol::PositiveInteger::new(1).assert_value(),
        input: Value::Null,
        state: crate::full_v1_reducer::DurableExecutionState::Settled {
            position: crate::full_v1_reducer::HistoryPosition::new(2).assert_value(),
            outcome: WorkerOutcome::Verified {
                output: Value::Null,
                artifacts: Vec::new(),
            },
        },
    };
    inner
        .append(
            &source,
            vec![
                RunEvent::PriorExecution { execution },
                RunEvent::RunStarted,
                crate::v2_run_ledger::vector_event(&source, RunVectorContent::SuspendRequest {})
                    .assert_value(),
            ],
        )
        .await
        .assert_value();
    if suspended {
        inner
            .append(
                &source,
                vec![RunEvent::Suspended {
                    suspension: RunSuspension::Command {
                        node: NodeName::new("deliver").assert_value(),
                        created_at: UnixTimestampMillis::new(1).assert_value(),
                    },
                }],
            )
            .await
            .assert_value();
    } else {
        let stored = inner.get(&source).await.assert_value().assert_value();
        append_terminal_failure(
            inner.as_ref(),
            &stored,
            "worker_failed",
            WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
        )
        .await
        .assert_value();
    }
    let before = inner.get(&source).await.assert_value().assert_value();
    let allocator = Arc::new(SuspensionAllocator {
        runtime,
        policy: Default::default(),
        store: Arc::new(SuspensionStore::default()),
        successor: StdMutex::new(None),
        preparation_gate: None,
    });
    let ledger = Arc::new(InitializationLedger {
        inner: inner.clone(),
        successor: RunId::new("rejected-successor"),
        point,
        failed: AtomicBool::new(false),
    });
    let controller = NativeV2PlatformController::new(ledger.clone(), allocator.clone())
        .await
        .assert_value();
    let mut params = resume_params(&source);
    params.successor_run_id = ledger.successor.clone();
    assert!(controller.resume(params.clone()).await.is_err());
    assert!(ledger.failed.load(Ordering::SeqCst), "fault was exercised");
    controller.wait_runtime_settled(&ledger.successor).await;
    let failed = inner
        .get(&ledger.successor)
        .await
        .assert_value()
        .assert_value();
    assert_eq!(
        failed.snapshot.phase,
        crate::v2_run_ledger::RunPhase::Finished
    );
    assert!(matches!(failed.snapshot.terminal,
        Some(TerminalResult::Failed { reason, .. }) if reason.as_str() == "runtime_unavailable"));
    assert_eq!(
        inner.get(&source).await.assert_value().assert_value(),
        before
    );
    assert!(allocator.successor.lock().assert_value().is_none());
    assert!(controller.runtimes.lock().await.is_empty());
    assert!(allocator.claim_controller(&ledger.successor).await.is_ok());
    assert_eq!(allocator.runtime.allocation_count(), 0);
    assert_eq!(driver.starts.load(Ordering::SeqCst), 0);
    assert_eq!(cleanup.exits(), vec![RunRuntimeExit::RuntimeLost]);
    assert!(
        controller
            .status(RunStatusParams {
                run_id: source.clone()
            })
            .await
            .assert_value()
            .workspace_recovery
            .recoverable
    );
    if !suspended {
        return;
    }

    params.successor_run_id = RunId::new("retry-successor");
    let resumed = controller.resume(params).await.assert_value();
    assert!(matches!(
        terminal(&controller, &resumed.run_id).await,
        TerminalResult::Succeeded { .. }
    ));
    let vectors = controller
        .vectors(RunVectorsParams {
            run_id: resumed.run_id,
            after: None,
            limit: None,
        })
        .await
        .assert_value()
        .vectors;
    assert_eq!(
        vectors.len(),
        2,
        "retry inherits once, then records continuation"
    );
    assert!(matches!(
        vectors[1].content,
        RunVectorContent::Continuation { .. }
    ));
    assert_eq!(
        driver.starts.load(Ordering::SeqCst),
        1,
        "settled worker is not repeated"
    );
}

#[tokio::test]
async fn vector_copy_failure_settles_successor_without_claiming_suspended_source() {
    rejected_initialization_leaves_source_resumable(FailurePoint::Vector, true).await;
}

#[tokio::test]
async fn prerequisite_failure_settles_successor_without_claiming_suspended_source() {
    rejected_initialization_leaves_source_resumable(FailurePoint::PriorExecution, true).await;
}

#[tokio::test]
async fn reload_failure_settles_successor_without_claiming_suspended_source() {
    rejected_initialization_leaves_source_resumable(FailurePoint::Reload, true).await;
}

#[tokio::test]
async fn version_failure_settles_successor_without_claiming_suspended_source() {
    rejected_initialization_leaves_source_resumable(FailurePoint::Version, true).await;
}

#[tokio::test]
async fn failed_run_resume_initialization_also_has_supervised_settlement() {
    for point in [
        FailurePoint::Vector,
        FailurePoint::Reload,
        FailurePoint::Version,
    ] {
        rejected_initialization_leaves_source_resumable(point, false).await;
    }
}

#[tokio::test]
async fn fresh_run_version_failure_also_releases_preparation_authority() {
    let driver = Arc::new(FakeDriver::new(Behavior::Complete));
    let (inner, cleanup, allocator) = runtime_parts(driver.clone());
    let request = request(Value::Null);
    let run_id = request.run_id.clone();
    let ledger = Arc::new(InitializationLedger {
        inner,
        successor: run_id.clone(),
        point: FailurePoint::Version,
        failed: AtomicBool::new(false),
    });
    let controller = NativeV2PlatformController::new(ledger.clone(), allocator.clone())
        .await
        .assert_value();
    let _ = submit_test_request(&controller, request).await;
    controller.wait_runtime_settled(&run_id).await;
    assert!(ledger.failed.load(Ordering::SeqCst));
    assert!(matches!(terminal(&controller, &run_id).await,
        TerminalResult::Failed { reason, .. } if reason.as_str() == "runtime_unavailable"));
    assert!(controller.runtimes.lock().await.is_empty());
    assert!(allocator.claim_controller(&run_id).await.is_ok());
    assert_eq!(driver.starts.load(Ordering::SeqCst), 0);
    assert_eq!(cleanup.exits(), vec![RunRuntimeExit::RuntimeLost]);
}
