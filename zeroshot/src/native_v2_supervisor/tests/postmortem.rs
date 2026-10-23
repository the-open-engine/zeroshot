use super::*;
use crate::active_memory::postmortem::{
    PostmortemError, PostmortemReport, PostmortemRequest, RunPostmortem,
};
use std::sync::atomic::AtomicBool;

enum Mode {
    Fails,
    Unconfirmed,
    Wait,
    Panics,
}

struct Postmortem {
    mode: Mode,
    started: tokio::sync::Notify,
    calls: AtomicUsize,
    drained: AtomicBool,
}

impl Postmortem {
    fn new(mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            started: tokio::sync::Notify::new(),
            calls: AtomicUsize::new(0),
            drained: AtomicBool::new(false),
        })
    }
}

#[async_trait]
impl RunPostmortem for Postmortem {
    async fn run(
        &self,
        request: PostmortemRequest,
        mut cancellation: watch::Receiver<bool>,
    ) -> Result<PostmortemReport, PostmortemError> {
        assert!(request.snapshot.active_executions().next().is_none());
        assert!(request.snapshot.terminal.is_none());
        let durable = request
            .ledger
            .get(&request.run_id)
            .await
            .assert_value()
            .assert_value();
        let primary = durable.snapshot.primary_graph.as_ref().assert_value();
        assert_eq!(primary.result, request.primary_terminal);
        assert!(primary.auxiliary_pending);
        assert!(durable.snapshot.terminal.is_none());
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        match self.mode {
            Mode::Fails => Err(PostmortemError::Unavailable(
                "test postmortem provider unavailable",
            )),
            Mode::Unconfirmed => Err(PostmortemError::CleanupUnconfirmed),
            Mode::Panics => panic!("test postmortem panic"),
            Mode::Wait => {
                while !*cancellation.borrow_and_update() {
                    if cancellation.changed().await.is_err() {
                        break;
                    }
                }
                self.drained.store(true, Ordering::Release);
                Ok(PostmortemReport {
                    cancelled: true,
                    ..Default::default()
                })
            }
        }
    }
}

async fn with_postmortem(postmortem: Arc<Postmortem>) -> Harness {
    let mut harness = harness(
        resolution::worker_graph(None),
        Value::Null,
        FakeDriver::default(),
    )
    .await;
    harness.supervisor = harness.supervisor.with_postmortem(Some(postmortem));
    harness
}

#[tokio::test]
async fn ordinary_postmortem_failure_or_panic_preserves_primary_outcome_and_does_not_run_twice() {
    for mode in [Mode::Fails, Mode::Panics] {
        let postmortem = Postmortem::new(mode);
        let harness = with_postmortem(postmortem.clone()).await;
        for _ in 0..2 {
            assert!(matches!(
                harness.supervisor.drive().await.assert_value(),
                TerminalResult::Succeeded { .. }
            ));
        }
        assert_eq!(postmortem.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn postmortem_cleanup_uncertainty_blocks_terminal_truth() {
    let harness = with_postmortem(Postmortem::new(Mode::Unconfirmed)).await;
    assert!(matches!(
        harness.supervisor.drive().await,
        Err(NativeV2SupervisorError::CleanupUnconfirmed)
    ));
    assert!(
        stored_run(&harness.ledger)
            .await
            .snapshot
            .terminal
            .is_none()
    );
}

#[tokio::test]
async fn force_stop_and_runtime_loss_during_postmortem_drain_the_tail_and_keep_latched_outcome() {
    for lost in [false, true] {
        let postmortem = Postmortem::new(Mode::Wait);
        let harness = with_postmortem(postmortem.clone()).await;
        let drive = harness.spawn_drive();
        tokio::time::timeout(Duration::from_secs(1), postmortem.started.notified())
            .await
            .assert_value();
        if lost {
            harness.supervisor.runtime_lost().await;
        } else {
            harness.supervisor.force_stop().await.assert_value();
        }
        let result = tokio::time::timeout(Duration::from_secs(1), drive)
            .await
            .assert_value()
            .assert_value()
            .assert_value();
        assert!(matches!(result, TerminalResult::Succeeded { .. }));
        assert!(postmortem.drained.load(Ordering::Acquire));
    }
}

#[tokio::test]
async fn force_stop_before_graph_completion_never_starts_postmortem() {
    let postmortem = Postmortem::new(Mode::Fails);
    let harness = with_postmortem(postmortem.clone()).await;
    harness.supervisor.force_stop().await.assert_value();
    harness.supervisor.drive().await.assert_value();
    assert_eq!(postmortem.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failed_graph_freezes_diagnostics_before_postmortem_and_preserves_them_at_terminal() {
    let postmortem = Postmortem::new(Mode::Fails);
    let mut harness = harness(
        graph(
            sequence(
                vec![
                    step("worker", 1_000),
                    json!({"kind":"fail","name":"failed","reason":"work_rejected"}),
                ],
                null_type(),
            ),
            null_type(),
        ),
        Value::Null,
        FakeDriver::scripted([(
            "worker",
            vec![Behavior::Reported {
                message: "provider refused the primary work",
                result: Ok(WorkerOutcome::declared_failure(WorkerErrorCode::Refusal)),
            }],
        )]),
    )
    .await;
    harness.supervisor = harness.supervisor.with_postmortem(Some(postmortem.clone()));
    let result = harness.supervisor.drive().await.assert_value();
    let TerminalResult::Failed {
        context: Some(context),
        ..
    } = &result
    else {
        panic!("primary failure context was lost");
    };
    assert_eq!(context.recorded.len(), 1);
    assert!(
        context.recorded[0]
            .message
            .as_str()
            .contains("provider refused the primary work")
    );
    let snapshot = stored_run(&harness.ledger).await.snapshot;
    assert_eq!(snapshot.primary_graph.assert_value().result, result);
    assert_eq!(snapshot.terminal, Some(result));
    assert_eq!(postmortem.calls.load(Ordering::SeqCst), 1);
}
