use super::*;
use crate::v2_run_ledger::sqlite::SqliteRunLedger;
use crate::v2_run_ledger::{MAX_REPLAY_BYTES, MAX_REPLAY_EVENTS, cursor_for};
use openengine_cluster_protocol::UnixTimestampMillis;

async fn seed_replay(ledger: &dyn RunLedger, log_count: usize, message_bytes: usize) -> RunId {
    let run_id = RunId::new("bounded-replay");
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: IdempotencyKey::new("bounded-replay").assert_value(),
            submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
            admitted: admitted_run(),
        })
        .await
        .assert_value();
    let worker = reference(&run_id, "worker", 1);
    ledger
        .append(&run_id, vec![RunEvent::RunStarted, started(&worker)])
        .await
        .assert_value();
    for start in (0..log_count).step_by(128) {
        let events = (start..(start + 128).min(log_count))
            .map(|index| RunEvent::SafeLog {
                execution: Some(worker.execution),
                timestamp: UnixTimestampMillis::new(1_700_000_000_000).assert_value(),
                stream: SafeLogStream::Output,
                line: SafeLogLine::new(format!("{index:08} {}", "x".repeat(message_bytes - 9)))
                    .assert_value(),
            })
            .collect();
        ledger.append(&run_id, events).await.assert_value();
    }
    finish_worker_run(ledger, &worker).await;
    run_id
}

#[tokio::test]
async fn sqlite_replay_pages_bound_bytes_and_events_without_skipping_the_terminal_tail() {
    for message_bytes in [64, 16 * 1024] {
        let ledger = SqliteRunLedger::open_in_memory().assert_value();
        let log_count = MAX_REPLAY_EVENTS * 3 + 7;
        let run_id = seed_replay(&ledger, log_count, message_bytes).await;
        let mut cursor = cursor_for(0);
        let mut count = 0;
        let mut pages = 0;
        loop {
            let tail = ledger
                .snapshot_and_tail(&run_id, Some(&cursor))
                .await
                .assert_value();
            assert!(tail.events.len() <= MAX_REPLAY_EVENTS);
            let bytes: usize = tail
                .events
                .iter()
                .map(|stored| serde_json::to_vec(&stored.event).assert_value().len())
                .sum();
            assert!(bytes <= MAX_REPLAY_BYTES);
            assert!(!tail.events.is_empty());
            for event in &tail.events {
                count += 1;
                assert_eq!(event.cursor, cursor_for(count));
            }
            cursor = tail.events.last().assert_value().cursor.clone();
            pages += 1;
            if cursor == tail.snapshot.cursor {
                break;
            }
        }
        assert_eq!(count as usize, log_count + 4);
        assert!(pages > 3);
    }
}

async fn assert_resumed_streams(ledger: Arc<dyn RunLedger>, run_id: RunId, log_count: usize) {
    let service = NativeV2Observability::new(ledger);
    let skipped = log_count / 3;
    let after = cursor_for(skipped as u64 + 2);
    let (_, mut logs) = service
        .logs(RunLogsParams {
            run_id: run_id.clone(),
            from_cursor: Some(after.clone()),
            execution: None,
        })
        .await
        .assert_value();
    let mut index = skipped;
    while let Some(event) = logs.recv().await.assert_value() {
        assert_eq!(event.cursor, cursor_for(index as u64 + 3));
        assert!(
            event
                .record
                .message
                .as_str()
                .starts_with(&format!("{index:08} "))
        );
        assert!(logs.pending.len() <= MAX_REPLAY_EVENTS);
        index += 1;
    }
    assert_eq!(index, log_count);
    let (_, mut watch) = service
        .watch(RunWatchParams {
            run_id,
            from_cursor: Some(after),
        })
        .await
        .assert_value();
    let completed = watch.recv().await.assert_value().assert_value();
    assert_eq!(completed.cursor, cursor_for(log_count as u64 + 3));
    let terminal = watch.recv().await.assert_value().assert_value();
    assert_eq!(terminal.cursor, cursor_for(log_count as u64 + 4));
    assert!(matches!(terminal.status, RunStatus::Finished { .. }));
    assert!(watch.recv().await.assert_value().is_none());
}

#[tokio::test]
async fn resumed_terminal_streams_deliver_all_pages_before_done() {
    let ledger = Arc::new(SqliteRunLedger::open_in_memory().assert_value());
    let log_count = MAX_REPLAY_EVENTS * 3 + 7;
    let run_id = seed_replay(ledger.as_ref(), log_count, 16 * 1024).await;
    assert_resumed_streams(ledger, run_id, log_count).await;
}

#[tokio::test]
#[ignore = "one GiB SQLite replay and memory exercise"]
async fn large_sqlite_replay_stays_bounded() {
    let directory =
        openengine_cluster_testkit::TemporaryDirectory::new("large-replay").assert_value();
    let ledger = Arc::new(SqliteRunLedger::open(directory.path("ledger.sqlite")).assert_value());
    let log_count = std::env::var("ZEROSHOT_REPLAY_STRESS_LOGS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(65_536);
    let run_id = seed_replay(ledger.as_ref(), log_count, 16 * 1024).await;
    eprintln!(
        "Seeded {log_count} log records ({} MiB of text)",
        log_count / 64
    );
    assert_resumed_streams(ledger, run_id, log_count).await;
}
