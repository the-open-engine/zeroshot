//! Exercise current producer timing through real initialization and transport bindings.
use super::*;
use openengine_cluster_client::{
    ClusterClient, NdjsonTransport, RunSubscriptionClient, RunSubscriptionEvent,
    SubscriptionTransport, WebSocketTransport,
};
use openengine_cluster_protocol::{RunListParams, RunWatchEventNotification, RunWatchParams};
use openengine_cluster_server::watch::fixtures::{
    await_ndjson_shutdown, await_websocket_shutdown, spawn_ndjson, spawn_websocket,
};
use crate::v2_run_ledger::RunLedger;

async fn mixed_runs() -> (Harness, RunId) {
    let harness = harness(Behavior::Hang).await;
    let finished = submit_test_request(
        &harness.controller,
        request_with_key(Value::Null, "finished-wire"),
    )
    .await
    .assert_value();
    harness.driver.wait_started().await;
    harness
        .controller
        .force(RunForceParams {
            run_id: finished.run_id.clone(),
        })
        .await
        .assert_value();
    terminal(&harness.controller, &finished.run_id).await;
    let active = submit_test_request(
        &harness.controller,
        request_with_key(Value::Null, "active-wire"),
    )
    .await
    .assert_value();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = harness
                .controller
                .status(RunStatusParams {
                    run_id: active.run_id.clone(),
                })
                .await
                .assert_value();
            if !serde_json::to_value(status).assert_value()["status"]["activeExecutions"]
                .as_array()
                .is_none_or(Vec::is_empty)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .assert_value();
    (harness, active.run_id)
}

#[tokio::test]
async fn ndjson_wire_contract_preserves_timing() {
    let (harness, run_id) = mixed_runs().await;
    let (write, read, server) = spawn_ndjson(harness.controller.clone());
    let transport = NdjsonTransport::new(read, write);
    tokio::time::timeout(
        Duration::from_secs(10),
        matching_contract(&transport, &harness, run_id),
    )
    .await
    .assert_value();
    drop(transport);
    await_ndjson_shutdown(server).await;
}

#[tokio::test]
async fn websocket_wire_contract_preserves_timing() {
    let (harness, run_id) = mixed_runs().await;
    let (socket, server) = spawn_websocket(harness.controller.clone()).await;
    let transport = WebSocketTransport::new(socket);
    tokio::time::timeout(
        Duration::from_secs(10),
        matching_contract(&transport, &harness, run_id),
    )
    .await
    .assert_value();
    drop(transport);
    await_websocket_shutdown(server).await;
}

async fn matching_contract<T: SubscriptionTransport>(
    transport: &T,
    harness: &Harness,
    run_id: RunId,
) {
    let client = ClusterClient::new(transport);
    client.initialize().await.assert_value();
    let status = client
        .run_status(RunStatusParams {
            run_id: run_id.clone(),
        })
        .await
        .assert_value();
    let serialized = serde_json::to_value(&status).assert_value();
    let started_at = serialized["status"]["activeExecutions"][0]["startedAt"].clone();
    assert!(started_at.as_u64().is_some());
    let stored = harness
        .ledger
        .get(&run_id)
        .await
        .assert_value()
        .assert_value();
    let start = stored
        .snapshot
        .executions
        .values()
        .find(|execution| matches!(execution.state, crate::v2_run_ledger::NodeState::Active))
        .assert_value()
        .started_recorded_at;
    assert_eq!(started_at, serde_json::to_value(start).assert_value());
    let listed = client
        .run_list(RunListParams::default())
        .await
        .assert_value();
    assert_eq!(listed.runs.len(), 2);
    assert_eq!(
        listed
            .runs
            .iter()
            .find(|run| run.run_id == run_id)
            .assert_value()
            .status,
        status.status
    );
    assert!(
        listed
            .runs
            .iter()
            .any(|run| matches!(run.status, RunStatus::Finished { .. }))
    );

    let subscriptions = RunSubscriptionClient::new(transport);
    let (_, mut watch) = subscriptions
        .run_watch(RunWatchParams {
            run_id: run_id.clone(),
            from_cursor: None,
        })
        .await
        .assert_value();
    // Keep the run active through initialization, status, list and its active notification.
    loop {
        let event = watch.next().await.assert_value().assert_value();
        if let RunSubscriptionEvent::Event(event) = event {
            assert_recorded_at(&event, &harness.ledger).await;
            let value = serde_json::to_value(&event).assert_value();
            if value["status"]["activeExecutions"][0]["startedAt"] == started_at {
                break;
            }
        }
    }
    harness
        .controller
        .force(RunForceParams {
            run_id: run_id.clone(),
        })
        .await
        .assert_value();
    terminal(&harness.controller, &run_id).await;
    loop {
        let event = watch.next().await.assert_value().assert_value();
        if let RunSubscriptionEvent::Event(event) = event {
            assert_recorded_at(&event, &harness.ledger).await;
            if matches!(event.status, RunStatus::Finished { .. }) {
                break;
            }
        }
    }
}

async fn assert_recorded_at(event: &RunWatchEventNotification, ledger: &FakeRunLedger) {
    let tail = ledger
        .snapshot_and_tail(&event.run_id, None)
        .await
        .assert_value();
    let stored = tail
        .events
        .iter()
        .find(|stored| stored.cursor == event.cursor)
        .assert_value();
    assert_eq!(event.recorded_at, stored.recorded_at);
    assert!(event.recorded_at.is_some());
}
