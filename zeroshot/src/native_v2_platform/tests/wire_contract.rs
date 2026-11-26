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

fn wire_request(key: &str, workspace: bool) -> RunSubmitParams {
    let mut request = request_with_key(Value::Null, key);
    if workspace {
        let mut graph = serde_json::to_value(&request.submission.graph).assert_value();
        graph["initialInput"] = json!({"kind":"null"});
        let worker = graph["root"]["children"][0].take();
        graph["root"] = json!({
            "kind":"seq", "name":"root", "state":{"kind":"null"},
            "children":[worker, {
                "kind":"succeed", "name":"done", "output":{"kind":"null"}, "bindings":[]
            }],
            "promotedStatePaths":[]
        });
        request.submission.graph = serde_json::from_value(graph).assert_value();
        request.submission.initial_input = Value::Null;
        request
            .submission
            .runtime
            .nodes
            .remove(&NodeName::new("deliver").assert_value());
        request.submission.source = None;
        request.submission.workspace =
            Some(serde_json::from_value(json!({"placement":"managed"})).assert_value());
    }
    request
}

async fn mixed_runs(workspace: bool) -> (Harness, RunId) {
    let mut harness = harness(Behavior::Hang).await;
    if workspace {
        // This shared Git fixture otherwise requires a delivery node, unlike a workspace host.
        harness.controller.delivery_policy = crate::native_v2_admission::DeliveryPolicy::Optional;
    }
    let finished = submit_test_request(
        &harness.controller,
        wire_request("finished-wire", workspace),
    )
    .await
    .assert_value();
    harness.driver.wait_started().await;
    harness
        .controller
        .force(RunForceParams::new(finished.run_id.clone()))
        .await
        .assert_value();
    terminal(&harness.controller, &finished.run_id).await;
    let active = submit_test_request(&harness.controller, wire_request("active-wire", workspace))
        .await
        .assert_value();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = harness
                .controller
                .status(RunStatusParams::new(active.run_id.clone()))
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
    ndjson_observation(false).await;
}

#[tokio::test]
async fn ndjson_workspace_wire_contract_preserves_observation_and_part_identity() {
    ndjson_observation(true).await;
}

async fn ndjson_observation(workspace: bool) {
    let (harness, run_id) = mixed_runs(workspace).await;
    let (write, read, server) = spawn_ndjson(harness.controller.clone());
    let transport = NdjsonTransport::new(read, write);
    tokio::time::timeout(
        Duration::from_secs(10),
        matching_contract(&transport, &harness, run_id, workspace),
    )
    .await
    .assert_value();
    drop(transport);
    await_ndjson_shutdown(server).await;
}

#[tokio::test]
async fn websocket_wire_contract_preserves_timing() {
    websocket_observation(false).await;
}

#[tokio::test]
async fn websocket_workspace_wire_contract_preserves_observation_and_part_identity() {
    websocket_observation(true).await;
}

async fn websocket_observation(workspace: bool) {
    let (harness, run_id) = mixed_runs(workspace).await;
    let (socket, server) = spawn_websocket(harness.controller.clone()).await;
    let transport = WebSocketTransport::new(socket);
    tokio::time::timeout(
        Duration::from_secs(10),
        matching_contract(&transport, &harness, run_id, workspace),
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
    workspace: bool,
) {
    let client = ClusterClient::new(transport);
    client.initialize().await.assert_value();
    let status = client
        .run_status(RunStatusParams::new(run_id.clone()))
        .await
        .assert_value();
    let serialized = serde_json::to_value(&status).assert_value();
    assert_eq!(serialized.get("workspace").is_some(), workspace);
    assert_eq!(serialized["partId"], run_id.as_str());
    assert_eq!(serialized["currentPartId"], run_id.as_str());
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
    assert!(
        listed
            .runs
            .iter()
            .all(|run| run.workspace.is_some() == workspace)
    );
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
            part_id: None,
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
            assert_eq!(event.workspace.is_some(), workspace);
            assert_eq!(event.part_id, run_id);
            let value = serde_json::to_value(&event).assert_value();
            if value["status"]["activeExecutions"][0]["startedAt"] == started_at {
                break;
            }
        }
    }
    harness
        .controller
        .force(RunForceParams::new(run_id.clone()))
        .await
        .assert_value();
    terminal(&harness.controller, &run_id).await;
    loop {
        let event = watch.next().await.assert_value().assert_value();
        if let RunSubscriptionEvent::Event(event) = event {
            assert_recorded_at(&event, &harness.ledger).await;
            assert_eq!(event.workspace.is_some(), workspace);
            assert_eq!(event.part_id, run_id);
            if matches!(event.status, RunStatus::Finished { .. }) {
                break;
            }
        }
    }
}

async fn assert_recorded_at(event: &RunWatchEventNotification, ledger: &FakeRunLedger) {
    let (part_id, cursor) =
        crate::native_v2_observability::lineage::split_cursor(&event.cursor).assert_value();
    assert_eq!(part_id, event.part_id);
    let tail = ledger
        .snapshot_and_tail(&part_id, None)
        .await
        .assert_value();
    let stored = tail
        .events
        .iter()
        .find(|stored| stored.cursor == cursor)
        .assert_value();
    assert_eq!(event.recorded_at, stored.recorded_at);
    assert!(event.recorded_at.is_some());
}
