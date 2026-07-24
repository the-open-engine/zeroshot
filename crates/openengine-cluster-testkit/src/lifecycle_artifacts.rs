//! Generated operational lifecycle transcript fixtures.

use openengine_cluster_protocol::TurnFailureKind;
use openengine_cluster_server::lifecycle::{FailedCompletion, TurnId};
use serde_json::json;

use crate::admission_artifacts::{scripted_dispatcher, transcript};
use crate::artifacts::Artifact;

pub(crate) async fn generate_lifecycle_goldens() -> Vec<Artifact> {
    let (graph, dispatcher, store) = scripted_dispatcher(1);
    let mut artifact = transcript(
        "lifecycle-controls.ndjson",
        &dispatcher,
        vec![json!({
            "jsonrpc":"2.0","id":"lifecycle-apply","method":"apply",
            "params":{"graph":graph,"input":null,"ifGeneration":0,"idempotencyKey":"lifecycle-create"}
        })],
    )
    .await;

    let permit = store
        .acquire_dispatch(TurnId::new("lifecycle-turn"))
        .await
        .expect("golden dispatch succeeds");
    store
        .fail_dispatch(FailedCompletion {
            lease_id: permit.lease_id,
            kind: TurnFailureKind::Timeout,
        })
        .await
        .expect("golden dispatch failure succeeds");

    let remainder = transcript(
        "lifecycle-controls.ndjson",
        &dispatcher,
        vec![
            json!({
                "jsonrpc":"2.0","id":"lifecycle-retry","method":"retry",
                "params":{"ifGeneration":1,"idempotencyKey":"lifecycle-retry"}
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
        ],
    )
    .await;
    artifact.bytes.extend_from_slice(&remainder.bytes);
    vec![artifact]
}
