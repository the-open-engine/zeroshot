use super::*;

use std::sync::atomic::AtomicBool;

use openengine_cluster_protocol::Cursor;
use openengine_cluster_testkit::assertions::AssertAt;
use crate::v2_run_ledger::{AppendResult, SnapshotAndTail};

const RETAINED_OUTPUT: &str = "retained before storage failure";
const UNPERSISTED_OUTPUT: &str = "output rejected by storage";

#[derive(Clone, Copy)]
enum FailurePoint {
    Output,
    Completion,
    CompletionPanic,
    PeerCompletionPanic,
}

struct FaultLedger {
    inner: Arc<FakeRunLedger>,
    point: FailurePoint,
    permanent: bool,
    lose_reads: bool,
    armed: AtomicBool,
    triggered: AtomicBool,
    peer: Option<Arc<PeerPanicGate>>,
}

impl FaultLedger {
    fn new(
        inner: Arc<FakeRunLedger>,
        point: FailurePoint,
        permanent: bool,
        lose_reads: bool,
    ) -> Self {
        Self {
            inner,
            point,
            permanent,
            lose_reads,
            armed: AtomicBool::new(false),
            triggered: AtomicBool::new(false),
            peer: None,
        }
    }

    fn read_available(&self) -> Result<(), RunLedgerError> {
        if self.lose_reads && self.triggered.load(Ordering::SeqCst) {
            Err(sqlite_failure())
        } else {
            Ok(())
        }
    }

    fn matches(&self, events: &[RunEvent]) -> bool {
        events.iter().any(|event| match self.point {
            FailurePoint::Output => matches!(event, RunEvent::SafeLog { .. }),
            FailurePoint::Completion | FailurePoint::CompletionPanic => {
                matches!(event, RunEvent::NodeCompleted { .. })
            }
            FailurePoint::PeerCompletionPanic => matches!(
                event, RunEvent::NodeCompleted { completion }
                    if completion.reference.node.as_str() == "left"
            ),
        })
    }
}

fn sqlite_failure() -> RunLedgerError {
    RunLedgerError::SqliteStorage(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL))
}

#[async_trait]
impl RunLedger for FaultLedger {
    async fn create_or_get(&self, request: CreateRun) -> Result<CreateRunOutcome, RunLedgerError> {
        self.inner.create_or_get(request).await
    }

    async fn get(&self, run_id: &RunId) -> Result<Option<StoredRun>, RunLedgerError> {
        self.read_available()?;
        self.inner.get(run_id).await
    }

    async fn get_by_submission_key(
        &self,
        key: &IdempotencyKey,
    ) -> Result<Option<StoredRun>, RunLedgerError> {
        self.read_available()?;
        self.inner.get_by_submission_key(key).await
    }

    async fn list(&self) -> Result<Vec<RunSummary>, RunLedgerError> {
        self.read_available()?;
        self.inner.list().await
    }

    async fn append(
        &self,
        run_id: &RunId,
        events: Vec<RunEvent>,
    ) -> Result<AppendResult, RunLedgerError> {
        if let Some(peer) = &self.peer {
            if events.iter().any(|event| {
                matches!(
                    event, RunEvent::SafeLog { line, .. } if line.as_str() == HELD_PEER_OUTPUT
                )
            }) {
                peer.output_started.notify_one();
                peer.release_output.notified().await;
            }
        }
        if self.permanent && self.triggered.load(Ordering::SeqCst) {
            return Err(sqlite_failure());
        }
        if self.armed.load(Ordering::SeqCst)
            && self.matches(&events)
            && !self.triggered.swap(true, Ordering::SeqCst)
        {
            assert!(
                !matches!(
                    self.point,
                    FailurePoint::CompletionPanic | FailurePoint::PeerCompletionPanic
                ),
                "synthetic private panic payload"
            );
            return Err(sqlite_failure());
        }
        self.inner.append(run_id, events).await
    }

    async fn request_force_stop(&self, run_id: &RunId) -> Result<AppendResult, RunLedgerError> {
        if self.permanent && self.triggered.load(Ordering::SeqCst) {
            return Err(sqlite_failure());
        }
        self.inner.request_force_stop(run_id).await
    }

    async fn snapshot_and_tail(
        &self,
        run_id: &RunId,
        after: Option<&Cursor>,
    ) -> Result<SnapshotAndTail, RunLedgerError> {
        self.read_available()?;
        self.inner.snapshot_and_tail(run_id, after).await
    }
}

struct FaultDriver {
    point: FailurePoint,
    release: watch::Sender<bool>,
    cancelled: AtomicBool,
    completed: AtomicBool,
}

impl FaultDriver {
    fn new(point: FailurePoint) -> Self {
        let (release, _) = watch::channel(false);
        Self {
            point,
            release,
            cancelled: AtomicBool::new(false),
            completed: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl NodeDriver for FaultDriver {
    async fn run(
        &self,
        invocation: DriverInvocation,
        mut control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        control
            .emit(LiveOutput::new(LiveOutputStream::Output, RETAINED_OUTPUT)?)
            .await?;
        let mut release = self.release.subscribe();
        while !*release.borrow_and_update() {
            release
                .changed()
                .await
                .map_err(|_| NodeRunnerError::Driver)?;
        }
        if matches!(self.point, FailurePoint::Output) {
            control
                .emit(LiveOutput::new(
                    LiveOutputStream::Output,
                    UNPERSISTED_OUTPUT,
                )?)
                .await?;
            // No additional output or natural completion can reveal the bridge error.
            control.cancelled().await;
            self.cancelled.store(true, Ordering::SeqCst);
            Err(NodeRunnerError::Cancelled)
        } else {
            self.completed.store(true, Ordering::SeqCst);
            Ok(fake_worker_outcome(&invocation))
        }
    }
}

struct FailureHarness {
    controller: NativeV2CloudController,
    ledger: Arc<FaultLedger>,
    driver: Arc<FaultDriver>,
    cleanup: Arc<FakeCleanup>,
    before: RunStatusResult,
}

impl FailureHarness {
    async fn new(point: FailurePoint, permanent: bool, lose_reads: bool) -> Self {
        let inner = Arc::new(FakeRunLedger::new());
        let ledger = Arc::new(FaultLedger::new(
            inner.clone(),
            point,
            permanent,
            lose_reads,
        ));
        let driver = Arc::new(FaultDriver::new(point));
        let cleanup = Arc::new(FakeCleanup::new(inner));
        let allocator = Arc::new(FakeAllocator::new(driver.clone(), cleanup.clone()));
        let controller = NativeV2CloudController::new(ledger.clone(), allocator)
            .await
            .assert_value_with("fault-injection controller startup");
        let receipt = submit_test_request(&controller, request(Value::Null))
            .await
            .assert_value_with("run admitted before fault");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let tail = ledger
                    .inner
                    .snapshot_and_tail(&receipt.run_id, None)
                    .await
                    .assert_value_with("retained tail");
                if tail.events.iter().any(|event| {
                    matches!(
                        &event.event,
                        RunEvent::SafeLog { line, .. } if line.as_str() == RETAINED_OUTPUT
                    )
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .assert_value_with("initial output persisted");
        let before = controller
            .status(RunStatusParams {
                run_id: receipt.run_id,
            })
            .await
            .assert_value_with("active status before fault");
        assert!(matches!(before.status, RunStatus::Running { .. }));
        Self {
            controller,
            ledger,
            driver,
            cleanup,
            before,
        }
    }

    fn release_fault(&self) {
        self.ledger.armed.store(true, Ordering::SeqCst);
        self.driver.release.send_replace(true);
    }

    async fn failed(&self) -> RunStatusResult {
        assert_eq!(
            terminal(&self.controller, &self.before.run_id).await,
            TerminalResult::Failed {
                reason: EnumLabel::new("runtime_failed").assert_value_with("failure reason"),
            }
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !self
                    .controller
                    .runtimes
                    .lock()
                    .await
                    .contains_key(&self.before.run_id)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .assert_value_with("runtime removed after explicit failure");
        assert!(self.ledger.triggered.load(Ordering::SeqCst));
        self.controller
            .status(RunStatusParams {
                run_id: self.before.run_id.clone(),
            })
            .await
            .assert_value_with("failed status survives engine removal")
    }

    async fn retained(&self) -> SnapshotAndTail {
        self.ledger
            .inner
            .snapshot_and_tail(&self.before.run_id, None)
            .await
            .assert_value_with("retained durable history")
    }

    fn assert_storage_diagnostic(&self) {
        let diagnostics = self
            .controller
            .operator_diagnostics
            .snapshot(&self.before.run_id);
        let diagnostic = diagnostics
            .diagnostics
            .first()
            .assert_value_with("operator diagnostic");
        assert_eq!(diagnostic.code, "runtime_failed");
        assert!(diagnostic.stderr.contains(&sqlite_failure().to_string()));
    }

    async fn assert_history_unavailable(&self) {
        let context = ConnectionContext::default();
        let watch_error = ClusterBackend::run_watch(
            &self.controller,
            &context,
            RunWatchParams {
                run_id: self.before.run_id.clone(),
                from_cursor: None,
            },
        )
        .await
        .err()
        .assert_value_with("watch cannot establish with unreadable history");
        assert_eq!(watch_error.code, "SOURCE_UNAVAILABLE");
        let logs_error = ClusterBackend::run_logs(
            &self.controller,
            &context,
            RunLogsParams {
                run_id: self.before.run_id.clone(),
                from_cursor: None,
                execution: None,
            },
        )
        .await
        .err()
        .assert_value_with("logs cannot establish with unreadable history");
        assert_eq!(logs_error.code, "SOURCE_UNAVAILABLE");
    }

    async fn streams(&self) -> (RunWatchEventStream, RunLogEventStream) {
        let context = ConnectionContext::default();
        let (_, watch) = ClusterBackend::run_watch(
            &self.controller,
            &context,
            RunWatchParams {
                run_id: self.before.run_id.clone(),
                from_cursor: None,
            },
        )
        .await
        .assert_value_with("watch before fault");
        let (_, logs) = ClusterBackend::run_logs(
            &self.controller,
            &context,
            RunLogsParams {
                run_id: self.before.run_id.clone(),
                from_cursor: None,
                execution: None,
            },
        )
        .await
        .assert_value_with("logs before fault");
        (watch, logs)
    }
}

async fn drain<E>(
    mut stream: RunSubscriptionStream<E>,
    expected_close: SubscriptionCloseReason,
) -> Vec<E> {
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut events = Vec::new();
        loop {
            match stream
                .next()
                .await
                .assert_value_with("explicit stream closure")
            {
                RunSubscriptionItem::Event(event) => events.push(event),
                RunSubscriptionItem::Closed { reason } => {
                    assert_eq!(reason, expected_close);
                    break;
                }
            }
        }
        assert!(stream.next().await.is_none());
        events
    })
    .await
    .assert_value_with("durable stream terminates")
}

#[tokio::test]
async fn output_append_failure_cancels_a_still_running_handle_and_persists_failure_after_cleanup() {
    let harness = FailureHarness::new(FailurePoint::Output, false, false).await;
    harness.release_fault();
    harness.failed().await;
    assert!(harness.driver.cancelled.load(Ordering::SeqCst));
    assert!(!harness.driver.completed.load(Ordering::SeqCst));
    assert_eq!(harness.cleanup.exits(), vec![RunRuntimeExit::RuntimeLost]);
    assert_eq!(harness.cleanup.terminal_seen(), vec![false]);
    let tail = harness.retained().await;
    assert!(tail.snapshot.active_executions().next().is_none());
    assert!(matches!(
        tail.events.last().map(|event| &event.event),
        Some(RunEvent::Terminal { .. })
    ));
    assert!(!tail.events.iter().any(|event| matches!(
        &event.event, RunEvent::SafeLog { line, .. } if line.as_str() == UNPERSISTED_OUTPUT
    )));
    harness.assert_storage_diagnostic();
}

#[tokio::test]
async fn completion_append_failure_after_normal_driver_exit_is_observable_and_durable() {
    let harness = FailureHarness::new(FailurePoint::Completion, false, false).await;
    harness.release_fault();
    let status = harness.failed().await;
    assert!(harness.driver.completed.load(Ordering::SeqCst));
    assert_eq!(harness.cleanup.terminal_seen(), vec![false]);
    let stored = harness.retained().await;
    assert_eq!(stored.snapshot.cursor, status.at_cursor);
    assert_eq!(
        stored.snapshot.terminal,
        Some(TerminalResult::Failed {
            reason: EnumLabel::new("runtime_failed").assert_value_with("failure reason"),
        })
    );
    harness.assert_storage_diagnostic();
}

#[tokio::test]
async fn permanent_storage_failure_preserves_status_and_reports_stream_completeness() {
    for (lose_reads, close) in [
        (false, SubscriptionCloseReason::Done),
        (true, SubscriptionCloseReason::SourceUnavailable),
    ] {
        let harness = FailureHarness::new(FailurePoint::Output, true, lose_reads).await;
        let (watch, logs) = harness.streams().await;
        harness.release_fault();
        let status = harness.failed().await;
        assert!(harness.driver.cancelled.load(Ordering::SeqCst));
        assert_eq!(
            harness.ledger.get(&harness.before.run_id).await.is_err(),
            lose_reads
        );
        assert_eq!(
            status,
            RunStatusResult {
                status: status.status.clone(),
                ..harness.before.clone()
            }
        );
        let retained = harness.retained().await;
        assert_eq!(retained.snapshot.cursor, harness.before.at_cursor);
        assert!(retained.snapshot.terminal.is_none());
        let watch = drain(watch, close).await;
        assert!(!watch.is_empty());
        assert!(
            watch
                .iter()
                .all(|event| !matches!(event.status, RunStatus::Finished { .. }))
        );
        let logs = drain(logs, close).await;
        assert_eq!(logs.len(), 1);
        assert_eq!(logs.assert_at(0).cursor, harness.before.at_cursor);
        assert_eq!(logs.assert_at(0).record.message.as_str(), RETAINED_OUTPUT);
        harness.assert_storage_diagnostic();
        if lose_reads {
            harness.assert_history_unavailable().await;
        }
    }
}

#[tokio::test]
async fn supervisor_panic_reports_failure_without_rendering_the_panic_payload() {
    let harness = FailureHarness::new(FailurePoint::CompletionPanic, false, false).await;
    harness.release_fault();
    harness.failed().await;
    assert!(harness.driver.completed.load(Ordering::SeqCst));
    assert_eq!(harness.cleanup.terminal_seen(), vec![false]);
    let diagnostics = harness
        .controller
        .operator_diagnostics
        .snapshot(&harness.before.run_id);
    let diagnostic = diagnostics
        .diagnostics
        .first()
        .assert_value_with("panic diagnostic");
    assert_eq!(diagnostic.code, "runtime_failed");
    assert!(diagnostic.stderr.contains("supervisor task panicked"));
    assert!(
        !diagnostic
            .stderr
            .contains("synthetic private panic payload")
    );
}

#[tokio::test]
async fn failed_cleanup_keeps_failure_visible_without_claiming_a_durable_terminal() {
    let harness = FailureHarness::new(FailurePoint::Completion, false, false).await;
    harness.cleanup.fail_next();
    harness.release_fault();
    harness.failed().await;
    assert_eq!(harness.cleanup.exits(), vec![RunRuntimeExit::RuntimeLost]);
    assert_eq!(harness.cleanup.terminal_seen(), vec![false]);
    let stored = harness.retained().await;
    assert!(stored.snapshot.terminal.is_none());
    harness.assert_storage_diagnostic();
}

const HELD_PEER_OUTPUT: &str = "peer output held during supervisor panic";

#[derive(Default)]
struct PeerPanicGate {
    output_started: tokio::sync::Notify,
    release_output: tokio::sync::Notify,
    cancelled: tokio::sync::Notify,
}

struct PeerPanicDriver {
    graph: GraphDriver,
    gate: Arc<PeerPanicGate>,
}

#[async_trait]
impl NodeDriver for PeerPanicDriver {
    async fn run(
        &self,
        invocation: DriverInvocation,
        mut control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        match invocation.node.reference.node.as_str() {
            "left" => self.gate.output_started.notified().await,
            "right" => {
                control
                    .emit(LiveOutput::new(LiveOutputStream::Output, HELD_PEER_OUTPUT)?)
                    .await?;
                control.cancelled().await;
                self.gate.cancelled.notify_one();
                return Err(NodeRunnerError::Cancelled);
            }
            _ => {}
        }
        self.graph.run(invocation, control).await
    }
}

#[tokio::test]
async fn supervisor_panic_waits_for_peer_output_before_cleanup_and_durable_failure() {
    let inner = Arc::new(FakeRunLedger::new());
    let gate = Arc::new(PeerPanicGate::default());
    let mut ledger = FaultLedger::new(
        inner.clone(),
        FailurePoint::PeerCompletionPanic,
        false,
        false,
    );
    ledger.peer = Some(gate.clone());
    ledger.armed.store(true, Ordering::SeqCst);
    let ledger = Arc::new(ledger);
    let cleanup = Arc::new(FakeCleanup::new(inner.clone()));
    let driver = Arc::new(PeerPanicDriver {
        graph: GraphDriver::default(),
        gate: gate.clone(),
    });
    let allocator = Arc::new(FakeAllocator::new(driver, cleanup.clone()));
    let controller = NativeV2CloudController::new(ledger.clone(), allocator)
        .await
        .assert_value_with("peer panic controller startup");
    let receipt = submit_test_request(&controller, complex_request())
        .await
        .assert_value_with("parallel graph admitted");

    tokio::time::timeout(Duration::from_secs(2), gate.cancelled.notified())
        .await
        .assert_value_with("panic cancels the still-running peer");
    assert!(ledger.triggered.load(Ordering::SeqCst));
    // Provider cancellation is acknowledged, but its durable write remains deliberately held.
    // A detached bridge would allow cleanup to race ahead of this write.
    let premature_cleanup = tokio::time::timeout(Duration::from_millis(25), async {
        while cleanup.exits().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        premature_cleanup.is_err(),
        "cleanup must await owned output"
    );
    gate.release_output.notify_one();

    assert_eq!(
        terminal(&controller, &receipt.run_id).await,
        TerminalResult::Failed {
            reason: EnumLabel::new("runtime_failed").assert_value_with("failure reason"),
        }
    );
    assert_eq!(cleanup.exits(), vec![RunRuntimeExit::RuntimeLost]);
    assert_eq!(cleanup.terminal_seen(), vec![false]);
    let retained = inner
        .snapshot_and_tail(&receipt.run_id, None)
        .await
        .assert_value_with("peer output and failure persisted");
    let peer_output = retained
        .events
        .iter()
        .position(|event| {
            matches!(
                &event.event, RunEvent::SafeLog { line, .. } if line.as_str() == HELD_PEER_OUTPUT
            )
        })
        .assert_value_with("held peer output drained");
    let terminal_event = retained
        .events
        .iter()
        .position(|event| matches!(event.event, RunEvent::Terminal { .. }))
        .assert_value_with("durable terminal follows drained output");
    assert!(peer_output < terminal_event);
    assert_eq!(terminal_event + 1, retained.events.len());
    assert!(retained.snapshot.active_executions().next().is_none());
}
