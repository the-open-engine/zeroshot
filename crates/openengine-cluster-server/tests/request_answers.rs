use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use openengine_cluster_protocol::{
    GetParams, GetResult, InitializeParams, InitializeResult, RunResumeFrom, RunResumeParams,
    RunResumeResult,
};
use openengine_cluster_server::watch::fixtures::{
    await_ndjson_shutdown, await_websocket_shutdown, spawn_ndjson, spawn_websocket,
};
use openengine_cluster_server::{BackendError, ClusterBackend, ConnectionContext};
use openengine_cluster_testkit::{EmptyBackend, assertions::AssertValue};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_tungstenite::tungstenite::Message;

struct AnswerBackend;

#[async_trait]
impl ClusterBackend for AnswerBackend {
    async fn initialize(
        &self,
        context: &ConnectionContext,
        params: InitializeParams,
    ) -> Result<InitializeResult, BackendError> {
        EmptyBackend.initialize(context, params).await
    }

    async fn get(
        &self,
        context: &ConnectionContext,
        params: GetParams,
    ) -> Result<GetResult, BackendError> {
        EmptyBackend.get(context, params).await
    }

    async fn run_resume(
        &self,
        _context: &ConnectionContext,
        params: RunResumeParams,
    ) -> Result<RunResumeResult, BackendError> {
        let Some(RunResumeFrom::Answer { signals, output }) = params.from else {
            panic!("expected an answer")
        };
        assert_eq!(signals.values().next().assert_value().as_str(), "approved");
        assert_eq!(output["feedback"].as_str().assert_value().len(), 1_048_000);
        assert!(params.github_token.assert_value().len() >= 4096);
        Ok(RunResumeResult {
            run_id: params.successor_run_id,
            resumed_from: params.run_id,
        })
    }
}

fn answer_frame(fill_allowance: bool) -> String {
    let mut request = json!({"jsonrpc":"2.0","id":1,"method":"run/resume","params":{
        "runId":"prior","successorRunId":"successor",
        "from":{"kind":"answer","signals":{"decision":"approved"},"output":{"feedback":"x".repeat(1_048_000)}},
        "githubToken":"x".repeat(4096)
    }});
    if fill_allowance {
        let remaining = 2 * 1024 * 1024 - request.to_string().len();
        request["params"]["githubToken"] = json!("x".repeat(4096 + remaining));
    }
    let frame = request.to_string();
    assert!(frame.len() > 1024 * 1024);
    assert!(frame.len() <= 2 * 1024 * 1024);
    frame
}

fn assert_success(response: &str) {
    let response: Value = serde_json::from_str(response).assert_value();
    assert_eq!(
        response["result"],
        json!({"runId":"successor","resumedFrom":"prior"})
    );
}

#[tokio::test]
async fn websocket_accepts_node_sized_answers_with_credentials_through_the_frame_boundary() {
    let (mut client, server) = spawn_websocket(AnswerBackend).await;
    for fill_allowance in [false, true] {
        client
            .send(Message::text(answer_frame(fill_allowance)))
            .await
            .assert_value();
        let response = client.next().await.assert_value().assert_value();
        assert_success(response.to_text().assert_value());
    }
    client.close(None).await.assert_value();
    drop(client);
    await_websocket_shutdown(server).await;
}

#[tokio::test]
async fn ndjson_accepts_node_sized_answers_with_credentials_through_the_frame_boundary() {
    let (mut writer, reader, server) = spawn_ndjson(AnswerBackend);
    let mut reader = BufReader::new(reader);
    for fill_allowance in [false, true] {
        writer
            .write_all(answer_frame(fill_allowance).as_bytes())
            .await
            .assert_value();
        writer.write_all(b"\n").await.assert_value();
        let mut response = String::new();
        reader.read_line(&mut response).await.assert_value();
        assert_success(&response);
    }
    drop(writer);
    drop(reader);
    await_ndjson_shutdown(server).await;
}
