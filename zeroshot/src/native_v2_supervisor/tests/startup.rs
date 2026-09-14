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
}

#[async_trait]
impl NodeRunner for DelayedRunner {
    async fn start(&self, request: NodeRunRequest) -> Result<NodeHandle, NodeRunnerError> {
        self.entered.notify_one();
        self.release.notified().await;
        self.inner.start(request).await
    }

    async fn close_run(&self, run_id: &RunId) {
        self.inner.close_run(run_id).await;
        self.release.notify_one();
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

fn with_delayed_start(harness: &mut Harness) -> Arc<DelayedRunner> {
    let delayed = Arc::new(DelayedRunner {
        inner: harness.supervisor.runner.clone(),
        entered: Notify::new(),
        release: Notify::new(),
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
async fn run_close_during_start_rejects_late_readiness_without_a_handle_or_live_registration() {
    let mut harness = hanging_worker(None).await;
    let (_delayed, registrar, drive) = begin_delayed_start(&mut harness).await;
    force_stop_and_join(&harness, drive).await;
    assert_eq!(harness.driver.starts("worker"), 0);
    assert_eq!(harness.sessions.opened.load(Ordering::SeqCst), 0);
    assert!(registrar.registered.lock().assert_value().is_empty());
    assert_terminal_is_last(&harness).await;
}

#[tokio::test(start_paused = true)]
async fn deadline_during_start_cancels_returned_handle_before_live_registration() {
    let mut harness = hanging_worker(Some(20)).await;
    let (delayed, registrar, drive) = begin_delayed_start(&mut harness).await;
    tokio::time::advance(Duration::from_millis(30)).await;
    delayed.release.notify_one();
    drive.await.assert_value().assert_value();
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
    let delayed = with_delayed_start(harness);
    let registrar = Arc::new(FakeLiveRegistrar::default());
    let drive = drive_with_registrar(harness, registrar.clone());
    delayed.entered.notified().await;
    (delayed, registrar, drive)
}
