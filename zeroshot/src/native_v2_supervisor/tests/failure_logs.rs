use super::*;
use crate::native_v2_observability::NativeV2Observability;
use openengine_cluster_protocol::{LogLevel, RunLogEventNotification, RunLogsParams};

pub(super) async fn public_subscription(
    harness: &Harness,
) -> crate::native_v2_observability::RunLogsSubscription {
    let service = NativeV2Observability::new(harness.ledger.clone());
    let (_, logs) = service
        .logs(RunLogsParams {
            run_id: harness.supervisor.run_id.clone(),
            from_cursor: None,
            execution: None,
        })
        .await
        .assert_value();
    logs
}

async fn public_logs(harness: &Harness) -> Vec<RunLogEventNotification> {
    let mut logs = public_subscription(harness).await;
    let mut records = Vec::new();
    while let Some(record) = logs.recv().await.assert_value() {
        records.push(record);
    }
    records
}

async fn public_errors(harness: &Harness) -> Vec<RunLogEventNotification> {
    public_logs(harness)
        .await
        .into_iter()
        .filter(|record| record.record.level == LogLevel::Error)
        .collect()
}

#[tokio::test]
async fn handled_worker_and_verifier_errors_remain_in_public_logs() {
    for node in [step("node", 1_000), verifier("node", 1_000)] {
        for (outcome, detail) in [
            (
                WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
                "crash",
            ),
            (
                WorkerOutcome::declared_failure(WorkerErrorCode::Timeout),
                "timeout",
            ),
            (
                WorkerOutcome::declared_failure(WorkerErrorCode::Malformed),
                "malformed",
            ),
            (
                WorkerOutcome::declared_failure(WorkerErrorCode::Refusal),
                "refusal",
            ),
            (WorkerOutcome::policy_refusal(), "policy denied"),
            (
                WorkerOutcome::interactive_refusal(),
                "interactive input required",
            ),
            (
                WorkerOutcome::authentication_refusal(),
                "authentication required",
            ),
            (
                WorkerOutcome::malformed(),
                "result did not match the node contract",
            ),
        ] {
            let harness = harness(
                graph(
                    sequence(vec![node.clone(), succeed("done")], null_type()),
                    null_type(),
                ),
                Value::Null,
                FakeDriver::scripted([(
                    "node",
                    vec![Behavior::Complete {
                        delay: Duration::ZERO,
                        outcome,
                    }],
                )]),
            )
            .await;
            assert!(matches!(
                harness.supervisor.drive().await.assert_value(),
                TerminalResult::Succeeded { .. }
            ));
            let errors = public_errors(&harness).await;
            assert_eq!(errors.len(), 1);
            assert!(errors[0].execution.is_some());
            assert!(
                errors[0]
                    .record
                    .message
                    .as_str()
                    .starts_with("Node node failed: ")
            );
            assert!(errors[0].record.message.as_str().contains(detail));
        }
    }
}

#[tokio::test]
async fn successful_retry_preserves_the_failed_attempt_and_runner_cause() {
    let mut review = verifier("review", 1_000);
    review["attempts"] = json!(2);
    let harness = harness(
        graph(
            sequence(vec![review, succeed("done")], null_type()),
            null_type(),
        ),
        Value::Null,
        FakeDriver::scripted([(
            "review",
            vec![
                Behavior::Fail(NodeRunnerError::SessionLost),
                Behavior::Complete {
                    delay: Duration::ZERO,
                    outcome: verifier_outcome("accepted"),
                },
            ],
        )]),
    )
    .await;
    assert!(matches!(
        harness.supervisor.drive().await.assert_value(),
        TerminalResult::Succeeded { .. }
    ));
    let records = public_logs(&harness).await;
    let errors: Vec<_> = records
        .iter()
        .filter(|record| record.record.level == LogLevel::Error)
        .collect();
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0]
            .record
            .message
            .as_str()
            .contains("a reusable node session was lost")
    );
    let executions: BTreeSet<_> = records
        .iter()
        .filter_map(|record| record.execution.clone())
        .collect();
    assert_eq!(executions.len(), 2);
}

#[tokio::test]
async fn returned_driver_details_survive_public_logs_without_breaking_settlement() {
    for detail in [
        "repository not found; upstream response: 404".to_owned(),
        format!("invalid\0record {} root cause at tail", "界".repeat(10_000)),
        format!("newline-rich {} root cause at tail", "\n".repeat(10_000)),
    ] {
        let node = concat!(
            "wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww",
            "wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww",
            "wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww",
            "wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww",
        );
        let harness = harness(
            graph(
                sequence(vec![step(node, 1_000), succeed("done")], null_type()),
                null_type(),
            ),
            Value::Null,
            FakeDriver::scripted([(
                node,
                vec![Behavior::Fail(NodeRunnerError::DriverDetail(
                    detail.clone(),
                ))],
            )]),
        )
        .await;
        harness.supervisor.drive().await.assert_value();
        let records = public_errors(&harness).await;
        let node_errors: Vec<_> = records
            .iter()
            .filter(|record| record.execution.is_some())
            .collect();
        assert_eq!(node_errors.len(), 1);
        let message = node_errors[0].record.message.as_str();
        assert!(!message.contains('\0'));
        assert!(message.len() <= crate::v2_run_ledger::MAX_SAFE_LOG_BYTES);
        if detail.len() < 8 * 1024 {
            assert!(message.contains(&detail));
        } else {
            if detail.contains('\0') {
                assert!(message.contains("invalid\u{fffd}record"));
            } else {
                assert!(message.contains("newline-rich"));
            }
            assert!(message.ends_with("root cause at tail"));
        }
    }
}

#[tokio::test]
async fn fatal_runtime_failure_logs_each_active_node_before_its_completion() {
    let harness = harness(
        parallel(
            json!({"kind": "all"}),
            vec![step("left", 1_000), verifier("right", 1_000)],
        ),
        Value::Null,
        FakeDriver::default(),
    )
    .await;
    let mut active = dispatch_initial(&harness).await;
    // Close the owned work without settling it: failure recovery owns both active completions.
    harness
        .supervisor
        .runner
        .close_run(&harness.supervisor.run_id)
        .await;
    assert!(
        harness
            .supervisor
            .drain_terminalizing_tasks(&mut active.tasks)
            .await
            .assert_value()
            .is_empty()
    );
    harness.supervisor.fail_runtime(|| {}).await.assert_value();
    let errors = public_errors(&harness).await;
    assert_eq!(errors.len(), 3);
    for name in ["left", "right"] {
        assert!(errors.iter().any(|record| record.execution.is_some()
            && record.record.message.as_str()
                == format!("Node {name} failed: crash: run runtime_failed")));
    }
    assert!(errors.iter().any(|record| record.execution.is_none()
        && record.record.message.as_str() == "Run failed: runtime_failed"));
    let tail = harness
        .ledger
        .snapshot_and_tail(&harness.supervisor.run_id, None)
        .await
        .assert_value();
    for (index, stored) in tail.events.iter().enumerate() {
        if let RunEvent::NodeCompleted { completion } = &stored.event {
            assert!(matches!(&tail.events[index - 1].event,
                RunEvent::SafeLog { execution: Some(execution), stream: SafeLogStream::Error, .. }
                    if *execution == completion.reference.execution));
        }
    }
}

#[tokio::test]
async fn authored_graph_failure_is_also_a_run_log() {
    let harness = harness(
        graph(
            json!({"kind": "fail", "name": "stop", "reason": "review_rejected"}),
            null_type(),
        ),
        Value::Null,
        FakeDriver::default(),
    )
    .await;
    harness.supervisor.drive().await.assert_value();
    let records = public_logs(&harness).await;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].record.level, LogLevel::Error);
    assert!(records[0].execution.is_none());
    assert_eq!(
        records[0].record.message.as_str(),
        "Run failed: review_rejected"
    );
}

fn winner_loser_graph() -> GraphSpec {
    parallel(
        json!({"kind": "any"}),
        vec![verifier("winner", 1_000), verifier("loser", 1_000)],
    )
}

#[tokio::test]
async fn a_parallel_loser_retains_its_error_without_changing_void_authority() {
    let harness = harness(
        winner_loser_graph(),
        Value::Null,
        FakeDriver::scripted([("loser", vec![Behavior::Fail(NodeRunnerError::SessionLost)])]),
    )
    .await;
    let mut active = dispatch_initial(&harness).await;
    let first = active.tasks.join_next().await.assert_value().assert_value();
    let second = active.tasks.join_next().await.assert_value().assert_value();
    let (winner, loser) = if first.reference.node.as_str() == "winner" {
        (first, second)
    } else {
        (second, first)
    };
    let losing_execution = loser.execution;
    harness
        .supervisor
        .settle(winner, &mut active.pending_voids)
        .await
        .assert_value();
    active
        .pending_voids
        .insert(losing_execution, ExecutionVoidReason::ParallelJoin);
    harness
        .supervisor
        .settle(loser, &mut active.pending_voids)
        .await
        .assert_value();
    harness.supervisor.drive().await.assert_value();
    let stored = stored_run(&harness.ledger).await;
    assert!(matches!(
        stored.snapshot.executions[&losing_execution].state,
        NodeState::Voided { .. }
    ));
    let errors = public_errors(&harness).await;
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0]
            .record
            .message
            .as_str()
            .contains("Node loser failed: crash")
    );
    assert!(
        errors[0]
            .record
            .message
            .as_str()
            .contains("a reusable node session was lost")
    );
}

#[tokio::test]
async fn cleanup_failure_is_public_while_its_execution_stays_active() {
    let harness = harness(
        super::resolution::worker_graph(None),
        Value::Null,
        FakeDriver::scripted([(
            "worker",
            vec![Behavior::Fail(NodeRunnerError::CleanupUnconfirmed)],
        )]),
    )
    .await;
    assert!(matches!(
        harness.supervisor.drive().await,
        Err(NativeV2SupervisorError::CleanupUnconfirmed)
    ));
    let stored = stored_run(&harness.ledger).await;
    assert!(stored.snapshot.terminal.is_none());
    assert_eq!(stored.snapshot.active_executions().count(), 1);
    let mut logs = public_subscription(&harness).await;
    let records = logs.read_available().await.assert_value();
    assert!(records.iter().any(|record| {
        record.record.level == LogLevel::Error
            && record
                .record
                .message
                .as_str()
                .contains("provider process cleanup could not be confirmed")
    }));
}

#[tokio::test]
async fn force_stop_keeps_an_already_failed_execution_cause() {
    let harness = harness(
        super::resolution::worker_graph(None),
        Value::Null,
        FakeDriver::scripted([("worker", vec![Behavior::Fail(NodeRunnerError::SessionLost)])]),
    )
    .await;
    let mut active = dispatch_initial(&harness).await;
    let finished = active.tasks.join_next().await.assert_value().assert_value();
    let execution = finished.execution;
    harness.supervisor.force_stop().await.assert_value();
    harness
        .supervisor
        .settle(finished, &mut active.pending_voids)
        .await
        .assert_value();
    harness.supervisor.drive().await.assert_value();
    let errors = public_errors(&harness).await;
    assert!(errors.iter().any(|record| {
        record
            .record
            .message
            .as_str()
            .contains("a reusable node session was lost")
    }));
    assert_eq!(
        stored_run(&harness.ledger).await.snapshot.executions[&execution].outcome(),
        Some(&WorkerOutcome::declared_failure(WorkerErrorCode::Refusal))
    );
}

struct RefusedSessionFactory;

#[async_trait]
impl SessionFactory for RefusedSessionFactory {
    async fn open(
        &self,
        _invocation: &NodeInvocation,
        _environment: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        Err(NodeRunnerError::SessionOpen)
    }
}

#[tokio::test]
async fn startup_failure_is_public_before_any_provider_output() {
    let mut harness = harness(
        super::resolution::worker_graph(None),
        Value::Null,
        FakeDriver::default(),
    )
    .await;
    let admitted = stored_run(&harness.ledger).await.admitted;
    harness.supervisor.runner = Arc::new(
        NativeNodeRunner::new(
            &admitted,
            harness.driver.clone(),
            Arc::new(RefusedSessionFactory),
        )
        .assert_value(),
    );
    harness.supervisor.drive().await.assert_value();
    assert_eq!(harness.driver.starts("worker"), 0);
    let mut errors = public_errors(&harness).await;
    let error = errors.pop().assert_value_with("startup error log");
    assert!(errors.is_empty());
    assert!(error.execution.is_some());
    assert!(
        error
            .record
            .message
            .as_str()
            .contains("node session could not be opened")
    );
}

#[tokio::test]
async fn cancellation_keeps_real_loser_errors_and_does_not_invent_errors_for_normal_stops() {
    for error in [NodeRunnerError::SessionLost, NodeRunnerError::Cancelled] {
        let has_error = error == NodeRunnerError::SessionLost;
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let harness = harness(
            winner_loser_graph(),
            Value::Null,
            FakeDriver::scripted([
                ("winner", vec![Behavior::Together(barrier.clone())]),
                ("loser", vec![Behavior::CancelAfterStart(barrier, error)]),
            ]),
        )
        .await;
        assert!(matches!(
            harness.supervisor.drive().await.assert_value(),
            TerminalResult::Succeeded { .. }
        ));
        let stored = stored_run(&harness.ledger).await;
        let loser = stored
            .snapshot
            .executions
            .values()
            .find(|node| node.reference.node.as_str() == "loser")
            .assert_value();
        assert!(matches!(loser.state, NodeState::Voided { .. }));
        let errors = public_errors(&harness).await;
        if has_error {
            assert_eq!(errors.len(), 1);
            assert!(
                errors[0]
                    .record
                    .message
                    .as_str()
                    .contains("a reusable node session was lost")
            );
        } else {
            assert!(errors.is_empty());
        }
    }
}
