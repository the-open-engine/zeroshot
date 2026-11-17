use super::*;
use crate::v2_run_ledger::{steering, MAX_REPLAY_BYTES};
use openengine_cluster_protocol::{RunSteerParams, MAX_NODE_INSTRUCTIONS_BYTES};

async fn assert_large_status(ledger: &dyn RunLedger, run: &RunId) {
    assert!(
        steering::status(ledger, run)
            .await
            .assert_value()
            .steers
            .is_empty()
    );
    for index in 0..70 {
        steering::record(
            ledger,
            RunSteerParams {
                mode: Default::default(),
                run_id: run.clone(),
                text: format!(
                    "{index:03}{}",
                    "é".repeat((MAX_NODE_INSTRUCTIONS_BYTES - 4) / 2)
                ),
            },
        )
        .await
        .assert_value();
    }
    let page = ledger.steering_events(run, None).await.assert_value();
    assert!(page.len() < 70);
    let bytes: usize = page
        .iter()
        .map(|event| serde_json::to_vec(&event.event).assert_value().len())
        .sum();
    assert!(bytes <= MAX_REPLAY_BYTES);
    let status = steering::status(ledger, run).await.assert_value();
    assert_eq!(status.omitted_steers, 6);
    assert_eq!(status.steers.len(), 64);
    for (index, steer) in status.steers.iter().enumerate() {
        assert!(steer.excerpt.starts_with(&format!("{:03}", index + 6)));
        assert_eq!(steer.excerpt.len(), 1023);
        assert_eq!(steer.text_bytes, MAX_NODE_INSTRUCTIONS_BYTES - 1);
    }
    assert_eq!(
        ledger
            .steering_events(run, Some(&cursor_for(0)))
            .await
            .assert_value()
            .len(),
        page.len()
    );
    assert!(matches!(
        ledger
            .steering_events(run, Some(&cursor_for(999_999)))
            .await,
        Err(RunLedgerError::CursorAhead)
    ));
    assert!(matches!(
        ledger.steering_events(&RunId::new("missing"), None).await,
        Err(RunLedgerError::RunNotFound)
    ));
}

#[tokio::test]
async fn steer_status_pages_large_texts_instead_of_retaining_full_record_on_fake() {
    let ledger = FakeRunLedger::new();
    let run = RunId::new("steer-stream");
    ledger
        .create_or_get(create(run.as_str(), run.as_str(), 'c'))
        .await
        .assert_value();
    assert_large_status(&ledger, &run).await;
}

#[tokio::test]
async fn steer_status_ignores_long_log_history_and_pages_large_texts_including_legacy_read_only() {
    let root = tempfile::tempdir().assert_value();
    let path = root.path().join("ledger.sqlite");
    let ledger = SqliteRunLedger::open(&path).assert_value();
    let run = RunId::new("steer-stream");
    ledger
        .create_or_get(create(run.as_str(), run.as_str(), 'c'))
        .await
        .assert_value();
    ledger
        .append(&run, vec![safe_log(None); 10_000])
        .await
        .assert_value();
    // Valid JSON with an unknown event kind proves status never decodes ordinary history.
    let connection = rusqlite::Connection::open(&path).assert_value();
    connection
        .execute(
            "UPDATE v2_run_events SET event_json = '{\"kind\":\"old_unreadable_log\"}'",
            [],
        )
        .assert_value();
    drop(connection);
    assert_large_status(&ledger, &run).await;
    let expected = steering::status(&ledger, &run).await.assert_value();
    drop(ledger);
    let reader = SqliteRunLedger::open_read_only(&path).assert_value();
    assert_eq!(
        steering::status(&reader, &run).await.assert_value(),
        expected
    );
    drop(reader);
    let connection = rusqlite::Connection::open(&path).assert_value();
    connection
        .execute_batch("DROP INDEX v2_run_steering")
        .assert_value();
    drop(connection);
    let before = std::fs::read(&path).assert_value();
    let reader = SqliteRunLedger::open_read_only(&path).assert_value();
    assert_eq!(
        steering::status(&reader, &run).await.assert_value(),
        expected
    );
    assert_eq!(std::fs::read(&path).assert_value(), before);
}

#[tokio::test]
async fn steer_status_preserves_pre_vector_read_only_runs_without_replaying_history() {
    use std::sync::Arc;
    use crate::native_v2_observability::NativeV2Observability;
    use openengine_cluster_protocol::RunStatusParams;
    let root = tempfile::tempdir().assert_value();
    let path = root.path().join("pre-vector.sqlite");
    let ledger = SqliteRunLedger::open(&path).assert_value();
    let run = RunId::new("pre-vector");
    ledger
        .create_or_get(create(run.as_str(), run.as_str(), 'c'))
        .await
        .assert_value();
    ledger
        .append(&run, vec![safe_log(None); 20_000])
        .await
        .assert_value();
    let params = RunStatusParams {
        run_id: run.clone(),
    };
    let expected = NativeV2Observability::new(Arc::new(ledger.clone()))
        .status(params.clone())
        .await
        .assert_value();
    drop(ledger);
    let connection = rusqlite::Connection::open(&path).assert_value();
    connection
        .execute_batch(
            "DROP INDEX v2_run_steering;
             DROP INDEX v2_run_vector_ids;
             UPDATE v2_run_events SET event_json = '{\"kind\":\"old_unreadable_log\"}';",
        )
        .assert_value();
    drop(connection);
    let before = std::fs::read(&path).assert_value();
    let reader = Arc::new(SqliteRunLedger::open_read_only(&path).assert_value());
    assert!(reader.get(&run).await.assert_value().is_some());
    let observer = NativeV2Observability::new(reader.clone());
    for _ in 0..2 {
        assert_eq!(
            observer.status(params.clone()).await.assert_value(),
            expected
        );
    }
    assert!(matches!(
        reader
            .steering_events(&run, Some(&cursor_for(999_999)))
            .await,
        Err(RunLedgerError::CursorAhead)
    ));
    assert!(matches!(
        reader.steering_events(&RunId::new("missing"), None).await,
        Err(RunLedgerError::RunNotFound)
    ));
    assert_eq!(std::fs::read(&path).assert_value(), before);
}

#[tokio::test]
async fn network_steer_records_modes_origin_and_canonical_destinations_in_existing_vectors() {
    use openengine_cluster_protocol::{ExecutionRef, NetworkSteerParams, SteerOrigin, SteeringMode};
    let ledger = FakeRunLedger::new();
    let run = RunId::new("network-record");
    ledger
        .create_or_get(create(run.as_str(), "network-record", 'a'))
        .await
        .assert_value();
    let origin = SteerOrigin {
        node: NodeName::new("sender").assert_value(),
        node_instance: 3,
        execution: ExecutionRef::new("execution/3").assert_value(),
    };
    let message = super::super::steering::record_network(
        &ledger,
        NetworkSteerParams {
            run_id: run.clone(),
            text: "message to future members".into(),
            mode: SteeringMode::Once,
            networks: vec![
                NodeName::new("team").assert_value(),
                NodeName::new("team").assert_value(),
            ],
        },
        origin.clone(),
    )
    .await
    .assert_value();
    assert_eq!(message.steer.mode, SteeringMode::Once);
    let network = message.steer.network.as_ref().assert_value();
    assert_eq!(network.origin, origin);
    assert_eq!(network.networks.len(), 1);
    let status = super::super::steering::status(&ledger, &run)
        .await
        .assert_value();
    assert_eq!(status.steers, [message.steer]);
    assert_eq!(
        super::super::steering::all(&ledger, &run)
            .await
            .assert_value()
            .len(),
        1
    );
}
