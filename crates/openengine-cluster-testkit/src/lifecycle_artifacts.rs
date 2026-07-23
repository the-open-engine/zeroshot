//! Generated operational lifecycle transcript fixtures.

use serde_json::json;

use crate::admission_artifacts::{scripted_dispatcher, transcript};
use crate::artifacts::Artifact;

pub(crate) async fn generate_lifecycle_goldens() -> Vec<Artifact> {
    let (graph, dispatcher, _store) = scripted_dispatcher(1);
    let requests = vec![
        json!({
            "jsonrpc":"2.0","id":"lifecycle-apply","method":"apply",
            "params":{"graph":graph,"input":null,"ifGeneration":0,"idempotencyKey":"lifecycle-create"}
        }),
        json!({
            "jsonrpc":"2.0","id":"lifecycle-update","method":"update",
            "params":{"labels":{"environment":"test"},"logLevel":"debug","suspended":false,
                "ifGeneration":1,"idempotencyKey":"lifecycle-update"}
        }),
        json!({
            "jsonrpc":"2.0","id":"lifecycle-stop","method":"stop",
            "params":{"mode":"drain","ifGeneration":1,"idempotencyKey":"lifecycle-stop"}
        }),
        json!({
            "jsonrpc":"2.0","id":"lifecycle-get","method":"get","params":{}
        }),
    ];
    vec![transcript("lifecycle-controls.ndjson", &dispatcher, requests).await]
}
