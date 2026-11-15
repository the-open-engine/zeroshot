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
    release: tokio::sync::Notify,
    calls: AtomicUsize,
    drained: AtomicBool,
}

impl Postmortem {
    fn new(mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
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
                tokio::select! {
                    () = self.release.notified() => {},
                    () = async {
                        while !*cancellation.borrow_and_update() {
                            if cancellation.changed().await.is_err() {
                                break;
                            }
                        }
                    } => {},
                }
                self.drained.store(true, Ordering::Release);
                Ok(PostmortemReport {
                    cancelled: *cancellation.borrow(),
                    ..Default::default()
                })
            }
        }
    }
}

async fn with_postmortem(postmortem: Arc<Postmortem>) -> Harness {
    with_graph_postmortem(Some(postmortem), false).await
}

async fn with_graph_postmortem(postmortem: Option<Arc<Postmortem>>, failed: bool) -> Harness {
    let graph = if failed {
        graph(
            sequence(
                vec![
                    step("worker", 1_000),
                    json!({"kind":"fail","name":"failed","reason":"work_rejected"}),
                ],
                null_type(),
            ),
            null_type(),
        )
    } else {
        resolution::worker_graph(None)
    };
    let mut harness = harness(graph, Value::Null, FakeDriver::default()).await;
    harness.supervisor = harness
        .supervisor
        .with_postmortem(postmortem.map(|value| value as Arc<dyn RunPostmortem>));
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

#[tokio::test]
async fn a_ready_stop_is_seen_without_polling_optional_postmortem_work() {
    let (stop, _) = watch::channel(false);
    stop.send_replace(true);
    let stopping = stop.subscribe();
    let mut polled = false;
    let work = async {
        polled = true;
        std::future::pending::<()>().await
    };
    tokio::time::timeout(Duration::from_secs(1), async {
        tokio::select! {
            biased;
            () = crate::native_v2_supervisor::postmortem::stop_requested(stopping) => {},
            () = work => panic!("optional work must not run after a ready stop"),
        }
    })
    .await
    .assert_value();
    assert!(!polled);
}

#[tokio::test]
async fn postmortem_skips_rejected_source_credentials_or_model_but_runs_after_crash() {
    for (outcome, skip_reason) in [
        (
            WorkerOutcome::authentication_refusal(),
            Some("authentication_required"),
        ),
        (WorkerOutcome::model_refusal(), Some("model_unavailable")),
        (
            WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
            None,
        ),
        (WorkerOutcome::policy_refusal(), None),
    ] {
        let postmortem = Postmortem::new(Mode::Fails);
        let mut harness = harness(
            resolution::worker_graph(None),
            Value::Null,
            FakeDriver::scripted([(
                "worker",
                vec![Behavior::Complete {
                    delay: Duration::ZERO,
                    outcome,
                }],
            )]),
        )
        .await;
        harness.supervisor = harness.supervisor.with_postmortem(Some(postmortem.clone()));
        let result = harness.supervisor.drive().await.assert_value();
        assert_eq!(
            postmortem.calls.load(Ordering::SeqCst),
            usize::from(skip_reason.is_none())
        );
        let stored = stored_run(&harness.ledger).await;
        assert!(
            !stored
                .snapshot
                .primary_graph
                .as_ref()
                .assert_value()
                .auxiliary_pending
        );
        if let Some(reason) = skip_reason {
            assert!(matches!(
                observed_status(&harness).await,
                openengine_cluster_protocol::RunStatus::Finished { terminal_result, .. }
                    if terminal_result == result
            ));
            let result_logs = graph_result_logs(&harness).await;
            assert_eq!(result_logs.len(), 1);
            assert!(!result_logs[0].contains("postmortem is running"));
            let mut logs = failure_logs::public_subscription(&harness).await;
            let records = logs.read_available().await.assert_value();
            assert!(
                records
                    .iter()
                    .any(|record| record.record.message.as_str().contains(&format!(
                        "postmortem skipped: source node worker ended with {reason}"
                    )))
            );
        }
    }
}

#[tokio::test]
async fn postmortem_gate_uses_the_selected_source_not_an_unrelated_rejected_node() {
    let postmortem = Postmortem::new(Mode::Fails);
    let mut harness = harness(
        graph(
            sequence(
                vec![
                    step("healthy", 1_000),
                    step("rejected", 1_000),
                    succeed("done"),
                ],
                null_type(),
            ),
            null_type(),
        ),
        Value::Null,
        FakeDriver::scripted([(
            "rejected",
            vec![Behavior::Complete {
                delay: Duration::ZERO,
                outcome: WorkerOutcome::model_refusal(),
            }],
        )]),
    )
    .await;
    harness.supervisor = harness.supervisor.with_postmortem(Some(postmortem.clone()));
    harness.supervisor.drive().await.assert_value();
    assert_eq!(postmortem.calls.load(Ordering::SeqCst), 1);
    let stored = stored_run(&harness.ledger).await;
    let mut admitted = stored.admitted;
    admitted.runtime.global_agents_mut().postmortem.source_node =
        Some(NodeName::new("rejected").assert_value());
    assert!(
        harness
            .supervisor
            .enabled_postmortem(&admitted, &stored.snapshot)
            .is_none()
    );
    admitted.runtime.global_agents_mut().postmortem.source_node =
        Some(NodeName::new("healthy").assert_value());
    assert!(
        harness
            .supervisor
            .enabled_postmortem(&admitted, &stored.snapshot)
            .is_some()
    );
}

#[tokio::test]
async fn restored_source_rejections_skip_postmortem_after_an_authored_refusal_branch() {
    for (outcome, reason) in [
        (
            WorkerOutcome::authentication_refusal(),
            Some("authentication_required"),
        ),
        (WorkerOutcome::model_refusal(), Some("model_unavailable")),
        (WorkerOutcome::policy_refusal(), None),
    ] {
        let postmortem = Postmortem::new(Mode::Fails);
        let route = json!({
            "kind":"choice", "name":"handle_refusal", "state":null_type(),
            "promotedStatePaths":[],
            "branches":[{
                "when":{"kind":"in","value":{"name":"worker","source":"error"},"labels":["refusal"]},
                "node":step("recovery", 10_000)
            }],
            "otherwise":{"kind":"fail","name":"unexpected","reason":"unexpected_failure"}
        });
        let mut harness = harness(
            graph(
                sequence(
                    vec![step("worker", 10_000), route, succeed("done")],
                    null_type(),
                ),
                null_type(),
            ),
            Value::Null,
            FakeDriver::default(),
        )
        .await;
        harness.supervisor = harness.supervisor.with_postmortem(Some(postmortem.clone()));
        harness
            .ledger
            .append(
                &harness.supervisor.run_id,
                vec![RunEvent::PriorExecution {
                    execution: checkpoint_tests::prerequisite(1, "worker", outcome),
                }],
            )
            .await
            .assert_value();
        assert!(matches!(
            harness.supervisor.drive().await.assert_value(),
            TerminalResult::Succeeded { .. }
        ));
        assert_eq!(harness.driver.starts("worker"), 0);
        assert_eq!(harness.driver.starts("recovery"), 1);
        assert_eq!(
            postmortem.calls.load(Ordering::SeqCst),
            usize::from(reason.is_none())
        );
        let stored = stored_run(&harness.ledger).await;
        assert_eq!(stored.snapshot.execution_seed.len(), 1);
        assert_eq!(stored.snapshot.executions.len(), 1);
        let mut logs = failure_logs::public_subscription(&harness).await;
        let records = logs.read_available().await.assert_value();
        let skipped = records.iter().find(|record| {
            record
                .record
                .message
                .as_str()
                .contains("postmortem skipped: source node worker")
        });
        if let Some(reason) = reason {
            assert!(
                skipped
                    .assert_value()
                    .record
                    .message
                    .as_str()
                    .contains(reason)
            );
        } else {
            assert!(skipped.is_none());
        }
    }
}

async fn observed_status(harness: &Harness) -> openengine_cluster_protocol::RunStatus {
    crate::native_v2_observability::NativeV2Observability::new(harness.ledger.clone())
        .status(openengine_cluster_protocol::RunStatusParams {
            run_id: harness.supervisor.run_id.clone(),
        })
        .await
        .assert_value()
        .status
}

async fn graph_result_logs(harness: &Harness) -> Vec<String> {
    harness
        .ledger
        .snapshot_and_tail(&harness.supervisor.run_id, None)
        .await
        .assert_value()
        .events
        .into_iter()
        .filter_map(|stored| match stored.event {
            RunEvent::SafeLog { line, .. }
                if line.as_str().starts_with("Run failed:")
                    || line.as_str().starts_with("Run succeeded") =>
            {
                Some(line.as_str().to_owned())
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn graph_result_is_visible_and_logged_while_postmortem_waits_then_only_terminal_remains() {
    use openengine_cluster_protocol::{RunStatus, RunWatchParams};
    for failed in [false, true] {
        let postmortem = Postmortem::new(Mode::Wait);
        let harness = with_graph_postmortem(Some(postmortem.clone()), failed).await;
        let service =
            crate::native_v2_observability::NativeV2Observability::new(harness.ledger.clone());
        let (_, mut watch) = service
            .watch(RunWatchParams {
                run_id: harness.supervisor.run_id.clone(),
                from_cursor: None,
            })
            .await
            .assert_value();
        assert!(matches!(
            observed_status(&harness).await,
            RunStatus::Admitted {}
        ));
        let drive = harness.spawn_drive();
        tokio::time::timeout(Duration::from_secs(5), postmortem.started.notified())
            .await
            .assert_value();
        assert!(!drive.is_finished());
        let result = stored_run(&harness.ledger)
            .await
            .snapshot
            .primary_graph
            .assert_value()
            .result;
        assert_eq!(matches!(result, TerminalResult::Failed { .. }), failed);
        let pending = RunStatus::Running {
            suspend_requested: false,
            active_executions: vec![],
            graph_result: Some(result.clone()),
        };
        assert_eq!(observed_status(&harness).await, pending);
        let events = watch.read_available().await.assert_value();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.status == pending)
                .count(),
            1
        );
        assert!(events.iter().any(|event| matches!(
            event.status,
            RunStatus::Running {
                graph_result: None,
                ..
            }
        )));
        let logs = graph_result_logs(&harness).await;
        assert_eq!(logs.len(), 1);
        assert!(logs[0].contains("Active Memory postmortem is running"));
        postmortem.release.notify_one();
        assert_eq!(drive.await.assert_value().assert_value(), result);
        assert!(
            matches!(observed_status(&harness).await, RunStatus::Finished { terminal_result, .. } if terminal_result == result)
        );
        assert_eq!(graph_result_logs(&harness).await, logs);
        assert!(matches!(
            watch
                .read_available()
                .await
                .assert_value()
                .last()
                .assert_value()
                .status,
            RunStatus::Finished { .. }
        ));
    }
}

#[tokio::test]
async fn force_stop_during_postmortem_keeps_success_and_failure_graph_results_until_finished() {
    use openengine_cluster_protocol::RunStatus;
    for failed in [false, true] {
        let postmortem = Postmortem::new(Mode::Wait);
        let harness = with_graph_postmortem(Some(postmortem.clone()), failed).await;
        let drive = harness.spawn_drive();
        tokio::time::timeout(Duration::from_secs(5), postmortem.started.notified())
            .await
            .assert_value();
        let result = stored_run(&harness.ledger)
            .await
            .snapshot
            .primary_graph
            .assert_value()
            .result;
        let logs = graph_result_logs(&harness).await;
        harness.supervisor.force_stop().await.assert_value();
        assert_eq!(drive.await.assert_value().assert_value(), result);
        assert!(postmortem.drained.load(Ordering::Acquire));
        assert!(
            matches!(observed_status(&harness).await, RunStatus::Finished { terminal_result, .. } if terminal_result == result)
        );
        assert_eq!(graph_result_logs(&harness).await, logs);
    }
}

#[tokio::test]
async fn graphs_without_postmortem_finish_normally_and_log_the_result_once() {
    for failed in [false, true] {
        let harness = with_graph_postmortem(None, failed).await;
        let result = harness.supervisor.drive().await.assert_value();
        assert_eq!(matches!(result, TerminalResult::Failed { .. }), failed);
        let logs = graph_result_logs(&harness).await;
        assert_eq!(logs.len(), 1);
        assert!(!logs[0].contains("postmortem"));
        let value = serde_json::to_value(observed_status(&harness).await).assert_value();
        assert!(value.get("graphResult").is_none());
        assert_eq!(
            value["terminalResult"],
            serde_json::to_value(result).assert_value()
        );
    }
}
