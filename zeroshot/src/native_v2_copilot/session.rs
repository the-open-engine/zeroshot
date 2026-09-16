use std::sync::Arc;
use async_trait::async_trait;
use openengine_cluster_protocol::WorkerOutcome;
use tokio::sync::Mutex;
use crate::native_v2_capsule::provider_process::{ProviderSessionCore, impl_provider_node_session};
use crate::native_v2_contract::{NodeInvocation, NodeRuntimeBinding};
use crate::native_v2_runner::{
    DriverControl, DriverInvocation, NodeDriver, NodeRunnerError, NodeSession, ResolvedEnvironment,
    SessionFactory,
};
use super::CopilotAdapter;

pub(super) struct CopilotSession {
    pub(super) core: ProviderSessionCore,
    pub(super) id: Mutex<Option<String>>,
}

impl_provider_node_session!(CopilotSession);

#[async_trait]
impl SessionFactory for CopilotAdapter {
    async fn open(
        &self,
        invocation: &NodeInvocation,
        _environment: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        if !matches!(invocation.binding, NodeRuntimeBinding::Agent { .. }) {
            return Err(NodeRunnerError::SessionOpen);
        }
        Ok(Arc::new(CopilotSession {
            core: ProviderSessionCore::new(),
            id: Mutex::new(None),
        }))
    }
}

#[async_trait]
impl NodeDriver for CopilotAdapter {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let session = invocation
            .session
            .as_any()
            .downcast_ref::<CopilotSession>()
            .ok_or(NodeRunnerError::Driver)?;
        self.run_turn(&invocation, session, &control).await
    }
}
