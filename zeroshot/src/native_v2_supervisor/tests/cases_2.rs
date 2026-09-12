use super::*;

#[tokio::test]
async fn timeout_cancels_then_acknowledges_cleanup_before_settlement() {
    let harness = harness(
        graph(
            sequence(vec![step("worker", 20), succeed("done")], null_type()),
            null_type(),
        ),
        Value::Null,
        FakeDriver::scripted([("worker", vec![Behavior::Hang])]),
    )
    .await;
    assert!(matches!(
        harness
            .supervisor
            .drive()
            .await
            .assert_value_with("terminal"),
        TerminalResult::Succeeded { .. }
    ));
    assert_eq!(harness.driver.cancellations("worker"), 1);
    assert_eq!(harness.driver.state().active, 0);
    let stored = stored_run(&harness.ledger).await;
    assert!(
        stored
            .snapshot
            .executions
            .values()
            .any(|execution| matches!(
                execution.outcome(),
                Some(WorkerOutcome::Error {
                    code: WorkerErrorCode::Timeout,
                    ..
                })
            ))
    );
    let tail = harness
        .ledger
        .snapshot_and_tail(&RunId::new("run-supervisor-test"), None)
        .await
        .assert_value_with("tail");
    assert!(tail.events.iter().any(|event| matches!(
        &event.event,
        RunEvent::SafeLog { line, .. } if line.as_str().starts_with("Node worker failed: timeout after ")
    )));
}

#[tokio::test]
async fn live_registration_failure_still_durably_drains_before_settlement() {
    let harness = harness(
        graph(
            sequence(vec![step("worker", 1_000), succeed("done")], null_type()),
            null_type(),
        ),
        Value::Null,
        FakeDriver::scripted([(
            "worker",
            vec![Behavior::Complete {
                delay: Duration::from_secs(1),
                outcome: success_for(NodeRole::Worker),
            }],
        )]),
    )
    .await;
    let supervisor = harness
        .supervisor
        .clone()
        .with_live_output(Arc::new(RejectLiveRegistrar {
            driver: harness.driver.clone(),
        }));

    assert!(matches!(
        supervisor.drive().await.assert_value_with("terminal"),
        TerminalResult::Succeeded { .. }
    ));
    let tail = harness
        .ledger
        .snapshot_and_tail(&RunId::new("run-supervisor-test"), None)
        .await
        .assert_value_with("tail");
    let log = tail
        .events
        .iter()
        .position(|event| {
            matches!(
                &event.event,
                RunEvent::SafeLog { line, .. } if line.as_str() == "worker"
            )
        })
        .assert_value_with("durable worker output");
    let completed = tail
        .events
        .iter()
        .position(|event| {
            matches!(
                &event.event,
                RunEvent::NodeCompleted { completion }
                    if completion.reference.node.as_str() == "worker"
                        && matches!(completion.outcome, WorkerOutcome::Error {
                            code: WorkerErrorCode::Crash,
                            ..
                        })
            )
        })
        .assert_value_with("crash settlement");
    let terminal = tail
        .events
        .iter()
        .position(|event| matches!(event.event, RunEvent::Terminal { .. }))
        .assert_value_with("terminal event");
    assert!(log < completed && completed < terminal);
}

use openengine_cluster_testkit::assertions::{AssertValue};

#[tokio::test]
async fn force_stop_closes_active_work_and_never_dispatches_again() {
    let harness = harness(
        graph(
            sequence(
                vec![step("first", 10_000), step("never", 1_000), succeed("done")],
                null_type(),
            ),
            null_type(),
        ),
        Value::Null,
        FakeDriver::scripted([("first", vec![Behavior::Hang])]),
    )
    .await;
    let supervisor = harness.supervisor.clone();
    let drive = tokio::spawn(async move { supervisor.drive().await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while harness.driver.starts("first") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .assert_value_with("first dispatch");
    harness
        .supervisor
        .force_stop()
        .await
        .assert_value_with("force stop");
    assert_eq!(
        drive
            .await
            .assert_value_with("drive task")
            .assert_value_with("terminal"),
        TerminalResult::Failed {
            reason: EnumLabel::new("force_stopped").assert_value_with("label")
        }
    );
    assert_eq!(harness.driver.cancellations("first"), 1);
    assert_eq!(harness.driver.starts("never"), 0);
    assert!(
        stored_run(&harness.ledger)
            .await
            .snapshot
            .active_executions()
            .next()
            .is_none()
    );
    let tail = harness
        .ledger
        .snapshot_and_tail(&RunId::new("run-supervisor-test"), None)
        .await
        .assert_value_with("tail");
    let log = tail
        .events
        .iter()
        .position(|event| {
            matches!(
                &event.event,
                RunEvent::SafeLog { line, .. } if line.as_str() == "first"
            )
        })
        .assert_value_with("durable output");
    let completed = tail
        .events
        .iter()
        .position(|event| {
            matches!(
                &event.event,
                RunEvent::NodeCompleted { completion }
                    if completion.reference.node.as_str() == "first"
            )
        })
        .assert_value_with("force settlement");
    let terminal = tail
        .events
        .iter()
        .position(|event| matches!(event.event, RunEvent::Terminal { .. }))
        .assert_value_with("terminal event");
    assert!(log < completed && completed < terminal);
}

#[tokio::test(start_paused = true)]
async fn nodes_without_deadlines_finish_after_the_old_worker_limit() {
    for (mut node, outcome) in [
        (step("worker", 1), success_for(NodeRole::Worker)),
        (verifier("worker", 1), verifier_outcome("accepted")),
    ] {
        node.as_object_mut().assert_value().remove("timeoutMs");
        let harness = harness(
            graph(
                sequence(vec![node, succeed("done")], null_type()),
                null_type(),
            ),
            Value::Null,
            FakeDriver::scripted([(
                "worker",
                vec![Behavior::Complete {
                    delay: Duration::from_secs(2 * 60 * 60),
                    outcome: outcome.clone(),
                }],
            )]),
        )
        .await;
        harness.supervisor.drive().await.assert_value();
        assert_eq!(harness.driver.cancellations("worker"), 0);
        assert!(
            stored_run(&harness.ledger)
                .await
                .snapshot
                .executions
                .values()
                .any(|execution| execution.outcome() == Some(&outcome))
        );
    }
}

#[tokio::test(start_paused = true)]
async fn unbounded_worker_survives_seven_hours_and_can_be_stopped() {
    let mut worker = step("worker", 1);
    worker.as_object_mut().assert_value().remove("timeoutMs");
    let harness = harness(
        graph(
            sequence(vec![worker, succeed("done")], null_type()),
            null_type(),
        ),
        Value::Null,
        FakeDriver::scripted([("worker", vec![Behavior::Hang])]),
    )
    .await;
    let supervisor = harness.supervisor.clone();
    let drive = tokio::spawn(async move { supervisor.drive().await });
    while harness.driver.starts("worker") == 0 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(7 * 60 * 60)).await;
    assert!(!drive.is_finished());
    assert_eq!(harness.driver.cancellations("worker"), 0);
    harness.supervisor.force_stop().await.assert_value();
    drive.await.assert_value().assert_value();
    assert_eq!(harness.driver.cancellations("worker"), 1);
    assert_eq!(harness.driver.state().active, 0);
}
