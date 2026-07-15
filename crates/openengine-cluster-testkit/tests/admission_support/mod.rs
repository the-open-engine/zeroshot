use std::sync::Arc;

use openengine_cluster_client::{ClientError, ClusterClient, InProcessTransport};
use openengine_cluster_protocol::{ApplyParams, Generation, GraphSpec, IdempotencyKey};
use openengine_cluster_server::admission::AdmissionCoordinator;
use openengine_cluster_server::{ConnectionContext, Dispatcher};
use openengine_cluster_testkit::admission::{InMemoryAdmissionStore, ScriptedOutcome, ScriptedVerifier};

type FixtureBackend = AdmissionCoordinator<ScriptedVerifier, InMemoryAdmissionStore>;
type FixtureClient = ClusterClient<InProcessTransport<FixtureBackend>>;

pub fn client(
    outcomes: Vec<ScriptedOutcome>,
) -> (
    FixtureClient,
    Arc<ScriptedVerifier>,
    Arc<InMemoryAdmissionStore>,
) {
    let verifier = Arc::new(ScriptedVerifier::new(outcomes));
    let store = Arc::new(InMemoryAdmissionStore::default());
    let backend = AdmissionCoordinator::from_shared(Arc::clone(&verifier), Arc::clone(&store));
    let dispatcher = Dispatcher::new(backend, ConnectionContext::default());
    (
        ClusterClient::new(InProcessTransport::new(dispatcher)),
        verifier,
        store,
    )
}

pub fn committed(
    graph: GraphSpec,
    input: serde_json::Value,
    generation: u64,
    key: &str,
) -> ApplyParams {
    ApplyParams {
        graph,
        input: Some(input),
        dry_run: false,
        if_generation: Some(Generation::new(generation).unwrap()),
        idempotency_key: Some(IdempotencyKey::new(key).unwrap()),
    }
}

pub fn rpc_code(error: ClientError) -> String {
    match error {
        ClientError::Rpc(error) => error.data.expect("domain error data").code,
        other => panic!("expected RPC error, got {other}"),
    }
}
