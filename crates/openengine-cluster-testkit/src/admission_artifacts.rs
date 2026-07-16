//! Generated admission transcript fixtures.

use openengine_cluster_server::admission::AdmissionCoordinator;
use openengine_cluster_server::{ClusterBackend, ConnectionContext, Dispatcher};
use openengine_cluster_protocol::GraphSpec;
use serde_json::{json, Value};

use crate::admission::{
    compiled_from_graph_fixture, graph_fixture, InMemoryAdmissionStore, ScriptedOutcome,
    ScriptedVerifier,
};
use crate::artifacts::Artifact;

const ROOT: &str = "protocol/openengine-cluster/v1";

pub(crate) async fn generate_admission_goldens() -> Vec<Artifact> {
    let (graph, dispatcher) = scripted_dispatcher(2);
    let lifecycle_requests = vec![
        json!({
            "jsonrpc":"2.0","id":"admission-init","method":"initialize",
            "params":{"protocolVersion":"openengine.cluster/v1"}
        }),
        json!({
            "jsonrpc":"2.0","id":"admission-plan","method":"plan",
            "params":{"graph":graph}
        }),
        json!({
            "jsonrpc":"2.0","id":"admission-apply","method":"apply",
            "params":{"graph":graph,"input":null,"ifGeneration":0,"idempotencyKey":"golden-create"}
        }),
        json!({
            "jsonrpc":"2.0","id":"admission-get","method":"get","params":{}
        }),
    ];
    let error_requests = vec![
        json!({
            "jsonrpc":"2.0","id":"dry-run-key","method":"apply",
            "params":{"graph":graph,"dryRun":true,"idempotencyKey":"forbidden"}
        }),
        json!({
            "jsonrpc":"2.0","id":"key-reuse","method":"apply",
            "params":{"graph":graph,"input":null,"ifGeneration":1,"idempotencyKey":"golden-create"}
        }),
    ];
    vec![
        transcript(
            "admission-lifecycle.ndjson",
            &dispatcher,
            lifecycle_requests,
        )
        .await,
        transcript("admission-errors.ndjson", &dispatcher, error_requests).await,
    ]
}

pub(crate) type ScriptedDispatcher =
    Dispatcher<AdmissionCoordinator<ScriptedVerifier, InMemoryAdmissionStore>>;

pub(crate) fn scripted_dispatcher(approvals: usize) -> (GraphSpec, ScriptedDispatcher) {
    let graph = graph_fixture("worker", json!({"kind":"null"}));
    let compiled = compiled_from_graph_fixture(&graph);
    let outcomes = (0..approvals)
        .map(|_| ScriptedOutcome::approve(compiled.clone(), vec![]))
        .collect();
    let backend = AdmissionCoordinator::new(
        ScriptedVerifier::new(outcomes),
        InMemoryAdmissionStore::default(),
    );
    (
        graph,
        Dispatcher::new(backend, ConnectionContext::default()),
    )
}

pub(crate) async fn transcript<B>(
    name: &str,
    dispatcher: &Dispatcher<B>,
    requests: Vec<Value>,
) -> Artifact
where
    B: ClusterBackend,
{
    let mut bytes = Vec::new();
    for request in requests {
        let request = request.to_string();
        let response = dispatcher.dispatch(&request).await;
        bytes.extend_from_slice(request.as_bytes());
        bytes.push(b'\n');
        bytes.extend_from_slice(response.as_bytes());
        bytes.push(b'\n');
    }
    Artifact {
        relative_path: format!("{ROOT}/goldens/{name}"),
        bytes,
    }
}
