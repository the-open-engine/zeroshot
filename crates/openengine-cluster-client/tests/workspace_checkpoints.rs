use std::collections::BTreeMap;

use async_trait::async_trait;
use openengine_cluster_client::{ClusterClient, JsonRpcTransport, TransportError};
use openengine_cluster_protocol::{
    CheckpointId, RunCheckpointsParams, RunId, RunResumeFrom, RunResumeParams,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{json, Value};

struct CheckpointTransport;

#[async_trait]
impl JsonRpcTransport for CheckpointTransport {
    async fn request(&self, request: String) -> Result<String, TransportError> {
        let request: Value = serde_json::from_str(&request).assert_value();
        let result = match request["method"].as_str() {
            Some("run/checkpoints") => {
                assert_eq!(
                    request["params"],
                    json!({
                        "protocolVersion": openengine_cluster_protocol::PROTOCOL_VERSION,
                        "runId": "prior", "after": "entry-1", "limit": 1
                    })
                );
                json!({
                    "runId": "prior", "partId":"prior", "checkpoints": [{
                        "checkpointId": "entry-2", "sequence": 2, "node": "write",
                        "mapIndices": [], "loopIterations": [], "createdAt": 42
                    }], "nextAfter": "entry-2"
                })
            }
            Some("run/resume") => {
                assert_eq!(
                    request["params"],
                    json!({
                        "protocolVersion": openengine_cluster_protocol::PROTOCOL_VERSION,
                        "runId": "prior", "partId":"prior", "successorPartId": "successor",
                        "from": {"kind": "checkpoint", "checkpointId": "entry-2"}
                    })
                );
                json!({"runId":"prior","currentPartId":"successor","resumedFromPartId":"prior"})
            }
            _ => return Err(TransportError::Protocol("unexpected method".to_owned())),
        };
        Ok(json!({"jsonrpc": "2.0", "id": request["id"], "result": result}).to_string())
    }
}

#[tokio::test]
async fn typed_client_lists_and_resumes_using_the_returned_checkpoint_identity() {
    let client = ClusterClient::new(CheckpointTransport);
    let result = client
        .run_checkpoints(RunCheckpointsParams {
            part_id: None,
            run_id: RunId::new("prior"),
            after: Some(CheckpointId::new("entry-1").assert_value()),
            limit: Some(1),
        })
        .await
        .assert_value();
    assert_eq!(
        result.next_after.as_ref().map(CheckpointId::as_str),
        Some("entry-2")
    );
    let checkpoint = result.checkpoints.into_iter().next().assert_value();
    assert_eq!(checkpoint.sequence.get(), 2);
    assert_eq!(checkpoint.node.as_str(), "write");
    let successor = client
        .run_resume(RunResumeParams {
            runtime: None,
            part_id: result.run_id.clone(),
            run_id: result.run_id,
            successor_run_id: RunId::new("successor"),
            from: Some(RunResumeFrom::Checkpoint {
                checkpoint_id: checkpoint.checkpoint_id,
            }),
            connections: BTreeMap::new(),
            connection_resolver: None,
            github_token: None,
            git_credentials: None,
            commit_identity: None,
            commit_co_author: None,
        })
        .await
        .assert_value();
    assert_eq!(successor.run_id, RunId::new("prior"));
    assert_eq!(successor.current_part_id, RunId::new("successor"));
    assert_eq!(successor.resumed_from, RunId::new("prior"));
}

struct RuntimeReplacementTransport;

#[async_trait]
impl JsonRpcTransport for RuntimeReplacementTransport {
    async fn request(&self, request: String) -> Result<String, TransportError> {
        let request: Value = serde_json::from_str(&request).assert_value();
        let result = if request["method"] == "run/resume_requirements" {
            assert_eq!(request["params"]["runId"], "prior");
            assert_eq!(
                request["params"]["runtime"]["runtime"]["model"],
                "replacement-model"
            );
            json!({"connections":{"setup":["SETUP_TOKEN"]}})
        } else {
            assert_eq!(request["method"], "run/resume");
            assert_eq!(request["params"]["runtime"]["kind"], "uniform");
            assert_eq!(
                request["params"]["runtime"]["runtime"]["model"],
                "replacement-model"
            );
            json!({"runId":"prior","currentPartId":"successor","resumedFromPartId":"prior"})
        };
        Ok(json!({"jsonrpc":"2.0","id":request["id"],"result":result}).to_string())
    }
}

#[tokio::test]
async fn typed_client_queries_fixed_requirements_and_carries_runtime_replacement() {
    let client = ClusterClient::new(RuntimeReplacementTransport);
    let result = client
        .run_resume_requirements(openengine_cluster_protocol::RunResumeRequirementsParams {
            part_id: None,
            run_id: RunId::new("prior"), runtime:serde_json::from_value(json!({"kind":"uniform","runtime":{"harness":"codex","provider":"openai","model":"replacement-model"}})).assert_value(),
        })
        .await
        .assert_value();
    assert_eq!(
        serde_json::to_value(result).assert_value(),
        json!({"connections":{"setup":["SETUP_TOKEN"]}})
    );
    let mut params: RunResumeParams = serde_json::from_value(
        json!({"runId":"prior","partId":"prior","successorPartId":"successor"}),
    )
    .assert_value();
    params.runtime = Some(
        serde_json::from_value(json!({"kind":"uniform","runtime":{
            "harness":"codex","provider":"openai","model":"replacement-model"
        }}))
        .assert_value(),
    );
    assert_eq!(
        client
            .run_resume(params)
            .await
            .assert_value()
            .current_part_id,
        RunId::new("successor")
    );
}

#[tokio::test]
async fn v2_only_peer_gets_one_physical_resume_and_never_a_foreign_guard() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use openengine_cluster_protocol::physical_runs;
    struct PhysicalPeer(AtomicUsize);
    #[async_trait]
    impl JsonRpcTransport for PhysicalPeer {
        async fn request(&self, request: String) -> Result<String, TransportError> {
            let request: Value = serde_json::from_str(&request).unwrap();
            let id = &request["id"];
            let params = &request["params"];
            let response = if request["method"] == "initialize" {
                assert_eq!(params["protocolVersion"], "");
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"Unsupported protocol version",
                    "data":{"code":"UNSUPPORTED_PROTOCOL_VERSION","details":{"supportedProtocolVersion":physical_runs::VERSION}}}})
            } else if params.get("protocolVersion").is_some() {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Invalid params"}})
            } else {
                assert_eq!(params, &json!({"runId":"source","successorRunId":"child"}));
                self.0.fetch_add(1, Ordering::Relaxed);
                json!({"jsonrpc":"2.0","id":id,"result":{"runId":"child","resumedFrom":"source"}})
            };
            Ok(response.to_string())
        }
    }
    let peer = PhysicalPeer(AtomicUsize::new(0));
    let client = ClusterClient::new(&peer);
    let mut params: RunResumeParams = serde_json::from_value(
        json!({"runId":"source", "partId":"source", "successorPartId":"child"}),
    )
    .unwrap();
    let receipt = client.run_resume(params.clone()).await.unwrap();
    assert_eq!(receipt.current_part_id.as_str(), "child");
    assert_eq!(receipt.resumed_from.as_str(), "source");
    params.part_id = RunId::new("foreign");
    assert!(client.run_resume(params).await.is_err());
    assert_eq!(peer.0.load(Ordering::Relaxed), 1);
}
