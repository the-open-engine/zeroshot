use super::*;

#[tokio::test]
async fn terminalizing_cancelled_writer_requires_confirmed_cleanup() {
    for reason in ["force_stopped", "runtime_lost"] {
        for cleanup_failed in [false, true] {
            let error = if cleanup_failed {
                NodeRunnerError::CleanupUnconfirmed
            } else {
                NodeRunnerError::Cancelled
            };
            let (harness, mut active) = start_cancellable_writer(error).await;
            let terminalize = async {
                if reason == "force_stopped" {
                    harness
                        .supervisor
                        .terminalize_force(&mut active.tasks)
                        .await
                } else {
                    harness.supervisor.terminalize_lost(&mut active.tasks).await
                }
            };
            let result = tokio::time::timeout(Duration::from_secs(1), terminalize)
                .await
                .assert_value_with("terminalization drains cancellation");
            let snapshot = stored_run(&harness.ledger).await.snapshot;
            if cleanup_failed {
                assert!(matches!(
                    result,
                    Err(NativeV2SupervisorError::CleanupUnconfirmed)
                ));
                assert!(snapshot.terminal.is_none());
                assert_eq!(snapshot.active_executions().count(), 1);
            } else {
                let expected = TerminalResult::Failed {
                    reason: EnumLabel::new(reason).assert_value(),
                };
                assert_eq!(result.assert_value(), expected);
                assert_eq!(snapshot.terminal, Some(expected));
                assert!(snapshot.active_executions().next().is_none());
            }
            assert!(active.tasks.is_empty());
            assert_eq!(harness.driver.cancellations("worker"), 1);
            assert_eq!(harness.driver.starts("never"), 0);
            assert_eq!(harness.driver.state().active, 0);
        }
    }
}

async fn start_cancellable_writer(error: NodeRunnerError) -> (Harness, ActiveDispatches) {
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let harness = harness(
        graph(
            sequence(
                vec![
                    step("worker", 10_000),
                    step("never", 1_000),
                    succeed("done"),
                ],
                null_type(),
            ),
            null_type(),
        ),
        Value::Null,
        FakeDriver::scripted([(
            "worker",
            vec![Behavior::CancelAfterStart(barrier.clone(), error)],
        )]),
    )
    .await;
    let Initialization::Program(program) = harness.supervisor.initialize().await.assert_value()
    else {
        panic!("new run must initialize a program");
    };
    let snapshot = stored_run(&harness.ledger).await.snapshot;
    let reduction = reduce(&program.admitted, &snapshot).assert_value();
    let mut active = ActiveDispatches::default();
    harness
        .supervisor
        .dispatch(
            &program,
            dispatch_decisions(&harness.supervisor.run_id, reduction.decisions),
            &mut active,
        )
        .await
        .assert_value();
    tokio::time::timeout(Duration::from_secs(1), barrier.wait())
        .await
        .assert_value_with("writer entered driver");
    (harness, active)
}
