use std::collections::HashSet;

use openengine_cluster_protocol::{RunId, INVALID_PARAMS, INVALID_PHASE, SCHEMA_VIOLATION};
use serde_json::json;

use super::*;
use crate::watch::fixtures::{FixtureBackend, FixtureStore};
use crate::ConnectionContext;

fn fixture_dispatcher() -> Dispatcher<FixtureBackend> {
    let store = Arc::new(FixtureStore::new(RunId::new("run-1"), Vec::new(), 8));
    Dispatcher::new(FixtureBackend::new(store), ConnectionContext::default())
}

/// Regression test for a race where `run_watch_subscription` sent the `watch` response before
/// registering the subscription's `WatchHandle` in `subscriptions`: a `subscription/cancel`
/// processed by the read loop in that window found nothing to remove and the subscription was
/// never cancellable again. Forces the response send to block (a pre-filled, capacity-1
/// outbound queue that nothing drains) so the task is parked exactly at that send call, then
/// asserts registration has already happened — true only when the insert precedes the send.
#[tokio::test]
async fn subscription_is_registered_before_its_response_send_can_complete() {
    let dispatcher = fixture_dispatcher();

    let (outbound_tx, mut outbound_rx) = mpsc::channel::<String>(1);
    assert!(outbound_tx.send("occupied".to_owned()).await.is_ok());

    let subscriptions: SubscriptionMap = Arc::new(Mutex::new(HashMap::new()));
    let state = ConnectionState {
        outbound_tx,
        subscriptions: Arc::clone(&subscriptions),
        in_flight_ids: Arc::new(Mutex::new(HashSet::new())),
    };

    let task = tokio::spawn(run_watch_subscription(
        dispatcher,
        RequestId::Integer(1),
        Value::Object(serde_json::Map::new()),
        state,
    ));

    // Let the spawned task run dispatch_watch to completion; it then blocks indefinitely on
    // the full outbound queue, since nothing here drains it yet. Poll via bounded cooperative
    // yields rather than a fixed sleep: a real-time sleep is a race against however long the
    // spawned task actually takes to be scheduled, which flakes under the CPU contention of a
    // full `cargo test --workspace` run; yielding is deterministic regardless of load and the
    // attempt cap still fails the test if registration never happens.
    let mut attempts = 0;
    while subscriptions.lock().len() != 1 {
        attempts += 1;
        assert!(
            attempts < 100_000,
            "subscription was never registered before its response send could complete, \
             so a cancel racing the response would be lost"
        );
        tokio::task::yield_now().await;
    }

    // Drain the queue so the parked task can finish instead of leaking past the test.
    let _ = outbound_rx.recv().await;
    let _ = outbound_rx.recv().await;
    let cancel = subscriptions.lock().values().next().cloned();
    assert!(
        cancel.is_some(),
        "established subscription must be cancellable"
    );
    if let Some(cancel) = cancel {
        cancel.notify_one();
    }
    assert!(
        task.await.is_ok(),
        "subscription task must terminate cleanly"
    );
    assert!(subscriptions.lock().is_empty());
}

#[tokio::test]
async fn native_v2_subscription_refusals_are_typed_and_release_connection_state() {
    let dispatcher = fixture_dispatcher();
    let cases = [
        ("run/watch", json!({}), INVALID_PARAMS, SCHEMA_VIOLATION),
        (
            "run/watch",
            json!({"runId":"run-1"}),
            openengine_cluster_protocol::APPLICATION_ERROR,
            INVALID_PHASE,
        ),
        ("run/logs", json!({}), INVALID_PARAMS, SCHEMA_VIOLATION),
        (
            "run/logs",
            json!({"runId":"run-1"}),
            openengine_cluster_protocol::APPLICATION_ERROR,
            INVALID_PHASE,
        ),
        ("run/attach", json!({}), INVALID_PARAMS, SCHEMA_VIOLATION),
        (
            "run/attach",
            json!({"runId":"run-1", "execution":"execution-1"}),
            openengine_cluster_protocol::APPLICATION_ERROR,
            INVALID_PHASE,
        ),
    ];
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<String>(cases.len());
    let ConnectionSetup { state, .. } = new_connection_setup(&outbound_tx);

    for (index, (method, params, rpc_code, domain_code)) in cases.into_iter().enumerate() {
        let id = RequestId::Integer(index as i64);
        state.in_flight_ids.lock().insert(id.clone());
        match method {
            "run/watch" => {
                native_v2::run_run_watch_subscription(
                    dispatcher.clone(),
                    id.clone(),
                    params,
                    state.clone(),
                )
                .await;
            }
            "run/logs" => {
                native_v2::run_run_logs_subscription(
                    dispatcher.clone(),
                    id.clone(),
                    params,
                    state.clone(),
                )
                .await;
            }
            "run/attach" => {
                native_v2::run_run_attach_subscription(
                    dispatcher.clone(),
                    id.clone(),
                    params,
                    state.clone(),
                )
                .await;
            }
            _ => panic!("unexpected native-v2 subscription method"),
        }

        let response = outbound_rx.recv().await;
        assert!(
            response.is_some(),
            "{method} must emit one refusal response"
        );
        let response: Value = serde_json::from_str(&response.unwrap_or_default())
            .unwrap_or_else(|error| panic!("{method} emitted invalid JSON: {error}"));
        assert_eq!(response.pointer("/error/code"), Some(&json!(rpc_code)));
        assert_eq!(
            response.pointer("/error/data/code"),
            Some(&json!(domain_code))
        );
        assert!(!state.in_flight_ids.lock().contains(&id));
        assert!(state.subscriptions.lock().is_empty());
    }
}
