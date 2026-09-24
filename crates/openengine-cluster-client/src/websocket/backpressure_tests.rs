use super::*;
use crate::SubscriptionTransport;
use openengine_cluster_protocol::RequestId;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::json;
use tokio::io::DuplexStream;
use tokio_tungstenite::tungstenite::protocol::Role;

async fn websocket_fixture(
    capacity: usize,
    backpressure: bool,
) -> (
    WebSocketTransport<DuplexStream>,
    WebSocketStream<DuplexStream>,
) {
    let (client, server) = tokio::io::duplex(capacity);
    let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
    let server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
    let transport = if backpressure {
        WebSocketTransport::with_subscription_backpressure(client)
    } else {
        WebSocketTransport::new(client)
    };
    (transport, server)
}

async fn receive_request(server: &mut WebSocketStream<DuplexStream>) -> serde_json::Value {
    let request = server.next().await.assert_value().assert_value();
    serde_json::from_str(request.to_text().assert_value()).assert_value()
}

async fn send_subscription_response(
    server: &mut WebSocketStream<DuplexStream>,
    request: &serde_json::Value,
    subscription_id: &str,
) {
    server
        .send(Message::text(
            json!({"jsonrpc":"2.0","id":request["id"],
                "result":{"subscriptionId":subscription_id}})
            .to_string(),
        ))
        .await
        .assert_value();
}

#[tokio::test]
async fn paused_bulk_replay_preserves_every_frame_and_backpressures_the_peer() {
    let (transport, mut server) = websocket_fixture(4_096, true).await;
    let peer = tokio::spawn(async move {
        let request = receive_request(&mut server).await;
        send_subscription_response(&mut server, &request, "bulk").await;
        for index in 0..2_048 {
            let frame = json!({"jsonrpc":"2.0","method":"event",
                "params":{"subscriptionId":"bulk","index":index}});
            server
                .send(Message::text(frame.to_string()))
                .await
                .assert_value();
        }
        server
            .send(Message::text(
                json!({"jsonrpc":"2.0","method":"subscription/closed",
            "params":{"subscriptionId":"bulk","reason":"done"}})
                .to_string(),
            ))
            .await
            .assert_value();
    });
    let (_, subscription) = transport
        .open_subscription(
            json!({"jsonrpc":"2.0","id":"bulk","method":"run/watch","params":{}}).to_string(),
            RequestId::String("bulk".to_owned()),
        )
        .await
        .assert_value();
    let mut subscription = subscription.assert_value();
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    assert!(
        !peer.is_finished(),
        "the paused reader must backpressure the producer"
    );
    assert_eq!(subscription.receiver.len(), 8);
    for index in 0..2_048 {
        let frame = subscription.receiver.recv().await.assert_value();
        let value: serde_json::Value = serde_json::from_str(&frame).assert_value();
        assert_eq!(value["params"]["index"], index);
    }
    let close = subscription.receiver.recv().await.assert_value();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&close).assert_value()["params"]["reason"],
        "done"
    );
    assert!(
        !subscription
            .overflowed
            .load(std::sync::atomic::Ordering::Acquire)
    );
    peer.await.assert_value();
}

#[tokio::test]
async fn abandoned_bulk_subscription_is_cancelled_without_filling_its_queue() {
    let (transport, mut server) = websocket_fixture(4_096, true).await;
    let (abandoned, wait_for_abandonment) = tokio::sync::oneshot::channel::<()>();
    let peer = tokio::spawn(async move {
        let request = receive_request(&mut server).await;
        send_subscription_response(&mut server, &request, "abandoned").await;
        wait_for_abandonment.await.assert_value();
        server
            .send(Message::text(
                json!({"jsonrpc":"2.0","method":"event",
                    "params":{"subscriptionId":"abandoned"}})
                .to_string(),
            ))
            .await
            .assert_value();
        let cancellation = server.next().await.assert_value().assert_value();
        serde_json::from_str::<serde_json::Value>(cancellation.to_text().assert_value())
            .assert_value()
    });

    let (_, subscription) = transport
        .open_subscription(
            json!({"jsonrpc":"2.0","id":"abandoned","method":"run/watch","params":{}}).to_string(),
            RequestId::String("abandoned".to_owned()),
        )
        .await
        .assert_value();
    drop(subscription.assert_value());
    abandoned.send(()).assert_value();
    let cancellation = peer.await.assert_value();
    assert_eq!(cancellation["method"], "subscription/cancel");
    assert_eq!(cancellation["params"]["subscriptionId"], "abandoned");
}

#[tokio::test]
async fn websocket_pump_handles_control_close_and_replay_size_boundaries() {
    let (transport, mut server) = websocket_fixture(4_096, false).await;
    let peer = tokio::spawn(async move {
        let request = receive_request(&mut server).await;
        server
            .send(Message::Binary(Vec::new().into()))
            .await
            .assert_value();
        server
            .send(Message::text(
                json!({"jsonrpc":"2.0","id":request["id"],"result":{}}).to_string(),
            ))
            .await
            .assert_value();
    });
    let response = crate::JsonRpcTransport::request(
        &transport,
        json!({"jsonrpc":"2.0","id":"ping","method":"get","params":{}}).to_string(),
    )
    .await
    .assert_value();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response).assert_value()["id"],
        "ping"
    );
    peer.await.assert_value();

    let (transport, mut server) = websocket_fixture(4_096, false).await;
    let peer = tokio::spawn(async move {
        server.next().await.assert_value().assert_value();
        server.send(Message::Close(None)).await.assert_value();
    });
    let error = crate::JsonRpcTransport::request(
        &transport,
        json!({"jsonrpc":"2.0","id":"closed","method":"get","params":{}}).to_string(),
    )
    .await
    .assert_error();
    assert!(error.to_string().contains("before responding"));
    peer.await.assert_value();

    let (transport, mut server) = websocket_fixture(crate::MAX_FRAME_BYTES * 2, true).await;
    let peer = tokio::spawn(async move {
        server.next().await.assert_value().assert_value();
        server
            .send(Message::text("x".repeat(crate::MAX_FRAME_BYTES + 1)))
            .await
            .assert_value();
    });
    let error = transport
        .open_subscription(
            json!({"jsonrpc":"2.0","id":"oversized","method":"run/watch","params":{}}).to_string(),
            RequestId::String("oversized".to_owned()),
        )
        .await
        .assert_error();
    assert!(error.to_string().contains("before responding"));
    peer.await.assert_value();
}
