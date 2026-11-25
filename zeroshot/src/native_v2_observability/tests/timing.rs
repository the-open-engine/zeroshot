use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use openengine_cluster_protocol::UnixTimestampMillis;
use crate::v2_run_ledger::{LedgerClock, sqlite::SqliteRunLedger};

fn clock() -> (Arc<AtomicU64>, LedgerClock) {
    let now = Arc::new(AtomicU64::new(10_000));
    let reading = now.clone();
    let clock: LedgerClock =
        Arc::new(move || UnixTimestampMillis::new(reading.load(Ordering::SeqCst)).assert_value());
    (now, clock)
}

async fn recorded_execution_timing(ledger: Arc<dyn RunLedger>, now: Arc<AtomicU64>) {
    let run_id = RunId::new("execution-timing");
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: IdempotencyKey::new("execution-timing").assert_value(),
            submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
            admitted: admitted_run(),
        })
        .await
        .assert_value();
    let left = reference(&run_id, "worker", 1);
    let right = reference(&run_id, "worker", 2);
    let mut retry = reference(&run_id, "worker", 3);
    retry.node_instance = left.node_instance;
    let map_start = |reference: &ExecutionRef, index, attempt| {
        let mut event = started(reference);
        if let RunEvent::NodeStarted {
            occurrence,
            attempt: value,
            ..
        } = &mut event
        {
            occurrence.map_indices = vec![index];
            *value = openengine_cluster_protocol::PositiveInteger::new(attempt).assert_value();
        }
        event
    };
    ledger
        .append(&run_id, vec![RunEvent::RunStarted, map_start(&left, 0, 1)])
        .await
        .assert_value();
    now.store(20_000, Ordering::SeqCst);
    ledger
        .append(&run_id, vec![map_start(&right, 1, 1)])
        .await
        .assert_value();
    // A nearby producer timestamp and a later status request must never supply start timing.
    now.store(30_000, Ordering::SeqCst);
    ledger
        .append(
            &run_id,
            vec![RunEvent::SafeLog {
                diagnostic: None,
                execution: Some(left.execution),
                timestamp: UnixTimestampMillis::new(999_999).assert_value(),
                stream: SafeLogStream::Output,
                line: SafeLogLine::new("unrelated log").assert_value(),
            }],
        )
        .await
        .assert_value();
    let service = NativeV2Observability::new(ledger.clone());
    let status = service
        .status(RunStatusParams::new(run_id.clone()))
        .await
        .assert_value();
    let status_json = serde_json::to_value(&status).assert_value();
    assert_eq!(
        status_json["status"]["activeExecutions"][0]["startedAt"],
        10_000
    );
    assert_eq!(
        status_json["status"]["activeExecutions"][1]["startedAt"],
        20_000
    );
    let hosted = openengine_cluster_protocol::HostedRunStatusResult::from(status);
    assert_eq!(
        serde_json::to_value(hosted).assert_value()["status"],
        status_json["status"]
    );
    let (_, mut watch) = service
        .watch(RunWatchParams {
            part_id: None,
            run_id: run_id.clone(),
            from_cursor: None,
        })
        .await
        .assert_value();
    let starts = watch.read_available().await.assert_value();
    assert_eq!(starts.len(), 3);
    for (event, time) in starts.iter().zip([10_000, 10_000, 20_000]) {
        assert_eq!(
            serde_json::to_value(event).assert_value()["recordedAt"],
            time
        );
    }
    assert_eq!(
        serde_json::to_value(&starts[2]).assert_value()["status"],
        status_json["status"]
    );
    now.store(40_000, Ordering::SeqCst);
    ledger
        .append(
            &run_id,
            vec![completed(&left, Value::Null), map_start(&retry, 0, 2)],
        )
        .await
        .assert_value();
    let transitions = watch.read_available().await.assert_value();
    assert_eq!(transitions.len(), 2);
    let settled = serde_json::to_value(&transitions[0]).assert_value();
    assert_eq!(settled["cursor"], "v2:5");
    assert_eq!(settled["recordedAt"], 40_000);
    assert_eq!(
        settled["status"]["activeExecutions"]
            .as_array()
            .assert_value()
            .len(),
        1
    );
    assert_eq!(
        settled["status"]["activeExecutions"][0]["execution"],
        public_reference(&right).as_str()
    );
    let retried = serde_json::to_value(&transitions[1]).assert_value();
    assert_eq!(
        retried["status"]["activeExecutions"][1]["execution"],
        public_reference(&retry).as_str()
    );
    assert_eq!(
        retried["status"]["activeExecutions"][1]["startedAt"],
        40_000
    );
    assert_eq!(
        retried["status"]["activeExecutions"][1]["mapIndices"],
        serde_json::json!([0])
    );
    let hosted =
        openengine_cluster_protocol::HostedRunWatchEventNotification::from(transitions[0].clone());
    assert_eq!(serde_json::to_value(hosted).assert_value(), settled);
    let (_, mut resumed) = service
        .watch(RunWatchParams {
            part_id: None,
            run_id: run_id.clone(),
            from_cursor: Some(transitions[0].cursor.clone()),
        })
        .await
        .assert_value();
    let replayed = resumed.read_available().await.assert_value();
    assert_eq!(replayed.len(), 1);
    let mut replayed = serde_json::to_value(&replayed[0]).assert_value();
    replayed["subscriptionId"] = retried["subscriptionId"].clone();
    assert_eq!(replayed, retried);
    now.store(50_000, Ordering::SeqCst);
    ledger.request_force_stop(&run_id).await.assert_value();
    let status = service
        .status(RunStatusParams::new(run_id))
        .await
        .assert_value();
    let status = serde_json::to_value(status).assert_value();
    assert_eq!(status["status"]["phase"], "stopping");
    assert_eq!(status["status"]["activeExecutions"][0]["startedAt"], 20_000);
    assert_eq!(status["status"]["activeExecutions"][1]["startedAt"], 40_000);
}

#[tokio::test]
async fn fake_projects_recorded_execution_timing() {
    let (now, clock) = clock();
    recorded_execution_timing(Arc::new(FakeRunLedger::new().with_wall_clock(clock)), now).await;
}

#[tokio::test]
async fn sqlite_projects_recorded_execution_timing() {
    let (now, clock) = clock();
    recorded_execution_timing(
        Arc::new(
            SqliteRunLedger::open_in_memory()
                .assert_value()
                .with_wall_clock(clock),
        ),
        now,
    )
    .await;
}

#[tokio::test]
async fn imported_prerequisites_report_import_time_without_inventing_active_starts() {
    let (now, clock) = clock();
    let ledger = Arc::new(FakeRunLedger::new().with_wall_clock(clock));
    let create = |name: &str| CreateRun {
        run_id: RunId::new(name),
        submission_key: IdempotencyKey::new(name).assert_value(),
        submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
        admitted: admitted_run(),
    };
    let source = RunId::new("source");
    let worker = reference(&source, "worker", 1);
    ledger.create_or_get(create("source")).await.assert_value();
    let original = ledger
        .append(
            &source,
            vec![
                RunEvent::RunStarted,
                started(&worker),
                completed(&worker, Value::Null),
            ],
        )
        .await
        .assert_value();
    let prior = crate::native_v2_supervisor::durable_history(&original.snapshot).assert_value();
    let successor = RunId::new("successor");
    ledger
        .create_or_get(create("successor"))
        .await
        .assert_value();
    now.store(50_000, Ordering::SeqCst);
    let mut events: Vec<_> = prior
        .into_iter()
        .map(|execution| RunEvent::PriorExecution { execution })
        .collect();
    events.push(RunEvent::RunStarted);
    ledger.append(&successor, events).await.assert_value();
    let service = NativeV2Observability::new(ledger);
    let (_, mut watch) = service
        .watch(RunWatchParams {
            part_id: None,
            run_id: successor,
            from_cursor: None,
        })
        .await
        .assert_value();
    let events = watch.read_available().await.assert_value();
    assert_eq!(events.len(), 2);
    for event in events {
        let event = serde_json::to_value(event).assert_value();
        assert_eq!(event["recordedAt"], 50_000);
        assert!(!event.to_string().contains("startedAt"));
    }
}
