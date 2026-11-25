use std::sync::Arc;

use openengine_cluster_protocol::{
    Cursor, IdempotencyKey, RunId, RunStatusParams, RunWatchParams, Sha256Digest,
    UnixTimestampMillis,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::Value;

use super::SqliteRunLedger;
use crate::native_v2_observability::NativeV2Observability;
use crate::native_v2_runner::test_support::{admitted, node_started, request};
use crate::v2_run_ledger::{CreateRun, RunEvent, RunLedger};

#[tokio::test]
async fn readonly_existing_snapshots_recover_only_known_start_times_without_writes() {
    for schema in ["timed", "mixed", "legacy"] {
        let root = openengine_cluster_testkit::TemporaryDirectory::for_test(schema);
        let path = root.path("runs.sqlite3");
        let run_id = RunId::new("existing-timing");
        {
            let clock = Arc::new(|| UnixTimestampMillis::new(10_000).assert_value());
            let ledger = SqliteRunLedger::open(&path)
                .assert_value()
                .with_wall_clock(clock);
            ledger
                .create_or_get(CreateRun {
                    run_id: run_id.clone(),
                    submission_key: IdempotencyKey::new("existing-timing").assert_value(),
                    submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
                    admitted: admitted(),
                })
                .await
                .assert_value();
            let started = |id| {
                let request = request(run_id.as_str(), "worker", (id, id));
                node_started(&request.invocation.reference, Value::Null)
            };
            ledger
                .append(&run_id, vec![RunEvent::RunStarted, started(1), started(2)])
                .await
                .assert_value();
        }
        {
            let db = rusqlite::Connection::open(&path).assert_value();
            let json: String = db
                .query_row("SELECT stored_json FROM v2_runs", [], |row| row.get(0))
                .assert_value();
            let mut stored: Value = serde_json::from_str(&json).assert_value();
            for node in stored["snapshot"]["executions"]
                .as_object_mut()
                .assert_value()
                .values_mut()
            {
                node.as_object_mut()
                    .assert_value()
                    .remove("startedRecordedAt");
            }
            db.execute("UPDATE v2_runs SET stored_json = ?1", [stored.to_string()])
                .assert_value();
            if schema == "mixed" {
                db.execute(
                    "UPDATE v2_run_events SET recorded_at_ms = NULL WHERE sequence = 2",
                    [],
                )
                .assert_value();
            } else if schema == "legacy" {
                db.execute_batch("ALTER TABLE v2_run_events DROP COLUMN recorded_at_ms")
                    .assert_value();
            }
        }
        let before = std::fs::read(&path).assert_value();
        let service = NativeV2Observability::new(Arc::new(
            SqliteRunLedger::open_read_only(&path).assert_value(),
        ));
        let status = service
            .status(RunStatusParams::new(run_id.clone()))
            .await
            .assert_value();
        let status = serde_json::to_value(status).assert_value();
        let active = &status["status"]["activeExecutions"];
        assert_eq!(
            active[0].get("startedAt"),
            (schema == "timed").then_some(&serde_json::json!(10_000))
        );
        assert_eq!(
            active[1].get("startedAt"),
            (schema != "legacy").then_some(&serde_json::json!(10_000))
        );
        let (_, mut watch) = service
            .watch(RunWatchParams {
                part_id: None,
                run_id,
                from_cursor: Some(Cursor::new("v2:1")),
            })
            .await
            .assert_value();
        let events = watch.read_available().await.assert_value();
        assert_eq!(events.len(), 2);
        for (event, known) in events.iter().zip([schema == "timed", schema != "legacy"]) {
            let event = serde_json::to_value(event).assert_value();
            assert_eq!(
                event.get("recordedAt"),
                known.then_some(&serde_json::json!(10_000))
            );
        }
        assert_eq!(
            serde_json::to_value(&events[1]).assert_value()["status"],
            status["status"]
        );
        drop(watch);
        drop(service);
        assert_eq!(std::fs::read(&path).assert_value(), before);
    }
}

#[tokio::test]
async fn readonly_terminal_age_uses_its_exact_existing_event_without_rewriting_legacy_snapshots() {
    for timed in [true, false] {
        let root = openengine_cluster_testkit::TemporaryDirectory::for_test("terminal-age");
        let path = root.path("runs.sqlite3");
        let run_id = RunId::new("terminal-age");
        {
            let ledger = SqliteRunLedger::open(&path)
                .unwrap()
                .with_wall_clock(Arc::new(|| UnixTimestampMillis::new(10_000).unwrap()));
            ledger
                .create_or_get(CreateRun {
                    run_id: run_id.clone(),
                    submission_key: IdempotencyKey::new("terminal-age").unwrap(),
                    submission_digest: Sha256Digest::new("a".repeat(64)).unwrap(),
                    admitted: admitted(),
                })
                .await
                .unwrap();
            ledger
                .append(
                    &run_id,
                    vec![
                        RunEvent::RunStarted,
                        RunEvent::Terminal {
                            result: openengine_cluster_protocol::TerminalResult::Succeeded {
                                output: Value::Null,
                            },
                        },
                    ],
                )
                .await
                .unwrap();
        }
        {
            let db = rusqlite::Connection::open(&path).unwrap();
            let json: String = db
                .query_row("SELECT stored_json FROM v2_runs", [], |row| row.get(0))
                .unwrap();
            let mut stored: Value = serde_json::from_str(&json).unwrap();
            stored["snapshot"]
                .as_object_mut()
                .unwrap()
                .remove("terminalRecordedAt");
            db.execute("UPDATE v2_runs SET stored_json = ?1", [stored.to_string()])
                .unwrap();
            if !timed {
                db.execute_batch("ALTER TABLE v2_run_events DROP COLUMN recorded_at_ms")
                    .unwrap();
            }
        }
        let before = std::fs::read(&path).unwrap();
        let ledger = SqliteRunLedger::open_read_only(&path).unwrap();
        let time = ledger
            .get(&run_id)
            .await
            .unwrap()
            .unwrap()
            .snapshot
            .terminal_recorded_at;
        assert_eq!(time.map(|time| time.get()), timed.then_some(10_000));
        drop(ledger);
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}
