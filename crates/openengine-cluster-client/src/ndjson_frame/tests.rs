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
