use super::*;

#[tokio::test]
async fn crashed_node_instance_verifier_retries_with_a_fresh_provider_session() {
    const VERIFIER_ATTEMPTS: u64 = 2;

    let mut verifier = verifier("review", 1_000);
    verifier["attempts"] = json!(VERIFIER_ATTEMPTS);
    let graph = graph(
        sequence(vec![verifier, succeed("done")], null_type()),
        null_type(),
    );
    let harness = harness_with_session_scope(
        graph,
        Value::Null,
        FakeDriver::scripted([(
            "review",
            vec![
                Behavior::Fail(NodeRunnerError::Driver),
                Behavior::Complete {
                    delay: Duration::ZERO,
                    outcome: verifier_outcome("accepted"),
                },
            ],
        )]),
        SessionScope::NodeInstance,
    )
    .await;

    assert_eq!(
        harness.supervisor.drive().await.assert_value_with("drive"),
        TerminalResult::Succeeded {
            output: Value::Null
        }
    );
    assert_eq!(harness.driver.starts("review"), VERIFIER_ATTEMPTS as usize);
    assert!(harness.sessions.closed.load(Ordering::SeqCst) >= 1);
    assert_eq!(
        harness.sessions.opened.load(Ordering::SeqCst),
        VERIFIER_ATTEMPTS as usize
    );
}

async fn seed_active_execution(ledger: &FakeRunLedger, run_id: &RunId) -> ExecutionRef {
    let reference = ExecutionRef {
        run_id: run_id.clone(),
        node: NodeName::new("worker").assert_value_with("node name"),
        node_instance: crate::native_v2_contract::NodeInstanceId::new(1)
            .assert_value_with("node instance"),
        execution: ExecutionId::new(1).assert_value_with("execution"),
    };
    ledger
        .append(
            run_id,
            vec![
                RunEvent::RunStarted,
                RunEvent::NodeStarted {
                    reference: reference.clone(),
                    occurrence: crate::full_v1_reducer::StructuralOccurrence {
                        node: reference.node.clone(),
                        map_indices: Vec::new(),
                    },
                    attempt: openengine_cluster_protocol::PositiveInteger::new(1)
                        .assert_value_with("attempt"),
                    input: Value::Null,
                },
            ],
        )
        .await
        .assert_value_with("seed active execution");
    reference
}

#[tokio::test]
async fn active_history_after_runtime_loss_is_terminalized_without_redispatch() {
    let harness = harness(
        graph(
            sequence(vec![step("worker", 1_000), succeed("done")], null_type()),
            null_type(),
        ),
        Value::Null,
        FakeDriver::default(),
    )
    .await;
    let run_id = RunId::new("run-supervisor-test");
    let reference = seed_active_execution(harness.ledger.as_ref(), &run_id).await;

    let expected = TerminalResult::Failed {
        reason: EnumLabel::new("runtime_lost").assert_value_with("label"),
    };
    assert_eq!(
        harness
            .supervisor
            .drive()
            .await
            .assert_value_with("terminal"),
        expected
    );
    assert_eq!(
        harness
            .supervisor
            .drive()
            .await
            .assert_value_with("idempotent"),
        expected
    );
    assert!(harness.driver.state().starts.is_empty());
    let tail = harness
        .ledger
        .snapshot_and_tail(&run_id, None)
        .await
        .assert_value_with("tail");
    assert!(tail.snapshot.active_executions().next().is_none());
    assert_eq!(
        tail.events
            .iter()
            .filter(|event| matches!(event.event, RunEvent::Terminal { .. }))
            .count(),
        1
    );
    assert!(matches!(
        tail.snapshot
            .executions
            .get(&reference.execution)
            .assert_value()
            .outcome(),
        Some(WorkerOutcome::Error {
            code: WorkerErrorCode::Crash,
            ..
        })
    ));
}

#[tokio::test]
async fn terminal_run_is_idempotently_observed_without_dispatch() {
    let harness = harness(
        graph(succeed("done"), null_type()),
        Value::Null,
        FakeDriver::default(),
    )
    .await;
    let expected = TerminalResult::Succeeded {
        output: Value::Null,
    };
    assert_eq!(
        harness.supervisor.drive().await.assert_value_with("first"),
        expected
    );
    assert_eq!(
        harness.supervisor.drive().await.assert_value_with("second"),
        expected
    );
    assert!(harness.driver.state().starts.is_empty());
    let tail = harness
        .ledger
        .snapshot_and_tail(&RunId::new("run-supervisor-test"), None)
        .await
        .assert_value_with("tail");
    assert_eq!(
        tail.events
            .iter()
            .filter(|event| matches!(event.event, RunEvent::Terminal { .. }))
            .count(),
        1
    );
}

use openengine_cluster_testkit::assertions::{AssertValue};

#[tokio::test]
async fn live_output_registration_is_closed_after_durable_drain() {
    let harness = harness(
        graph(
            sequence(vec![step("worker", 1_000), succeed("done")], null_type()),
            null_type(),
        ),
        Value::Null,
        FakeDriver::default(),
    )
    .await;
    let live = Arc::new(FakeLiveRegistrar::default());
    let supervisor = harness.supervisor.clone().with_live_output(live.clone());
    assert!(matches!(
        supervisor.drive().await.assert_value_with("terminal"),
        TerminalResult::Succeeded { .. }
    ));
    assert_eq!(
        live.registered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        1
    );
    assert_eq!(
        *live
            .closed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        1
    );
}
