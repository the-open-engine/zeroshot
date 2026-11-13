use super::*;
use openengine_cluster_protocol::{RunSuspension, UnixTimestampMillis};
use std::sync::atomic::AtomicU64;

#[path = "suspension/publication.rs"]
mod publication;

fn wait_node() -> Value {
    json!({"kind":"verifier","name":"wait","worker":crate::native_v2_wait::WAIT_WORKER_REF,
        "input":crate::native_v2_wait::input_schema().assert_value(),"output":{"kind":"null"},
        "signals":crate::native_v2_wait::signal_labels().assert_value(),
        "diagnostic":crate::native_v2_wait::diagnostic_schema().assert_value(),
        "inputBindings":[{"target":["durationMs"],"value":{"source":"state","path":["durationMs"]}}],
        "writeBindings":[],"attempts":1})
}

async fn waiting(threshold: Option<Duration>) -> (Harness, Arc<AtomicU64>) {
    waiting_duration(threshold, 3 * 24 * 60 * 60 * 1000).await
}

async fn waiting_duration(threshold: Option<Duration>, duration: i64) -> (Harness, Arc<AtomicU64>) {
    let state = json!({"kind":"record","fields":{"durationMs":{"required":true,"type":{"kind":"integer"}}}});
    let graph = graph(
        sequence(
            vec![wait_node(), step("after", 10000), succeed("done")],
            state.clone(),
        ),
        state,
    );
    let mut harness = harness(graph, json!({"durationMs":duration}), FakeDriver::default()).await;
    let (supervisor, _) = super::checkpoint_tests::RecordingCheckpointStore::attach(&harness);
    let clock = Arc::new(AtomicU64::new(1_762_905_600_000));
    let clock_read = clock.clone();
    harness.supervisor = supervisor
        .with_wait_threshold(threshold)
        .with_wall_clock(Arc::new(move || {
            UnixTimestampMillis::new(clock_read.load(Ordering::SeqCst)).assert_value()
        }));
    (harness, clock)
}

async fn wait_active(harness: &Harness) {
    loop {
        if harness
            .supervisor
            .snapshot()
            .await
            .assert_value()
            .active_executions()
            .any(|execution| execution.reference.node.as_str() == "wait")
        {
            return;
        }
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn long_wait_tracks_wall_clock_and_checks_after_machine_sleep() {
    let (harness, clock) = waiting(None).await;
    let task = harness.spawn_drive();
    wait_active(&harness).await;
    clock.fetch_add(2 * 24 * 60 * 60 * 1000, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(60)).await;
    assert!(!task.is_finished());
    clock.fetch_add(24 * 60 * 60 * 1000, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(60)).await;
    assert!(matches!(
        task.await.assert_value().assert_value(),
        TerminalResult::Succeeded { .. }
    ));
    assert_eq!(harness.driver.starts("wait"), 0);
    assert_eq!(harness.driver.starts("after"), 1);
}

#[tokio::test(start_paused = true)]
async fn threshold_suspends_without_advancing_time() {
    let (harness, clock) = waiting(Some(Duration::from_secs(3600))).await;
    let before = clock.load(Ordering::SeqCst);
    assert!(matches!(
        harness.supervisor.drive().await,
        Err(NativeV2SupervisorError::Suspended)
    ));
    assert_eq!(clock.load(Ordering::SeqCst), before);
    let snapshot = harness.supervisor.snapshot().await.assert_value();
    assert!(
        matches!(snapshot.suspension,Some(RunSuspension::Wait{wait,..}) if wait.due_at.get()==before+3*24*60*60*1000)
    );
    let waiter = snapshot
        .executions
        .values()
        .find(|node| node.reference.node.as_str() == "wait")
        .assert_value();
    assert!(matches!(
        waiter.state,
        crate::v2_run_ledger::NodeState::SuspendedWait { .. }
    ));
    assert!(waiter.outcome().is_none());
    assert_eq!(harness.driver.starts("after"), 0);
}

#[tokio::test(start_paused = true)]
async fn command_interrupts_only_in_place_wait_not_executing_workers() {
    let (harness, _) = waiting(None).await;
    let task = harness.spawn_drive();
    wait_active(&harness).await;
    harness.supervisor.suspend().await.assert_value();
    assert!(matches!(
        task.await.assert_value(),
        Err(NativeV2SupervisorError::Suspended)
    ));
    assert!(matches!(
        harness
            .supervisor
            .snapshot()
            .await
            .assert_value()
            .suspension,
        Some(RunSuspension::Wait { .. })
    ));
}

#[tokio::test(start_paused = true)]
async fn force_stop_cancels_in_place_wait() {
    let (harness, _) = waiting(None).await;
    let task = harness.spawn_drive();
    wait_active(&harness).await;
    harness.supervisor.force_stop().await.assert_value();
    assert!(
        matches!(task.await.assert_value().assert_value(), TerminalResult::Failed{reason,..} if reason.as_str()=="force_stopped")
    );
    assert_eq!(harness.driver.starts("after"), 0);
}

#[tokio::test(start_paused = true)]
async fn suspension_waits_for_entire_parallel_group() {
    let group = json!({"kind":"par","name":"group","state":null_type(),
        "branches":[step("left",10000),step("right",10000)],"join":{"kind":"all"},"promotedStatePaths":[]});
    let mut harness = harness(
        graph(
            sequence(
                vec![group, step("after", 10000), succeed("done")],
                null_type(),
            ),
            null_type(),
        ),
        Value::Null,
        FakeDriver::scripted([
            ("left", vec![super::checkpoint_tests::completed_after(100)]),
            ("right", vec![super::checkpoint_tests::completed_after(200)]),
        ]),
    )
    .await;
    let (supervisor, _) = super::checkpoint_tests::RecordingCheckpointStore::attach(&harness);
    harness.supervisor = supervisor;
    let task = harness.spawn_drive();
    while harness.driver.starts("left") == 0 {
        tokio::task::yield_now().await;
    }
    harness.supervisor.suspend().await.assert_value();
    assert!(matches!(
        task.await.assert_value(),
        Err(NativeV2SupervisorError::Suspended)
    ));
    assert_eq!(harness.driver.starts("right"), 1);
    assert_eq!(harness.driver.starts("after"), 0);
    assert!(
        harness
            .supervisor
            .snapshot()
            .await
            .assert_value()
            .active_executions()
            .next()
            .is_none()
    );
}

#[tokio::test(start_paused = true)]
async fn suspension_waits_for_all_map_waves() {
    let mut harness = harness_with_options(
        super::checkpoint_tests::mapped_waves_graph(),
        json!({"items":["a","b","c","d","e"]}),
        FakeDriver::scripted([(
            "first",
            vec![
                super::checkpoint_tests::completed_after(100),
                super::checkpoint_tests::completed_after(200),
                super::checkpoint_tests::completed_after(300),
                super::checkpoint_tests::completed_after(400),
                super::checkpoint_tests::completed_after(500),
            ],
        )]),
        HarnessOptions {
            max_concurrency: Some(1),
            session_scope: SessionScope::Execution,
            resolver: None,
        },
    )
    .await;
    let (supervisor, _) = super::checkpoint_tests::RecordingCheckpointStore::attach(&harness);
    harness.supervisor = supervisor;
    let task = harness.spawn_drive();
    while harness.driver.starts("first") == 0 {
        tokio::task::yield_now().await;
    }
    harness.supervisor.suspend().await.assert_value();
    assert!(matches!(
        task.await.assert_value(),
        Err(NativeV2SupervisorError::Suspended)
    ));
    assert_eq!(harness.driver.starts("first"), 5);
    assert_eq!(harness.driver.starts("second"), 5);
    assert_eq!(harness.driver.max_active(), 1);
    assert_eq!(harness.driver.starts("after"), 0);
}

#[tokio::test(start_paused = true)]
async fn wait_shorter_than_threshold_stays_in_place() {
    let (harness, clock) = waiting_duration(Some(Duration::from_secs(3600)), 59 * 60 * 1000).await;
    let task = harness.spawn_drive();
    wait_active(&harness).await;
    assert!(
        harness
            .supervisor
            .snapshot()
            .await
            .assert_value()
            .suspension
            .is_none()
    );
    clock.fetch_add(59 * 60 * 1000, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(60)).await;
    assert!(matches!(
        task.await.assert_value().assert_value(),
        TerminalResult::Succeeded { .. }
    ));
    assert_eq!(harness.driver.starts("after"), 1);
}

#[tokio::test(start_paused = true)]
async fn engine_wait_bound_and_immediate_durations_settle_without_provider_work() {
    for duration in [
        -1,
        0,
        crate::native_v2_wait::MAX_WAIT_MILLIS as i64,
        crate::native_v2_wait::MAX_WAIT_MILLIS as i64 + 1,
    ] {
        let (harness, clock) = waiting_duration(None, duration).await;
        let task = harness.spawn_drive();
        if duration == crate::native_v2_wait::MAX_WAIT_MILLIS as i64 {
            wait_active(&harness).await;
            assert!(!task.is_finished());
            clock.fetch_add(duration as u64, Ordering::SeqCst);
            tokio::time::advance(Duration::from_secs(60)).await;
        }
        task.await.assert_value().assert_value();
        let snapshot = harness.supervisor.snapshot().await.assert_value();
        let waiter = snapshot
            .executions
            .values()
            .find(|node| node.reference.node.as_str() == "wait")
            .assert_value();
        assert_eq!(
            matches!(waiter.outcome(), Some(WorkerOutcome::Error { .. })),
            duration > crate::native_v2_wait::MAX_WAIT_MILLIS as i64
        );
        assert_eq!(harness.driver.starts("wait"), 0);
    }
}
