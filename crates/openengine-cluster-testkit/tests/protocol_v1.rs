use openengine_cluster_client::{ClientError, ClusterClient, InProcessTransport, NdjsonTransport};
use openengine_cluster_protocol::{Generation, Phase, MAX_SAFE_GENERATION, PROTOCOL_VERSION};
use openengine_cluster_server::{ConnectionContext, Dispatcher};
use openengine_cluster_testkit::EmptyBackend;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

fn dispatcher() -> Dispatcher<EmptyBackend> {
    Dispatcher::new(EmptyBackend, ConnectionContext::new("test-connection"))
}

fn response_code(response: &str) -> i64 {
    serde_json::from_str::<Value>(response).unwrap()["error"]["code"]
        .as_i64()
        .unwrap()
}

fn assert_unsupported_version(error: ClientError) {
    match error {
        ClientError::Rpc { code, data, .. } => {
            assert_eq!(code, -32000);
            assert_eq!(data.unwrap().code, "UNSUPPORTED_PROTOCOL_VERSION");
        }
        unexpected => panic!("expected unsupported-version RPC error, got {unexpected}"),
    }
}

#[tokio::test]
async fn initialize_and_get_match_across_transports() {
    let in_process = ClusterClient::new(InProcessTransport::new(dispatcher()));
    assert_unsupported_version(
        in_process
            .initialize_with_version("openengine.cluster/v0")
            .await
            .unwrap_err(),
    );
    let in_initialize = in_process.initialize().await.unwrap();
    let in_get = in_process.get().await.unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_openengine-cluster-stdio"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let mut stderr = child.stderr.take().unwrap();
    let stdio = ClusterClient::new(NdjsonTransport::new(stdout, stdin));
    assert_unsupported_version(
        stdio
            .initialize_with_version("openengine.cluster/v0")
            .await
            .unwrap_err(),
    );
    let stdio_initialize = stdio.initialize().await.unwrap();
    let stdio_get = stdio.get().await.unwrap();

    assert_eq!(in_initialize, stdio_initialize);
    assert_eq!(in_get, stdio_get);
    assert_eq!(in_initialize.protocol_version, PROTOCOL_VERSION);
    assert_eq!(in_initialize.status.phase, Phase::Empty);
    assert_eq!(in_initialize.status.observed_generation, None);
    assert_eq!(in_initialize.status.current_run_id, None);
    assert_eq!(in_initialize.status.at_cursor, None);
    assert_eq!(in_get.spec, None);
    assert_eq!(in_get.at_cursor, None);

    drop(stdio);
    assert!(child.wait().await.unwrap().success());
    let mut diagnostics = String::new();
    stderr.read_to_string(&mut diagnostics).await.unwrap();
    assert!(diagnostics.contains("unsupported protocol version"));
}

#[tokio::test]
async fn rejects_invalid_jsonrpc_inputs_deterministically() {
    let dispatcher = dispatcher();

    let unsupported = dispatcher
        .dispatch_line(
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"openengine.cluster/v0"}}).to_string(),
        )
        .await;
    let unsupported: Value = serde_json::from_str(&unsupported).unwrap();
    assert_eq!(unsupported["error"]["code"], -32000);
    assert_eq!(
        unsupported["error"]["data"]["code"],
        "UNSUPPORTED_PROTOCOL_VERSION"
    );

    let cases = [
        ("{", -32700),
        (
            r#"[{"jsonrpc":"2.0","id":1,"method":"get","params":{}}]"#,
            -32600,
        ),
        (r#"{"id":2,"method":"get","params":{}}"#, -32600),
        (
            r#"{"jsonrpc":"2.0","id":"positional","method":"get","params":[]}"#,
            -32602,
        ),
        (
            r#"{"jsonrpc":"2.0","id":3,"method":"missing","params":{}}"#,
            -32601,
        ),
    ];
    for (request, expected) in cases {
        assert_eq!(
            response_code(&dispatcher.dispatch_line(request).await),
            expected
        );
    }

    let integer_id: Value = serde_json::from_str(
        &dispatcher
            .dispatch_line(
                &json!({"jsonrpc":"2.0","id":41,"method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION}}).to_string(),
            )
            .await,
    )
    .unwrap();
    let string_id: Value = serde_json::from_str(
        &dispatcher
            .dispatch_line(r#"{"jsonrpc":"2.0","id":"get-1","method":"get","params":{}}"#)
            .await,
    )
    .unwrap();
    assert_eq!(integer_id["id"], 41);
    assert_eq!(string_id["id"], "get-1");

    assert_eq!(
        Generation::new(MAX_SAFE_GENERATION).unwrap().get(),
        MAX_SAFE_GENERATION
    );
    assert!(Generation::new(MAX_SAFE_GENERATION + 1).is_err());
    assert!(serde_json::from_str::<Generation>("9007199254740992").is_err());
}

#[tokio::test]
async fn stdio_emits_protocol_frames_only() {
    let input = format!(
        "{}\n{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION}}),
        json!({"jsonrpc":"2.0","id":2,"method":"get","params":{}})
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_openengine-cluster-stdio"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .await
        .unwrap();
    let output = child.wait_with_output().await.unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let frames: Vec<&str> = stdout.lines().collect();
    assert_eq!(frames.len(), 2);
    for frame in frames {
        let response: Value = serde_json::from_str(frame).unwrap();
        assert_eq!(response["jsonrpc"], "2.0");
        assert!(response.get("result").is_some());
    }

    let mut rejected = Command::new(env!("CARGO_BIN_EXE_openengine-cluster-stdio"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    rejected
        .stdin
        .take()
        .unwrap()
        .write_all(b"[]\n")
        .await
        .unwrap();
    let output = rejected.wait_with_output().await.unwrap();
    assert_eq!(String::from_utf8(output.stdout).unwrap().lines().count(), 1);
    assert!(!output.stderr.is_empty());
}
