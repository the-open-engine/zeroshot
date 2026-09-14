use super::*;
use crate::SubscriptionTransport;
use openengine_cluster_protocol::RequestId;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;
use tokio_tungstenite::tungstenite::protocol::Role;

#[tokio::test]
async fn paused_bulk_replay_preserves_every_frame_and_backpressures_the_peer() {
    let (client, server) = tokio::io::duplex(4_096);
    let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
    let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
    let transport = WebSocketTransport::with_subscription_backpressure(client);
    let peer = tokio::spawn(async move {
        let request = server.next().await.assert_value().assert_value();
        let request: serde_json::Value =
            serde_json::from_str(request.to_text().assert_value()).assert_value();
        server
            .send(Message::text(
                json!({"jsonrpc":"2.0","id":request["id"],
            "result":{"subscriptionId":"bulk"}})
                .to_string(),
            ))
            .await
            .assert_value();
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
