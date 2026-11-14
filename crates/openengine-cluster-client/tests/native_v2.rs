#[path = "support/mod.rs"]
pub mod support;

use openengine_cluster_client::ClusterClient;
use openengine_cluster_protocol::{
    RunForceParams, RunId, RunListParams, RunStatusParams, RunSubmitParams,
};
use serde_json::{json, Value};
use support::{AssertValue, RecordingTransport};

fn response(method: &str) -> Value {
    [
        ("run/submit", json!({"runId":"run-1"})),
        ("run/list", json!({"runs":[]})),
        (
            "run/status",
            json!({
                "runId":"run-1","title":"Protocol client test",
                "source":{
                    "repository":"open-engine/zeroshot","branch":"main",
                    "revision":"0123456789abcdef0123456789abcdef01234567"
                },
                "size":"small","atCursor":"v2:1","status":{"phase":"admitted"}
            }),
        ),
        (
            "run/force",
            json!({
                "runId":"run-1","title":"Protocol client test",
                "source":{
                    "repository":"open-engine/zeroshot","branch":"main",
                    "revision":"0123456789abcdef0123456789abcdef01234567"
                },
                "size":"small","atCursor":"v2:2",
                "status":{"phase":"stopping","activeExecutions":[]}
            }),
        ),
    ]
    .into_iter()
    .find_map(|(candidate, value)| (candidate == method).then_some(value))
    .assert_value_with("expected a known native v2 client method")
}

fn submit() -> RunSubmitParams {
    serde_json::from_value(json!({
        "runId":"run-1",
        "submission":{
            "title":"Protocol client test",
            "graph": {
                "profile":"openengine.graph.full/v1",
                "initialInput":{"kind":"null"},
                "policy":{"policy":"policy.native-v2@1","default":"deny"},
                "root":{
                    "kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]
                }
            },
            "initialInput":null,
            "runtime":{
                "harness":"codex","provider":"openai","size":"small","nodes":{}
            },
            "source":{
                "repository":"open-engine/zeroshot","branch":"main",
                "revision":"0123456789abcdef0123456789abcdef01234567"
            },
            "submissionKey":"submission-1"
        }
    }))
    .assert_value()
}

#[tokio::test]
async fn typed_run_calls_use_the_public_native_v2_method_names() {
    let transport = RecordingTransport::new(response);
    let client = ClusterClient::new(transport.clone());
    assert_eq!(
        client.run_submit(submit()).await.assert_value().run_id,
        RunId::new("run-1")
    );
    assert!(
        client
            .run_list(RunListParams::default())
            .await
            .assert_value()
            .runs
            .is_empty()
    );
    client
        .run_status(RunStatusParams {
            run_id: RunId::new("run-1"),
        })
        .await
        .assert_value();
    client
        .run_force(RunForceParams {
            run_id: RunId::new("run-1"),
        })
        .await
        .assert_value();
    assert_eq!(
        transport.methods().await,
        ["run/submit", "run/list", "run/status", "run/force"]
    );
}

#[tokio::test]
async fn ndjson_inbox_show_reads_a_full_request_larger_than_one_mebibyte() {
    use openengine_cluster_client::NdjsonTransport;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (client_stream, server_stream) = tokio::io::duplex(4096);
    let (read, write) = tokio::io::split(client_stream);
    let client = ClusterClient::new(NdjsonTransport::new(read, write));
    let labels: Vec<_> = (0..4096)
        .map(|index| format!("a{index:04}{}", "x".repeat(123)))
        .collect();
    let peer = tokio::spawn(async move {
        let mut server = BufReader::new(server_stream);
        let mut line = String::new();
        server.read_line(&mut line).await.assert_value();
        let request: Value = serde_json::from_str(&line).assert_value();
        assert_eq!(request["method"], "inbox/show");
        let response = json!({"jsonrpc":"2.0","id":request["id"],"result":{
            "runId":"run-1","node":"approval","createdAt":1000,"state":"pending",
            "request":{"id":"question","text":"x".repeat(1024 * 1024 - 1024),
                "signals":{"decision":["approved","rejected"]},"expiresAt":2000,
                "output":{"kind":"record","fields":{"selection":{"required":true,"type":{"kind":"enum","values":labels}}}}}
        }}).to_string() + "\n";
        assert!(response.len() > 1024 * 1024);
        assert!(response.len() < openengine_cluster_protocol::MAX_REQUEST_RESPONSE_BYTES);
        server
            .get_mut()
            .write_all(response.as_bytes())
            .await
            .assert_value();
    });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.inbox_show(RunStatusParams {
            run_id: RunId::new("run-1"),
        }),
    )
    .await
    .assert_value();
    assert!(result.is_ok(), "{result:?}");
    let result = result.assert_value();
    assert_eq!(result.request.text.len(), 1024 * 1024 - 1024);
    peer.await.assert_value();
}

#[tokio::test]
async fn ndjson_run_inventory_above_one_frame_preserves_other_pending_calls() {
    use openengine_cluster_client::NdjsonTransport;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (client_stream, server_stream) = tokio::io::duplex(4096);
    let (read, write) = tokio::io::split(client_stream);
    let client = ClusterClient::new(NdjsonTransport::new(read, write));
    let peer = tokio::spawn(async move {
        let mut server = BufReader::new(server_stream);
        let mut requests = Vec::new();
        for _ in 0..2 {
            let mut line = String::new();
            server.read_line(&mut line).await.assert_value();
            requests.push(serde_json::from_str::<Value>(&line).assert_value());
        }
        let mut status = response("run/status");
        status["status"] = json!({"phase":"suspended","reason":"request","node":"approval",
            "createdAt":1000,"metadata":{},"request":{"id":"question","text":"x".repeat(750_000),
            "signals":{"decision":["approved","rejected"]},"output":{"kind":"record","fields":{}},"expiresAt":2000}});
        requests.sort_by_key(|request| request["method"].as_str().assert_value().to_owned());
        for request in requests {
            let result = if request["method"] == "run/list" {
                let runs: Vec<_> = (0..5)
                    .map(|index| {
                        let mut row = status.clone();
                        row["runId"] = json!(format!("run-{index}"));
                        row
                    })
                    .collect();
                json!({"runs":runs})
            } else {
                response("run/status")
            };
            let frame =
                json!({"jsonrpc":"2.0","id":request["id"],"result":result}).to_string() + "\n";
            if request["method"] == "run/list" {
                assert!(
                    frame.len() > openengine_cluster_protocol::MAX_REQUEST_RESPONSE_BYTES + 1024
                );
            }
            server
                .get_mut()
                .write_all(frame.as_bytes())
                .await
                .assert_value();
        }
    });
    let (listed, status) = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        tokio::join!(
            client.run_list(RunListParams::default()),
            client.run_status(RunStatusParams {
                run_id: RunId::new("run-1")
            })
        )
    })
    .await
    .assert_value();
    assert_eq!(listed.assert_value().runs.len(), 5);
    assert_eq!(status.assert_value().run_id, RunId::new("run-1"));
    peer.await.assert_value();
}

#[tokio::test]
async fn websocket_run_inventory_above_default_wire_limits_preserves_other_pending_calls() {
    use futures_util::{SinkExt, StreamExt};
    use openengine_cluster_client::{WebSocketDialOptions, dial_websocket};
    use tokio::net::TcpListener;
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let endpoint = format!("ws://{}", listener.local_addr().assert_value());
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.assert_value();
        let mut server = accept_async(stream).await.assert_value();
        let mut requests = Vec::new();
        for _ in 0..2 {
            let message = server.next().await.assert_value().assert_value();
            requests.push(
                serde_json::from_str::<Value>(message.to_text().assert_value()).assert_value(),
            );
        }
        requests.sort_by_key(|request| request["method"].as_str().assert_value().to_owned());
        for request in requests {
            let result = if request["method"] == "run/list" {
                let runs: Vec<_> = (0..96)
                    .map(|index| {
                        let mut status = response("run/status");
                        status["runId"] = json!(format!("run-{index}"));
                        status["status"] = json!({"phase":"suspended","reason":"request",
                            "node":"approval","createdAt":1000,"metadata":{},
                            "request":{"id":format!("question-{index}"),"text":"x".repeat(750_000),
                                "signals":{"decision":["approved","rejected"]},
                                "output":{"kind":"record","fields":{}},"expiresAt":2000}});
                        status
                    })
                    .collect();
                json!({"runs":runs})
            } else {
                response("run/status")
            };
            let frame = json!({"jsonrpc":"2.0","id":request["id"],"result":result}).to_string();
            if request["method"] == "run/list" {
                // One frame exceeds both Tungstenite defaults: 16 MiB/frame and 64 MiB/message.
                assert!(frame.len() > 64 * 1024 * 1024);
            }
            server.send(Message::text(frame)).await.assert_value();
        }
    });
    let transport = dial_websocket(
        &endpoint,
        WebSocketDialOptions::default().allow_plaintext(true),
    )
    .await
    .assert_value();
    let client = ClusterClient::new(transport);
    let (listed, status) = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        tokio::join!(
            client.run_list(RunListParams::default()),
            client.run_status(RunStatusParams {
                run_id: RunId::new("run-1")
            })
        )
    })
    .await
    .assert_value();
    assert_eq!(listed.assert_value().runs.len(), 96);
    assert_eq!(status.assert_value().run_id, RunId::new("run-1"));
    peer.await.assert_value();
}
