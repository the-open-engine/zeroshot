//! Response bounds shared by NDJSON and WebSocket readers.

use super::*;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;
use tokio::io::BufReader;

fn status(text: String) -> serde_json::Value {
    json!({
        "runId":"run-1","title":"Inventory request",
        "source":{"repository":"open-engine/zeroshot","branch":"main",
            "revision":"0123456789abcdef0123456789abcdef01234567"},
        "size":"small","atCursor":"v2:1",
        "status":{"phase":"suspended","reason":"request","node":"approval","createdAt":1000,
            "request":{"id":"question","text":text,"signals":{"decision":["approved","rejected"]},
                "output":{"kind":"record","fields":{}},"expiresAt":2000},"metadata":{}}
    })
}

async fn read(bytes: &[u8]) -> io::Result<Option<String>> {
    next_line(&mut BufReader::with_capacity(17, bytes)).await
}

#[tokio::test]
async fn collection_bounds_follow_typed_statuses_across_chunks_and_escaped_strings() {
    let request = status("question []{}\"\\".repeat(60_000));
    let frame = json!({"id":1,"result":{"runs":[request.clone(),request.clone(),request.clone(),request]},"jsonrpc":"2.0"}).to_string();
    assert!(frame.len() > crate::MAX_FRAME_BYTES);
    assert_eq!(
        read(format!("{frame}\r\n").as_bytes()).await.assert_value(),
        Some(frame.clone())
    );
    assert_eq!(read(frame.as_bytes()).await.assert_value(), Some(frame));
}

#[tokio::test]
async fn unfinished_run_inventory_keeps_a_finite_total_receive_bound() {
    use tokio::io::AsyncWriteExt;
    use tokio::sync::oneshot;

    let entry = serde_json::to_vec(&status("x".repeat(750_000))).assert_value();
    let count = crate::websocket::MAX_WIRE_RESPONSE_BYTES / entry.len() + 1;
    let (mut peer, stream) = tokio::io::duplex(64 * 1024);
    let (release, held) = oneshot::channel::<()>();
    let sender = tokio::spawn(async move {
        peer.write_all(br#"{"jsonrpc":"2.0","id":1,"result":{"runs":["#)
            .await
            .assert_value();
        // Stream only enough valid statuses to cross the bound, with no array end or newline.
        // A missing bound must time out rather than grow an unbounded test payload.
        for _ in 0..count {
            if peer.write_all(&entry).await.is_err() || peer.write_all(b",").await.is_err() {
                return;
            }
        }
        let _ = held.await;
    });
    let mut reader = BufReader::new(stream);
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(60), next_line(&mut reader)).await;
    drop(reader);
    let _ = release.send(());
    sender.await.assert_value();
    let error = result.assert_value().unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("too large"), "{error}");
}

#[tokio::test]
async fn collection_entries_and_non_collection_frames_keep_explicit_bounds() {
    let oversized = status("x".repeat(MAX_REQUEST_RESPONSE_BYTES));
    for frame in [
        json!({"jsonrpc":"2.0","id":1,"result":{"runs":[oversized]}}),
        json!({"jsonrpc":"2.0","id":1,"result":"x".repeat(crate::MAX_FRAME_BYTES)}),
        json!({"jsonrpc":"2.0","id":1,"other":{"runs":vec![status("x".repeat(800_000)); 5]}}),
    ] {
        assert_eq!(
            read(frame.to_string().as_bytes()).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
    let small = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"runs\":[]}}";
    assert_eq!(
        read(small.as_bytes()).await.assert_value(),
        Some(small.to_owned())
    );
}

#[tokio::test]
async fn run_inventory_cannot_hide_an_oversized_envelope_or_non_status_items() {
    for frame in [
        json!({"jsonrpc":"2.0","id":1,"result":{"runs":[],"extra":"x".repeat(crate::MAX_FRAME_BYTES)}}),
        json!({"jsonrpc":"2.0","id":1,"result":{"runs":[{"status":"unknown"}]}}),
    ] {
        assert!(read(frame.to_string().as_bytes()).await.is_err());
    }
}

#[test]
fn ordinary_bounds_follow_the_transport_while_inventory_entries_keep_their_bound() {
    let ordinary = json!({"jsonrpc":"2.0","id":1,"result":{"value":"x".repeat(4096)}}).to_string();
    assert!(validate(ordinary.as_bytes(), 4096).is_err());
    assert!(validate(ordinary.as_bytes(), 8192).is_ok());
    let oversized = json!({"jsonrpc":"2.0","id":1,"result":{
        "runs":[status("x".repeat(MAX_REQUEST_RESPONSE_BYTES))]}})
    .to_string();
    assert!(validate(oversized.as_bytes(), 64 * 1024 * 1024).is_err());
}
