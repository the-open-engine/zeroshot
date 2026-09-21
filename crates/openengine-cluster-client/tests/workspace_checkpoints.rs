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
                        "runId": "prior", "after": "entry-1", "limit": 1
                    })
                );
                json!({
                    "runId": "prior", "checkpoints": [{
                        "checkpointId": "entry-2", "sequence": 2, "node": "write",
                        "mapIndices": [], "loopIterations": [], "createdAt": 42
                    }], "nextAfter": "entry-2"
                })
            }
            Some("run/resume") => {
                assert_eq!(
                    request["params"],
                    json!({
                        "runId": "prior", "successorRunId": "successor",
                        "from": {"kind": "checkpoint", "checkpointId": "entry-2"}
                    })
                );
                json!({"runId": "successor", "resumedFrom": "prior"})
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
            run_id: result.run_id,
            successor_run_id: RunId::new("successor"),
            from: Some(RunResumeFrom::Checkpoint {
                checkpoint_id: checkpoint.checkpoint_id,
            }),
            connections: BTreeMap::new(),
            connection_resolver: None,
            github_token: None,
        })
        .await
        .assert_value();
    assert_eq!(successor.run_id, RunId::new("successor"));
    assert_eq!(successor.resumed_from, RunId::new("prior"));
}
