//! Deterministic fixtures and backend for Cluster Protocol conformance tests.

pub mod artifacts;

use async_trait::async_trait;
use openengine_cluster_protocol::{GetParams, GetResult, InitializeParams, InitializeResult};
use openengine_cluster_server::{BackendError, ClusterBackend, ConnectionContext};

#[derive(Clone, Copy, Debug, Default)]
pub struct EmptyBackend;

#[async_trait]
impl ClusterBackend for EmptyBackend {
    async fn initialize(
        &self,
        _context: &ConnectionContext,
        _params: &InitializeParams,
    ) -> Result<InitializeResult, BackendError> {
        Ok(InitializeResult::empty())
    }

    async fn get(
        &self,
        _context: &ConnectionContext,
        _params: &GetParams,
    ) -> Result<GetResult, BackendError> {
        Ok(GetResult::empty())
    }
}
