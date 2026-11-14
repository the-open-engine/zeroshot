use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;

fn status() -> serde_json::Value {
    json!({"runId":"request-run","title":"Approval","size":"medium","atCursor":"v2:1",
        "status":{"phase":"suspended","reason":"request","node":"approval","createdAt":1000,
        "request":{"id":"question","text":"Escaped \\\" } ], [ é","signals":{"decision":["approved","rejected"]},
            "output":{"kind":"record","fields":{}},"expiresAt":2000},"metadata":{}}})
}

fn read(bytes: &[u8]) -> Result<CliRunListResult, TargetAuthorityError> {
    let mut reader = RunListReader::default();
    for &byte in bytes {
        reader.push(byte)?;
    }
    reader.finish()
}

#[test]
fn collection_framing_and_escaped_text_are_validated() {
    let run = status();
    for list in [json!({"runs":[]}), json!({"runs":[run.clone(),run]})] {
        let text = serde_json::to_vec_pretty(&list).assert_value();
        assert_eq!(
            serde_json::to_value(read(&text).assert_value()).assert_value(),
            list
        );
    }
    let escaped_key = format!(r#"{{"r\u0075ns":[{}]}}"#, status());
    assert_eq!(read(escaped_key.as_bytes()).assert_value().runs.len(), 1);
    for malformed in [
        String::new(),
        "[]".to_owned(),
        "{}".to_owned(),
        "{\"runs\":[".to_owned(),
        "{\"runs\":[]".to_owned(),
        "{\"runs\":[,]}".to_owned(),
        format!(r#"{{"runs":[{},]}}"#, status()),
        format!(r#"{{"runs":[{} {}]}}"#, status(), status()),
        r#"{"runs":[],"runs":[]}"#.to_owned(),
        r#"{"runs":[],"extra":true}"#.to_owned(),
        r#"{"runs":[]}{}"#.to_owned(),
        r#"{"runs":[{}]}"#.to_owned(),
    ] {
        assert!(read(malformed.as_bytes()).is_err(), "accepted {malformed}");
    }
}

#[test]
fn every_entry_and_the_outer_framing_remain_bounded() {
    let mut run = status();
    let overhead = run.to_string().len() - run["status"]["request"]["text"].to_string().len() + 2;
    run["status"]["request"]["text"] = json!("x".repeat(MAX_REQUEST_RESPONSE_BYTES - overhead));
    assert_eq!(run.to_string().len(), MAX_REQUEST_RESPONSE_BYTES);
    let mut reader = RunListReader::default();
    for byte in format!(r#"{{"runs":[{run}]}}"#).bytes() {
        reader.push(byte).assert_value();
    }
    assert_eq!(reader.finish().assert_value().runs.len(), 1);
    let text = run["status"]["request"]["text"]
        .as_str()
        .assert_value()
        .to_owned()
        + "x";
    run["status"]["request"]["text"] = json!(text);
    assert!(read(format!(r#"{{"runs":[{run}]}}"#).as_bytes()).is_err());
    assert!(read(format!("{}{{\"runs\":[]}}", " ".repeat(MAX_RESPONSE_BYTES)).as_bytes()).is_err());
}

#[tokio::test]
async fn hosted_inventory_stream_rejects_a_total_response_over_the_receive_ceiling() {
    use openengine_cluster_client::MAX_WIRE_RESPONSE_BYTES;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    let mut run = status();
    run["status"]["request"]["text"] = json!("x".repeat(750_000));
    let mut entry = serde_json::to_vec(&run).assert_value();
    entry.push(b',');
    let count = MAX_WIRE_RESPONSE_BYTES / entry.len() + 1;
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let url = format!("http://{}/runs", listener.local_addr().assert_value());
    let (release, held) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.assert_value();
        let mut socket = BufReader::new(socket);
        loop {
            let mut header = String::new();
            socket.read_line(&mut header).await.assert_value();
            if header == "\r\n" {
                break;
            }
        }
        let socket = socket.get_mut();
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n9\r\n{\"runs\":[\r\n")
            .await.assert_value();
        let chunk_header = format!("{:x}\r\n", entry.len());
        // No terminating HTTP chunk or array end: the byte ceiling must decide the result.
        // Keep the test producer finite even when the receive limit regresses.
        for _ in 0..count {
            for part in [chunk_header.as_bytes(), entry.as_slice(), b"\r\n"] {
                if socket.write_all(part).await.is_err() {
                    return;
                }
            }
        }
        let _ = held.await;
    });
    let response = reqwest::get(url).await.assert_value();
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(60), read_run_list(response)).await;
    let _ = release.send(());
    server.await.assert_value();
    let error = result.assert_value().unwrap_err();
    assert!(error.to_string().contains("too large"), "{error:?}");
}
