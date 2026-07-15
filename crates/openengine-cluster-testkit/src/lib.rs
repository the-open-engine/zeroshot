//! Deterministic fixtures for Cluster Protocol conformance tests.

pub mod artifacts;
mod negative_graph_fixtures;
mod schema_helpers;
pub mod worker_artifacts;
pub mod worker_profiles;

use async_trait::async_trait;
use openengine_cluster_protocol::{
    ClusterStatus, GetParams, GetResult, InitializeParams, InitializeResult, ServerCapabilities,
};
use openengine_cluster_server::{BackendError, ClusterBackend, ConnectionContext};

#[derive(Clone, Copy, Debug, Default)]
pub struct EmptyBackend;

#[async_trait]
impl ClusterBackend for EmptyBackend {
    async fn initialize(
        &self,
        _context: &ConnectionContext,
        _params: InitializeParams,
    ) -> Result<InitializeResult, BackendError> {
        Ok(InitializeResult::new(
            ServerCapabilities::default(),
            ClusterStatus::empty(),
        ))
    }

    async fn get(
        &self,
        _context: &ConnectionContext,
        _params: GetParams,
    ) -> Result<GetResult, BackendError> {
        Ok(GetResult {
            spec: None,
            status: ClusterStatus::empty(),
            at_cursor: None,
        })
    }
}
