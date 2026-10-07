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
                let tail = harness
                    .ledger
                    .snapshot_and_tail(&harness.supervisor.run_id, None)
                    .await
                    .assert_value();
                assert!(tail.events.iter().any(|stored| matches!(&stored.event,
                    RunEvent::SafeLog { execution: Some(_), stream: SafeLogStream::Error, line, .. }
                        if line.as_str().contains("provider process cleanup could not be confirmed"))));
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
    let active = dispatch_initial(&harness).await;
    tokio::time::timeout(Duration::from_secs(1), barrier.wait())
        .await
        .assert_value_with("writer entered driver");
    (harness, active)
}

struct RejectedRuntimeCleanup;

#[async_trait]
impl RunRuntimeCleanup for RejectedRuntimeCleanup {
    async fn cleanup(&self, _exit: RunRuntimeExit) -> Result<(), RuntimeCleanupUnavailable> {
        Err(RuntimeCleanupUnavailable)
    }
}

#[tokio::test]
async fn runtime_cleanup_failure_retains_actual_node_errors_without_settling_them() {
    for force in [false, true] {
        let (mut harness, mut active) =
            start_cancellable_writer(NodeRunnerError::SessionLost).await;
        harness.supervisor.runtime_cleanup = Some(Arc::new(RejectedRuntimeCleanup));
        let result = if force {
            harness
                .supervisor
                .terminalize_force(&mut active.tasks)
                .await
        } else {
            harness.supervisor.terminalize_lost(&mut active.tasks).await
        };
        assert!(matches!(
            result,
            Err(NativeV2SupervisorError::RuntimeCleanup(_))
        ));
        let snapshot = stored_run(&harness.ledger).await.snapshot;
        assert!(snapshot.terminal.is_none());
        assert_eq!(snapshot.active_executions().count(), 1);
        let mut logs = super::failure_logs::public_subscription(&harness).await;
        let records = logs.read_available().await.assert_value();
        assert!(records.iter().any(|record| {
            record
                .record
                .message
                .as_str()
                .contains("a reusable node session was lost")
        }));
    }
}
