use openengine_cluster_server::stdio::serve_stdio;
use openengine_cluster_server::admission::AdmissionCoordinator;
use openengine_cluster_server::{ConnectionContext, Dispatcher};
use openengine_cluster_testkit::admission::{
    compiled_from_graph_fixture, graph_fixture, InMemoryAdmissionStore, ScriptedOutcome,
    ScriptedVerifier,
};
use serde_json::json;

#[tokio::main]
async fn main() {
    let graph = graph_fixture("worker", json!({"kind":"null"}));
    let compiled = compiled_from_graph_fixture(&graph);
    let outcomes = (0..128)
        .map(|_| ScriptedOutcome::approve(compiled.clone(), vec![]))
        .collect();
    let backend = AdmissionCoordinator::new(
        ScriptedVerifier::new(outcomes),
        InMemoryAdmissionStore::default(),
    );
    let dispatcher = Dispatcher::new(backend, ConnectionContext::default());
    if let Err(error) = serve_stdio(dispatcher).await {
        eprintln!("cluster protocol stdio server failed: {error}");
        std::process::exit(1);
    }
}
