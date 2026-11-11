use super::*;
use crate::native_v2_observability::NativeV2Observability;
use crate::v2_run_ledger::sqlite::SqliteRunLedger;
use openengine_cluster_protocol::RunLogsParams;

#[tokio::test]
async fn force_stop_before_or_during_initialization_keeps_one_first_version_record() {
    for concurrent in [false, true] {
        let harness = harness(
            graph(
                sequence(vec![step("worker", 1_000), succeed("done")], null_type()),
                null_type(),
            ),
            Value::Null,
            FakeDriver::default(),
        )
        .await;
        let stored = stored_run(&harness.ledger).await;
        let root = tempfile::tempdir().assert_value();
        let path = root.path().join("run.sqlite");
        let ledger = Arc::new(SqliteRunLedger::open(&path).assert_value());
        let run_id = stored.snapshot.run_id;
        ledger
            .create_or_get(CreateRun {
                run_id: run_id.clone(),
                submission_key: stored.submission_key,
                submission_digest: stored.submission_digest,
                admitted: stored.admitted,
            })
            .await
            .assert_value();
        let supervisor = NativeV2Supervisor::new(
            run_id.clone(),
            ledger.clone(),
            harness.supervisor.runner.clone(),
            harness.supervisor.environment.clone(),
        );
        let terminal = tokio::time::timeout(Duration::from_secs(5), async {
            if concurrent {
                let (first, second, terminal) = tokio::join!(
                    supervisor.force_stop(),
                    supervisor.force_stop(),
                    supervisor.drive(),
                );
                first.assert_value();
                second.assert_value();
                terminal.assert_value()
            } else {
                supervisor.force_stop().await.assert_value();
                supervisor.force_stop().await.assert_value();
                supervisor.drive().await.assert_value()
            }
        })
        .await
        .assert_value();
        assert_eq!(
            terminal,
            TerminalResult::Failed {
                context: None,
                reason: EnumLabel::new("force_stopped").assert_value(),
            }
        );
        assert_eq!(supervisor.drive().await.assert_value(), terminal);
        supervisor.force_stop().await.assert_value();
        assert!(harness.driver.state().starts.is_empty());
        drop(supervisor);
        drop(ledger);

        // Verify the finished public logs after reopening durable storage.
        let ledger = Arc::new(SqliteRunLedger::open_read_only(path).assert_value());
        let (_, mut logs) = NativeV2Observability::new(ledger)
            .logs(RunLogsParams {
                run_id,
                from_cursor: None,
                execution: None,
            })
            .await
            .assert_value();
        let mut messages = Vec::new();
        while let Some(record) = logs.recv().await.assert_value() {
            assert!(record.execution.is_none());
            messages.push(record.record.message.as_str().to_owned());
        }
        assert_eq!(
            messages,
            [
                concat!("Zeroshot version: ", env!("CARGO_PKG_VERSION")),
                "Run failed: force_stopped",
            ],
            "concurrent initialization: {concurrent}"
        );
    }
}
