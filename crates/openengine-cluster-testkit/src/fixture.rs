//! Shared in-process fixture wiring for integration tests that need the transport-neutral
//! `Dispatcher` (and its backend) directly — for example to call `Dispatcher::watch` or
//! `ClusterBackend::watch` with an explicit queue capacity — rather than only a wrapped
//! `ClusterClient`.

use std::sync::Arc;

use openengine_cluster_client::{ClusterClient, InProcessTransport};
use openengine_cluster_server::admission::AdmissionCoordinator;
use openengine_cluster_server::{ConnectionContext, Dispatcher};

use crate::admission::{InMemoryAdmissionStore, ScriptedOutcome, ScriptedVerifier};

pub type FixtureBackend = AdmissionCoordinator<ScriptedVerifier, InMemoryAdmissionStore>;
pub type FixtureClient = ClusterClient<InProcessTransport<FixtureBackend>>;

/// Builds a fresh `ClusterClient`/`Dispatcher`/backend/verifier/store fixture wired to
/// `outcomes`. The returned client, dispatcher, and backend are cheap `Arc`-backed handles onto
/// the same underlying coordinator and store. The sole construction site for this fixture shape;
/// callers that only need a subset (for example just the client and store) destructure and
/// discard the rest rather than re-deriving it.
#[must_use]
pub fn dispatcher_fixture(
    outcomes: Vec<ScriptedOutcome>,
) -> (
    FixtureClient,
    Dispatcher<FixtureBackend>,
    FixtureBackend,
    Arc<ScriptedVerifier>,
    Arc<InMemoryAdmissionStore>,
) {
    let verifier = Arc::new(ScriptedVerifier::new(outcomes));
    let store = Arc::new(InMemoryAdmissionStore::default());
    let backend = AdmissionCoordinator::from_shared(Arc::clone(&verifier), Arc::clone(&store));
    let dispatcher = Dispatcher::new(backend.clone(), ConnectionContext::default());
    let client = ClusterClient::new(InProcessTransport::new(dispatcher.clone()));
    (client, dispatcher, backend, verifier, store)
}
