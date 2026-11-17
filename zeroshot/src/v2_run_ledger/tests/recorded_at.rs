//! Ledger-recorded event times: one clock reading per batch, never decreasing, unknown on old rows.
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::v2_run_ledger::{LedgerClock, StoredRunEvent};

fn settable_clock(start: u64) -> (Arc<AtomicU64>, LedgerClock) {
    let now = Arc::new(AtomicU64::new(start));
    let reading = now.clone();
    let clock: LedgerClock =
        Arc::new(move || UnixTimestampMillis::new(reading.load(Ordering::SeqCst)).assert_value());
    (now, clock)
}

fn times(events: &[StoredRunEvent]) -> Vec<Option<u64>> {
    events
        .iter()
        .map(|stored| stored.recorded_at.map(UnixTimestampMillis::get))
        .collect()
}

async fn assert_batch_clock_and_clamping(ledger: &dyn RunLedger, now: &AtomicU64) {
    let request = create("run-recorded-at", "submission-recorded-at", 'c');
    let run_id = request.run_id.clone();
    ledger.create_or_get(request).await.assert_value();

    now.store(10_000, Ordering::SeqCst);
    let first = ledger
        .append(
            &run_id,
            vec![RunEvent::RunStarted, started(reference(&run_id, 1))],
        )
        .await
        .assert_value();
    assert_eq!(times(&first.events), [Some(10_000), Some(10_000)]);

    // A wall clock stepping backwards never makes a later record look earlier.
    now.store(4_000, Ordering::SeqCst);
    let clamped = ledger
        .append(
            &run_id,
            vec![safe_log(Some(ExecutionId::new(1).assert_value()))],
        )
        .await
        .assert_value();
    assert_eq!(times(&clamped.events), [Some(10_000)]);

    now.store(25_000, Ordering::SeqCst);
    let completion = ledger
        .append(
            &run_id,
            vec![completed(reference(&run_id, 1), json!("done"))],
        )
        .await
        .assert_value();
    assert_eq!(times(&completion.events), [Some(25_000)]);

    now.store(30_000, Ordering::SeqCst);
    let stop = ledger.request_force_stop(&run_id).await.assert_value();
    assert_eq!(times(&stop.events), [Some(30_000)]);

    let replay = ledger.snapshot_and_tail(&run_id, None).await.assert_value();
    assert_eq!(
        times(&replay.events),
        [
            Some(10_000),
            Some(10_000),
            Some(10_000),
            Some(25_000),
            Some(30_000)
        ]
    );
    // The time lives on the envelope: replayed events equal what the producer appended.
    assert_eq!(replay.events.assert_at(0).event, RunEvent::RunStarted);
    let mut snapshot = RunSnapshot::admitted(run_id.clone(), &admitted_run()).assert_value();
    for stored in &replay.events {
        apply_event(
            &mut snapshot,
            &stored.event,
            cursor_sequence(&stored.cursor).assert_value(),
        )
        .assert_value();
    }
    assert_eq!(snapshot, replay.snapshot);
}

#[tokio::test]
async fn fake_records_one_nondecreasing_time_per_batch() {
    let (now, clock) = settable_clock(1);
    let ledger = FakeRunLedger::new().with_wall_clock(clock);
    assert_batch_clock_and_clamping(&ledger, &now).await;
}

#[tokio::test]
async fn sqlite_records_one_nondecreasing_time_per_batch_across_reopen() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test("recorded-at-batch");
    let path = root.path("runs.sqlite3");
    let (now, clock) = settable_clock(1);
    {
        let ledger = SqliteRunLedger::open(&path)
            .assert_value()
            .with_wall_clock(clock.clone());
        assert_batch_clock_and_clamping(&ledger, &now).await;
    }
    // Clamping uses the durable previous time, not process state.
    now.store(2_000, Ordering::SeqCst);
    let reopened = SqliteRunLedger::open(&path)
        .assert_value()
        .with_wall_clock(clock);
    let run_id = RunId::new("run-recorded-at");
    let terminal = reopened
        .append(
            &run_id,
            vec![RunEvent::Terminal {
                result: TerminalResult::Succeeded {
                    output: Value::Null,
                },
            }],
        )
        .await
        .assert_value();
    assert_eq!(times(&terminal.events), [Some(30_000)]);
}

fn drop_recorded_at_column(path: &std::path::Path) {
    let connection = rusqlite::Connection::open(path).assert_value();
    connection
        .execute_batch("ALTER TABLE v2_run_events DROP COLUMN recorded_at_ms")
        .assert_value();
}

fn has_recorded_at_column(path: &std::path::Path) -> bool {
    let connection = rusqlite::Connection::open(path).assert_value();
    connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('v2_run_events') WHERE name = 'recorded_at_ms'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .assert_value()
        == 1
}

#[tokio::test]
async fn sqlite_migrates_pre_column_ledgers_and_keeps_old_times_unknown() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test("recorded-at-migration");
    let path = root.path("runs.sqlite3");
    let request = create("run-legacy-times", "submission-legacy-times", 'd');
    let run_id = request.run_id.clone();
    let legacy_events = vec![RunEvent::RunStarted, started(reference(&run_id, 1))];
    {
        let ledger = SqliteRunLedger::open(&path).assert_value();
        ledger.create_or_get(request).await.assert_value();
        ledger
            .append(&run_id, legacy_events.clone())
            .await
            .assert_value();
    }
    // Reproduce a ledger written before the column existed.
    drop_recorded_at_column(&path);
    assert!(!has_recorded_at_column(&path));

    // Read-only inspection never migrates and reports old times as unknown.
    {
        let read_only = SqliteRunLedger::open_read_only(&path).assert_value();
        let replay = read_only
            .snapshot_and_tail(&run_id, None)
            .await
            .assert_value();
        assert_eq!(times(&replay.events), [None, None]);
        let replayed: Vec<_> = replay.events.into_iter().map(|e| e.event).collect();
        assert_eq!(replayed, legacy_events);
    }
    assert!(!has_recorded_at_column(&path));

    let (now, clock) = settable_clock(7_000);
    let migrated = SqliteRunLedger::open(&path)
        .assert_value()
        .with_wall_clock(clock.clone());
    assert!(has_recorded_at_column(&path));
    let appended = migrated
        .append(
            &run_id,
            vec![completed(reference(&run_id, 1), json!("done"))],
        )
        .await
        .assert_value();
    assert_eq!(times(&appended.events), [Some(7_000)]);
    drop(migrated);

    // Migration is idempotent and leaves existing rows NULL rather than inventing a time.
    now.store(8_000, Ordering::SeqCst);
    let reopened = SqliteRunLedger::open(&path)
        .assert_value()
        .with_wall_clock(clock);
    let replay = reopened
        .snapshot_and_tail(&run_id, None)
        .await
        .assert_value();
    assert_eq!(times(&replay.events), [None, None, Some(7_000)]);
    assert_eq!(replay.snapshot.cursor, cursor_for(3));
}

#[tokio::test]
async fn sqlite_clamps_past_a_latest_row_without_a_time() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test("recorded-at-null-head");
    let path = root.path("runs.sqlite3");
    let request = create("run-null-head", "submission-null-head", 'e');
    let run_id = request.run_id.clone();
    let (now, clock) = settable_clock(50_000);
    {
        let ledger = SqliteRunLedger::open(&path)
            .assert_value()
            .with_wall_clock(clock.clone());
        ledger.create_or_get(request).await.assert_value();
        ledger
            .append(
                &run_id,
                vec![RunEvent::RunStarted, started(reference(&run_id, 1))],
            )
            .await
            .assert_value();
    }
    // An older binary appending to a migrated ledger leaves its row without a time.
    rusqlite::Connection::open(&path)
        .assert_value()
        .execute(
            "UPDATE v2_run_events SET recorded_at_ms = NULL WHERE sequence = 2",
            [],
        )
        .assert_value();

    now.store(40_000, Ordering::SeqCst);
    let ledger = SqliteRunLedger::open(&path)
        .assert_value()
        .with_wall_clock(clock);
    let appended = ledger
        .append(
            &run_id,
            vec![completed(reference(&run_id, 1), json!("done"))],
        )
        .await
        .assert_value();
    assert_eq!(times(&appended.events), [Some(50_000)]);
}

#[test]
fn sqlite_concurrent_opens_migrate_a_pre_column_ledger_once() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test("recorded-at-race");
    let path = root.path("runs.sqlite3");
    drop(SqliteRunLedger::open(&path).assert_value());
    drop_recorded_at_column(&path);

    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        let opens: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    SqliteRunLedger::open(&path).map(drop)
                })
            })
            .collect();
        for open in opens {
            open.join().assert_value().assert_value();
        }
    });
    assert!(has_recorded_at_column(&path));
}

#[test]
fn recorded_at_is_an_optional_camel_case_envelope_field() {
    let legacy: StoredRunEvent =
        serde_json::from_value(json!({"cursor": "v2:1", "event": {"kind": "run_started"}}))
            .assert_value();
    assert_eq!(legacy.recorded_at, None);
    assert_eq!(
        serde_json::to_value(&legacy).assert_value(),
        json!({"cursor": "v2:1", "event": {"kind": "run_started"}})
    );
    let recorded = StoredRunEvent {
        recorded_at: Some(UnixTimestampMillis::new(1_725_000_000_123).assert_value()),
        ..legacy
    };
    assert_eq!(
        serde_json::to_value(&recorded).assert_value(),
        json!({"cursor": "v2:1", "recordedAt": 1_725_000_000_123_u64, "event": {"kind": "run_started"}})
    );
}
