use async_trait::async_trait;
use openengine_cluster_client::{ClientError, ClusterClient, JsonRpcTransport, TransportError};
use openengine_cluster_protocol::{Generation, IdempotencyKey, StopMode, StopParams, UpdateParams};
use serde_json::{json, Value};
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::Mutex;

#[derive(Clone)]
struct ScriptedTransport {
    requests: Arc<Mutex<Vec<Value>>>,
    results: Arc<Mutex<VecDeque<Value>>>,
}

impl ScriptedTransport {
    fn new(results: impl IntoIterator<Item = Value>) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            results: Arc::new(Mutex::new(results.into_iter().collect())),
        }
    }
}

#[async_trait]
impl JsonRpcTransport for ScriptedTransport {
    async fn request(&self, request: String) -> Result<String, TransportError> {
        let request: Value = serde_json::from_str(&request).unwrap();
        self.requests.lock().await.push(request.clone());
        let result = self.results.lock().await.pop_front().unwrap();
        Ok(json!({"jsonrpc":"2.0","id":request["id"],"result":result}).to_string())
    }
}

fn generation() -> Generation {
    Generation::new(1).unwrap()
}

#[tokio::test]
async fn lifecycle_calls_use_typed_update_and_stop_contracts() {
    let transport = ScriptedTransport::new([
        json!({
            "generation":1,"runId":"run-1","phase":"running",
            "operational":{"labels":{},"logLevel":"info","dispatchState":"suspended","inFlight":0},
            "atCursor":"cursor-2","deduped":false
        }),
        json!({
            "generation":1,"runId":"run-1","phase":"finished",
            "acceptedMode":"force","effectiveMode":"force",
            "operational":{"labels":{},"logLevel":"info","dispatchState":"stopped","stopMode":"force","inFlight":0},
            "atCursor":"cursor-4","deduped":false
        }),
    ]);
    let client = ClusterClient::new(transport.clone());

    let update = client
        .update(UpdateParams {
            labels: None,
            log_level: None,
            suspended: Some(true),
            if_generation: generation(),
            idempotency_key: IdempotencyKey::new("suspend").unwrap(),
        })
        .await
        .unwrap();
    let stop = client
        .stop(StopParams {
            mode: StopMode::Force,
            if_generation: generation(),
            idempotency_key: IdempotencyKey::new("force").unwrap(),
        })
        .await
        .unwrap();

    assert_eq!(update.at_cursor.as_str(), "cursor-2");
    assert_eq!(stop.effective_mode, StopMode::Force);
    let requests = transport.requests.lock().await;
    assert_eq!(requests[0]["method"], "update");
    assert_eq!(requests[0]["params"]["ifGeneration"], 1);
    assert_eq!(requests[0]["params"]["idempotencyKey"], "suspend");
    assert_eq!(requests[1]["method"], "stop");
    assert_eq!(requests[1]["params"]["mode"], "force");
}

#[derive(Clone)]
struct WrongResponseIdentity;

#[async_trait]
impl JsonRpcTransport for WrongResponseIdentity {
    async fn request(&self, _request: String) -> Result<String, TransportError> {
        Ok(json!({
            "jsonrpc":"2.0","id":999,
            "result":{
                "generation":1,"runId":"run-1","phase":"running",
                "operational":{"labels":{},"logLevel":"info","dispatchState":"suspended","inFlight":0},
                "atCursor":"cursor-2","deduped":false
            }
        })
        .to_string())
    }
}

#[tokio::test]
async fn lifecycle_rejects_wrong_response_identity() {
    let error = ClusterClient::new(WrongResponseIdentity)
        .update(UpdateParams {
            labels: None,
            log_level: None,
            suspended: Some(true),
            if_generation: generation(),
            idempotency_key: IdempotencyKey::new("suspend").unwrap(),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ClientError::InvalidResponse(message) if message.contains("response id mismatch")
    ));
}
