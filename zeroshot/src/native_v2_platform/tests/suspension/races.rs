use super::*;
use crate::v2_run_ledger::{AppendResult, SnapshotAndTail};
use futures_util::FutureExt;
use openengine_cluster_protocol::Cursor;

struct SuspensionLedger {
    inner: Arc<FakeRunLedger>,
    suspended: tokio::sync::Notify,
    suspension_release: watch::Sender<bool>,
    force_entered: tokio::sync::Notify,
    force_release: watch::Sender<bool>,
}

async fn wait_released(release: &watch::Sender<bool>) {
    release
        .subscribe()
        .wait_for(|released| *released)
        .await
        .assert_value();
}

#[async_trait]
impl RunLedger for SuspensionLedger {
    async fn create_or_get(&self, request: CreateRun) -> Result<CreateRunOutcome, RunLedgerError> {
        self.inner.create_or_get(request).await
    }

    async fn get(&self, run_id: &RunId) -> Result<Option<StoredRun>, RunLedgerError> {
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
        let suspension = events
            .iter()
            .any(|event| matches!(event, RunEvent::Suspended { .. }));
        let appended = self.inner.append(run_id, events).await?;
        if suspension {
            // The durable status is visible while the engine still owns its driving turn.
            self.suspended.notify_one();
            wait_released(&self.suspension_release).await;
        }
        Ok(appended)
    }

    async fn request_force_stop(&self, run_id: &RunId) -> Result<AppendResult, RunLedgerError> {
        self.force_entered.notify_one();
        wait_released(&self.force_release).await;
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

struct SuspendedEngineHarness {
    controller: NativeV2PlatformController,
    ledger: Arc<SuspensionLedger>,
    allocator: Arc<SuspensionAllocator>,
    cleanup: Arc<FakeCleanup>,
    source: RunId,
}

impl SuspendedEngineHarness {
    async fn new() -> Self {
        let driver = Arc::new(FakeDriver::new(Behavior::Complete));
        let (inner, cleanup, runtime) = runtime_parts(driver);
        let ledger = Arc::new(SuspensionLedger {
            inner,
            suspended: tokio::sync::Notify::new(),
            suspension_release: watch::channel(false).0,
            force_entered: tokio::sync::Notify::new(),
            force_release: watch::channel(false).0,
        });
        let clock = Arc::new(AtomicU64::new(1_762_905_600_000));
        let read_clock = clock.clone();
        let allocator = Arc::new(SuspensionAllocator {
            runtime,
            store: Arc::new(SuspensionStore::default()),
            successor: StdMutex::new(None),
            preparation_gate: Some(watch::channel(false).0),
            policy: crate::native_v2_wait::WaitPolicy {
                in_place_threshold: Some(Duration::from_secs(3600)),
                wall_clock: Arc::new(move || {
                    UnixTimestampMillis::new(read_clock.load(Ordering::SeqCst)).assert_value()
                }),
            },
        });
        let controller = NativeV2PlatformController::new_with_delivery_policy(
            ledger.clone(),
            allocator.clone(),
            DeliveryPolicy::Optional,
        )
        .await
        .assert_value();
        let source = submit_test_request(&controller, wait_request())
            .await
            .assert_value()
            .run_id;
        ledger.suspended.notified().await;
        clock.fetch_add(7_200_000, Ordering::SeqCst);
        Self {
            controller,
            ledger,
            allocator,
            cleanup,
            source,
        }
    }
}

#[tokio::test]
async fn live_force_does_not_wait_for_unrelated_submission_ownership() {
    let harness = harness(Behavior::Hang).await;
    let source = submit_test_request(&harness.controller, request(Value::Null))
        .await
        .assert_value()
        .run_id;
    harness.driver.wait_started().await;
    let _submission = harness.controller.submission_turn.lock().await;
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        harness.controller.force(RunForceParams { run_id: source }),
    )
    .await
    .assert_value_with("live cancellation stays independent of unrelated submissions")
    .assert_value();
    assert!(matches!(result.status,
        RunStatus::Finished { terminal_result: TerminalResult::Failed { reason, .. }, .. }
            if reason.as_str() == "force_stopped"));
}

#[tokio::test]
async fn suspended_engine_force_reserves_ownership_while_persistence_is_pending() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let harness = SuspendedEngineHarness::new().await;
        let controller = harness.controller.clone();
        let run_id = harness.source.clone();
        let force = tokio::spawn(async move { controller.force(RunForceParams { run_id }).await });
        harness.ledger.force_entered.notified().await;
        harness.ledger.suspension_release.send_replace(true);
        harness
            .controller
            .wait_runtime_settled(&harness.source)
            .await;

        let resume = harness.controller.resume(resume_params(&harness.source));
        tokio::pin!(resume);
        assert!(futures_util::poll!(&mut resume).is_pending());
        assert_eq!(harness.ledger.list().await.assert_value().len(), 1);
        assert!(harness.allocator.successor.lock().assert_value().is_none());

        harness.ledger.force_release.send_replace(true);
        let forced = force.await.assert_value().assert_value();
        assert!(matches!(forced.status,
            RunStatus::Finished { terminal_result: TerminalResult::Failed { reason, .. }, .. }
                if reason.as_str() == "force_stopped"));
        assert!(resume.await.is_err());
        assert_eq!(harness.ledger.list().await.assert_value().len(), 1);
        assert_eq!(
            harness.cleanup.exits(),
            vec![RunRuntimeExit::Suspended, RunRuntimeExit::ForceStopped]
        );
    })
    .await
    .assert_value();
}

#[tokio::test]
async fn accepted_continuation_fences_a_previously_captured_suspended_engine() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let harness = SuspendedEngineHarness::new().await;
        // Resume owns the source's control turn while it waits for its engine to settle.
        let resume = harness.controller.resume(resume_params(&harness.source));
        tokio::pin!(resume);
        assert!(futures_util::poll!(&mut resume).is_pending());
        let force = harness.controller.force(RunForceParams {
            run_id: harness.source.clone(),
        });
        tokio::pin!(force);
        assert!(futures_util::poll!(&mut force).is_pending());
        harness.ledger.suspension_release.send_replace(true);
        let successor = resume.await.assert_value();
        let before = harness
            .controller
            .status(RunStatusParams {
                run_id: harness.source.clone(),
            })
            .await
            .assert_value();
        harness.ledger.force_release.send_replace(true);
        let forced = force.await.assert_value();
        assert_eq!(forced.status, before.status);
        assert_eq!(forced.at_cursor, before.at_cursor);
        let after = harness
            .controller
            .status(RunStatusParams {
                run_id: harness.source.clone(),
            })
            .await
            .assert_value();
        assert_eq!(after, before);
        assert_eq!(
            forced.workspace_recovery.successor_run_id,
            Some(successor.run_id.clone())
        );
        assert!(matches!(forced.status, RunStatus::Suspended { .. }));
        assert!(
            harness
                .ledger
                .force_entered
                .notified()
                .now_or_never()
                .is_none()
        );
        assert_eq!(harness.cleanup.exits(), vec![RunRuntimeExit::Suspended]);
        // Settle the deliberately held successor preparation before leaving the test.
        harness
            .controller
            .force(RunForceParams {
                run_id: successor.run_id,
            })
            .await
            .assert_value();
    })
    .await
    .assert_value();
}
