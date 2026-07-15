use async_trait::async_trait;
use openengine_cluster_client::{ClusterClient, JsonRpcTransport, TransportError};
use openengine_cluster_protocol::{ApplyParams, GetParams, PlanParams};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct RecordingTransport {
    methods: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl JsonRpcTransport for RecordingTransport {
    async fn request(&self, request: String) -> Result<String, TransportError> {
        let request: serde_json::Value = serde_json::from_str(&request).unwrap();
        let method = request["method"].as_str().unwrap().to_owned();
        self.methods.lock().await.push(method.clone());
        let result = match method.as_str() {
            "plan" => json!({"ok":false,"diagnostics":[]}),
            "apply" => json!({
                "generation":null,"runId":null,"phase":"empty","deduped":false,
                "diff":{"added":["worker"],"removed":[],"changed":[]}
            }),
            "get" => json!({
                "spec":null,
                "status":{"phase":"empty","observedGeneration":null,"currentRunId":null,"atCursor":null},
                "atCursor":null
            }),
            _ => unreachable!(),
        };
        Ok(json!({"jsonrpc":"2.0","id":request["id"],"result":result}).to_string())
    }
}

fn graph() -> openengine_cluster_protocol::GraphSpec {
    serde_json::from_str(include_str!(
        "../../../protocol/openengine-cluster/v1/fixtures/graph/positive/single-worker.json"
    ))
    .unwrap()
}

#[tokio::test]
async fn typed_admission_calls_use_named_plan_apply_and_get_methods() {
    let transport = RecordingTransport::default();
    let client = ClusterClient::new(transport.clone());
    assert!(!client.plan(PlanParams { graph: graph() }).await.unwrap().ok);
    assert_eq!(
        client
            .apply(ApplyParams {
                graph: graph(),
                input: None,
                dry_run: true,
                if_generation: None,
                idempotency_key: None,
            })
            .await
            .unwrap()
            .diff
            .unwrap()
            .added[0]
            .as_str(),
        "worker"
    );
    assert!(
        client
            .get(GetParams::default())
            .await
            .unwrap()
            .spec
            .is_none()
    );
    assert_eq!(*transport.methods.lock().await, ["plan", "apply", "get"]);
}
