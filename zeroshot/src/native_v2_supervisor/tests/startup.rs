use super::*;
use tokio::sync::Notify;

#[derive(Default)]
struct PendingRegistrar {
    entered: Notify,
    dropped: AtomicUsize,
}

#[async_trait]
impl LiveOutputRegistrar for PendingRegistrar {
    async fn register(
        &self,
        _reference: &ExecutionRef,
        _source: LiveOutputSource,
    ) -> Result<Box<dyn LiveOutputRegistration>, LiveOutputUnavailable> {
        let _pending = super::resolution::PendingResolution(&self.dropped);
        self.entered.notify_one();
        std::future::pending().await
    }
}

struct DelayedRunner {
    inner: Arc<dyn NodeRunner>,
    entered: Notify,
    release: Notify,
    dropped: AtomicUsize,
    node: &'static str,
}

#[async_trait]
impl NodeRunner for DelayedRunner {
    async fn start(&self, request: NodeRunRequest) -> Result<NodeHandle, NodeRunnerError> {
        if request.invocation.reference.node.as_str() == self.node {
            let _pending = super::resolution::PendingResolution(&self.dropped);
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.inner.start(request).await
    }

    async fn close_run(&self, run_id: &RunId) {
        self.inner.close_run(run_id).await;
    }
}

async fn hanging_worker(timeout: Option<u64>) -> Harness {
    harness(
        super::resolution::worker_graph(timeout),
        Value::Null,
        FakeDriver::scripted([("worker", vec![Behavior::Hang])]),
    )
    .await
}

fn with_delayed_start(harness: &mut Harness, node: &'static str) -> Arc<DelayedRunner> {
    let delayed = Arc::new(DelayedRunner {
        inner: harness.supervisor.runner.clone(),
        entered: Notify::new(),
        release: Notify::new(),
        dropped: AtomicUsize::new(0),
        node,
    });
    harness.supervisor.runner = delayed.clone();
    delayed
}

#[tokio::test(start_paused = true)]
async fn deadline_during_live_registration_cancels_and_drains_the_started_node() {
    let harness = hanging_worker(Some(20)).await;
    let registrar = Arc::new(PendingRegistrar::default());
    let supervisor = harness
        .supervisor
        .clone()
        .with_live_output(registrar.clone());
    supervisor.drive().await.assert_value();
    assert_eq!(registrar.dropped.load(Ordering::SeqCst), 1);
    assert_eq!(harness.driver.cancellations("worker"), 1);
    let stored = stored_run(&harness.ledger).await;
    let worker = stored.snapshot.executions.values().next().assert_value();
    assert_eq!(
        worker.outcome(),
        Some(&WorkerOutcome::declared_failure(WorkerErrorCode::Timeout))
    );
    assert_eq!(harness.sessions.closed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn force_stop_during_live_registration_has_no_late_registration_or_output() {
    let harness = hanging_worker(None).await;
    let registrar = Arc::new(PendingRegistrar::default());
    let drive = drive_with_registrar(&harness, registrar.clone());
    registrar.entered.notified().await;
    force_stop_and_join(&harness, drive).await;
    assert_eq!(registrar.dropped.load(Ordering::SeqCst), 1);
    assert_eq!(harness.sessions.closed.load(Ordering::SeqCst), 1);
    assert_terminal_is_last(&harness).await;
}

#[tokio::test]
async fn force_stop_and_runtime_loss_cancel_a_never_ready_start_without_late_work() {
    for lost in [false, true] {
        let mut harness = hanging_worker(None).await;
        let (delayed, registrar, drive) = begin_delayed_start(&mut harness).await;
        if lost {
            harness.supervisor.runtime_lost().await;
            tokio::time::timeout(Duration::from_secs(1), drive)
                .await
                .assert_value()
                .assert_value()
                .assert_value();
        } else {
            force_stop_and_join(&harness, drive).await;
        }
        assert_eq!(delayed.dropped.load(Ordering::SeqCst), 1);
        delayed.release.notify_one();
        assert_eq!(harness.driver.starts("worker"), 0);
        assert_eq!(harness.sessions.opened.load(Ordering::SeqCst), 0);
        assert!(registrar.registered.lock().assert_value().is_empty());
        assert_terminal_is_last(&harness).await;
    }
}

#[tokio::test(start_paused = true)]
async fn deadline_cancels_a_never_ready_start_before_live_registration() {
    let mut harness = hanging_worker(Some(20)).await;
    let (delayed, registrar, drive) = begin_delayed_start(&mut harness).await;
    tokio::time::advance(Duration::from_millis(30)).await;
    tokio::time::timeout(Duration::from_secs(1), drive)
        .await
        .assert_value()
        .assert_value()
        .assert_value();
    assert_eq!(delayed.dropped.load(Ordering::SeqCst), 1);
    assert_eq!(harness.driver.starts("worker"), 0);
    assert!(registrar.registered.lock().assert_value().is_empty());
    let stored = stored_run(&harness.ledger).await;
    assert_eq!(
        stored
            .snapshot
            .executions
            .values()
            .next()
            .assert_value()
            .outcome(),
        Some(&WorkerOutcome::declared_failure(WorkerErrorCode::Timeout))
    );
    assert_terminal_is_last(&harness).await;
}

async fn assert_terminal_is_last(harness: &Harness) {
    let tail = harness
        .ledger
        .snapshot_and_tail(&harness.supervisor.run_id, None)
        .await
        .assert_value();
    assert!(matches!(
        tail.events.last().assert_value().event,
        RunEvent::Terminal { .. }
    ));
}

type Driving = tokio::task::JoinHandle<Result<TerminalResult, NativeV2SupervisorError>>;

fn drive_with_registrar(harness: &Harness, registrar: Arc<dyn LiveOutputRegistrar>) -> Driving {
    let supervisor = harness.supervisor.clone().with_live_output(registrar);
    tokio::spawn(async move { supervisor.drive().await })
}

async fn force_stop_and_join(harness: &Harness, drive: Driving) {
    harness.supervisor.force_stop().await.assert_value();
    tokio::time::timeout(Duration::from_secs(1), drive)
        .await
        .assert_value()
        .assert_value()
        .assert_value();
}

async fn begin_delayed_start(
    harness: &mut Harness,
) -> (Arc<DelayedRunner>, Arc<FakeLiveRegistrar>, Driving) {
    let delayed = with_delayed_start(harness, "worker");
    let registrar = Arc::new(FakeLiveRegistrar::default());
    let drive = drive_with_registrar(harness, registrar.clone());
    delayed.entered.notified().await;
    (delayed, registrar, drive)
}

#[tokio::test]
async fn a_never_ready_start_does_not_block_a_parallel_winner_or_its_void() {
    let mut harness = harness(
        parallel(
            json!({"kind": "any"}),
            vec![verifier("slow", 10_000), verifier("fast", 10_000)],
        ),
        Value::Null,
        FakeDriver::default(),
    )
    .await;
    let delayed = with_delayed_start(&mut harness, "slow");
    super::resolution::assert_parallel_winner(&harness).await;
    assert_eq!(delayed.dropped.load(Ordering::SeqCst), 1);
}

#[derive(Default)]
struct CleanupState {
    started: Notify,
    cancelled: Notify,
    release: Notify,
}

struct CleanupRunner(Arc<CleanupState>);

#[async_trait]
impl NodeRunner for CleanupRunner {
    async fn start(&self, request: NodeRunRequest) -> Result<NodeHandle, NodeRunnerError> {
        let (handle, mut bridge) =
            crate::native_v2_runner::remote_node_handle(request.invocation.reference);
        let state = self.0.clone();
        tokio::spawn(async move {
            state.started.notify_one();
            bridge.cancelled().await;
            let usage =
                serde_json::from_value(json!({"inputTokens": 7, "outputTokens": 5})).assert_value();
            bridge.record_token_usage(Some(usage)).await.assert_value();
            state.cancelled.notify_one();
            state.release.notified().await;
            bridge.finish(Err(NodeRunnerError::Cancelled));
        });
        Ok(handle)
    }

    async fn close_run(&self, _run_id: &RunId) {}
}

#[tokio::test(start_paused = true)]
async fn capsule_deadline_preserves_usage_and_waits_for_cleanup_before_settlement() {
    use crate::native_v2_capsule::{NativeCapsuleNodeEndpoint, RemoteCapsuleNodeRunner};

    let mut harness = hanging_worker(Some(20)).await;
    let state = Arc::new(CleanupState::default());
    let endpoint = NativeCapsuleNodeEndpoint::new(Arc::new(CleanupRunner(state.clone())));
    harness.supervisor.runner = Arc::new(RemoteCapsuleNodeRunner::new(Arc::new(endpoint)));
    let supervisor = harness.supervisor.clone();
    let drive = tokio::spawn(async move { supervisor.drive().await });
    state.started.notified().await;
    tokio::time::advance(Duration::from_millis(30)).await;
    state.cancelled.notified().await;
    assert!(!drive.is_finished());
    let pending = stored_run(&harness.ledger).await;
    assert!(pending.snapshot.terminal.is_none());
    assert!(
        pending
            .snapshot
            .executions
            .values()
            .all(|entry| entry.outcome().is_none())
    );
    state.release.notify_one();
    drive.await.assert_value().assert_value();
    let tail = harness
        .ledger
        .snapshot_and_tail(&harness.supervisor.run_id, None)
        .await
        .assert_value();
    let usage = tail
        .events
        .iter()
        .position(|event| {
            matches!(
                &event.event,
                RunEvent::TokenUsageObserved { usage: Some(usage), .. }
                    if usage.input_tokens.get() == 7 && usage.output_tokens.get() == 5
            )
        })
        .assert_value();
    let completion = tail.events.iter().position(|event| matches!(
        &event.event,
        RunEvent::NodeCompleted { completion }
            if completion.outcome == WorkerOutcome::declared_failure(WorkerErrorCode::Timeout)
    )).assert_value();
    assert!(usage < completion);
    assert_terminal_is_last(&harness).await;
}
