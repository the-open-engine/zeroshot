//! Composition root for the integrated native-v2 capsule candidate.
//!
//! The cloud controller owns admission, durability, observation, and runtime allocation. Inside
//! one allocated capsule this module binds the graph-wide agent harness together with the
//! trusted Git delivery lane and hands the resulting runner to the private capsule transport.

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::native_v2_claude::{ClaudeAdapter, ClaudeAdapterConfig, ClaudeAdapterConfigError};
use crate::native_v2_codex::{NativeV2CodexAdapter, NativeV2CodexConfig};
use crate::native_v2_copilot::{CopilotAdapter, CopilotConfig};
use crate::native_v2_contract::{AdmittedRun, NodeInvocation, NodeRuntimeBinding, RuntimePlan};
use crate::native_v2_delivery::{
    GitHubDeliveryAuthority, NativeV2DeliveryAdapter, NativeV2DeliveryConfig,
};
use crate::native_v2_runner::{
    DriverControl, DriverInvocation, NativeNodeRunner, NodeDriver, NodeRunnerError, NodeSession,
    ResolvedEnvironment, SessionFactory,
};

#[cfg(test)]
#[path = "native_v2_candidate/tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) mod test_support;

/// The one harness/provider lane selected for the entire graph.
pub enum NativeV2HarnessConfig {
    Copilot(CopilotConfig),
    Codex(NativeV2CodexConfig),
    Claude(ClaudeAdapterConfig),
}

pub struct NativeV2CandidateConfig {
    pub harness: NativeV2HarnessConfig,
    pub delivery: NativeV2DeliveryConfig,
    pub github: Arc<dyn GitHubDeliveryAuthority>,
}

#[derive(Clone, Copy)]
enum CandidatePlacement {
    Capsule,
    Local(SessionBoundary),
}

#[derive(Clone, Copy)]
enum SessionBoundary {
    Run,
    Owner,
}

impl CandidatePlacement {
    fn is_local(self) -> bool {
        matches!(self, Self::Local(_))
    }

    fn session_boundary(self) -> SessionBoundary {
        match self {
            Self::Capsule | Self::Local(SessionBoundary::Run) => SessionBoundary::Run,
            Self::Local(SessionBoundary::Owner) => SessionBoundary::Owner,
        }
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum NativeV2CandidateError {
    #[error("candidate harness/provider does not match the admitted graph runtime")]
    RuntimeMismatch,
    #[error("agent and delivery adapters must use the same run workspace")]
    WorkspaceMismatch,
    #[error(transparent)]
    Claude(#[from] ClaudeAdapterConfigError),
    #[error(transparent)]
    Runner(#[from] NodeRunnerError),
}

/// Builds the complete capsule-local runner for one admitted native-v2 run.
pub fn build_native_v2_candidate(
    admitted: &AdmittedRun,
    config: NativeV2CandidateConfig,
) -> Result<NativeNodeRunner, NativeV2CandidateError> {
    build_candidate(admitted, config, CandidatePlacement::Capsule, None)
}

/// Hosted candidate with a trusted Git credential visible only to checkout and delivery.
pub fn build_native_v2_candidate_with_github_token(
    admitted: &AdmittedRun,
    config: NativeV2CandidateConfig,
    github_token: Option<Arc<str>>,
) -> Result<NativeNodeRunner, NativeV2CandidateError> {
    build_candidate(admitted, config, CandidatePlacement::Capsule, github_token)
}

/// Builds the same candidate with child processes running as the invoking local user.
pub fn build_local_native_v2_candidate(
    admitted: &AdmittedRun,
    config: NativeV2CandidateConfig,
) -> Result<NativeNodeRunner, NativeV2CandidateError> {
    build_candidate(
        admitted,
        config,
        CandidatePlacement::Local(SessionBoundary::Run),
        None,
    )
}

/// Local equivalent of [`build_native_v2_candidate_with_github_token`].
pub fn build_local_native_v2_candidate_with_github_token(
    admitted: &AdmittedRun,
    config: NativeV2CandidateConfig,
    github_token: Option<Arc<str>>,
) -> Result<NativeNodeRunner, NativeV2CandidateError> {
    build_candidate(
        admitted,
        config,
        CandidatePlacement::Local(SessionBoundary::Run),
        github_token,
    )
}

/// Local candidate whose reusable node sessions survive multiple supervisor runs.
pub(crate) fn build_local_owner_native_v2_candidate(
    admitted: &AdmittedRun,
    config: NativeV2CandidateConfig,
) -> Result<NativeNodeRunner, NativeV2CandidateError> {
    build_candidate(
        admitted,
        config,
        CandidatePlacement::Local(SessionBoundary::Owner),
        None,
    )
}

fn build_candidate(
    admitted: &AdmittedRun,
    config: NativeV2CandidateConfig,
    placement: CandidatePlacement,
    github_token: Option<Arc<str>>,
) -> Result<NativeNodeRunner, NativeV2CandidateError> {
    validate_config(admitted, &config)?;
    let delivery = Arc::new(
        NativeV2DeliveryAdapter::new(config.delivery, config.github)
            .with_trusted_github_token(github_token),
    );
    match config.harness {
        NativeV2HarnessConfig::Copilot(config) => {
            let agent = Arc::new(if placement.is_local() {
                CopilotAdapter::new_local(config)
            } else {
                CopilotAdapter::new(config)
            });
            assemble_runner(admitted, CandidateAgents::new(agent), delivery, placement)
        }
        NativeV2HarnessConfig::Codex(config) => {
            let agent = Arc::new(if placement.is_local() {
                NativeV2CodexAdapter::new_local(config)
            } else {
                NativeV2CodexAdapter::new(config)
            });
            assemble_runner(admitted, CandidateAgents::new(agent), delivery, placement)
        }
        NativeV2HarnessConfig::Claude(config) => {
            let agent = Arc::new(if placement.is_local() {
                ClaudeAdapter::new_local(config)?
            } else {
                ClaudeAdapter::new(config)?
            });
            assemble_runner(admitted, CandidateAgents::new(agent), delivery, placement)
        }
    }
}

fn validate_config(
    admitted: &AdmittedRun,
    config: &NativeV2CandidateConfig,
) -> Result<(), NativeV2CandidateError> {
    let workspace_matches = match (&admitted.runtime, &config.harness) {
        (RuntimePlan::Copilot { .. }, NativeV2HarnessConfig::Copilot(harness)) => {
            harness.workspace == config.delivery.workspace
        }
        (RuntimePlan::Codex { provider, .. }, NativeV2HarnessConfig::Codex(harness))
            if provider == &harness.provider =>
        {
            harness.workspace == config.delivery.workspace
        }
        (RuntimePlan::Claude { provider, .. }, NativeV2HarnessConfig::Claude(harness))
            if provider == &harness.provider =>
        {
            harness.workspace == config.delivery.workspace
        }
        _ => return Err(NativeV2CandidateError::RuntimeMismatch),
    };
    if !workspace_matches {
        return Err(NativeV2CandidateError::WorkspaceMismatch);
    }
    Ok(())
}

fn assemble_runner(
    admitted: &AdmittedRun,
    agents: CandidateAgents,
    delivery: Arc<NativeV2DeliveryAdapter>,
    placement: CandidatePlacement,
) -> Result<NativeNodeRunner, NativeV2CandidateError> {
    let lane = Arc::new(CandidateNodeLane {
        agent_driver: agents.driver,
        agent_sessions: agents.sessions,
        delivery,
    });
    Ok(match placement.session_boundary() {
        SessionBoundary::Run => NativeNodeRunner::new(admitted, lane.clone(), lane)?,
        SessionBoundary::Owner => NativeNodeRunner::new_owner_scoped(admitted, lane.clone(), lane)?,
    })
}

struct CandidateAgents {
    driver: Arc<dyn NodeDriver>,
    sessions: Arc<dyn SessionFactory>,
}

impl CandidateAgents {
    fn new<T>(agent: Arc<T>) -> Self
    where
        T: NodeDriver + SessionFactory + 'static,
    {
        Self {
            driver: agent.clone(),
            sessions: agent,
        }
    }
}

/// Routes only by the admitted closed binding: agent nodes use the graph-wide harness and the
/// graph-visible delivery verifier uses trusted Git delivery.
struct CandidateNodeLane {
    agent_driver: Arc<dyn NodeDriver>,
    agent_sessions: Arc<dyn SessionFactory>,
    delivery: Arc<NativeV2DeliveryAdapter>,
}

#[async_trait]
impl SessionFactory for CandidateNodeLane {
    async fn open(
        &self,
        invocation: &NodeInvocation,
        environment: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        match &invocation.binding {
            NodeRuntimeBinding::Agent { .. } => {
                self.agent_sessions.open(invocation, environment).await
            }
            NodeRuntimeBinding::GitDelivery { .. } => {
                self.delivery.open(invocation, environment).await
            }
        }
    }
}

#[async_trait]
impl NodeDriver for CandidateNodeLane {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<openengine_cluster_protocol::WorkerOutcome, NodeRunnerError> {
        match &invocation.node.binding {
            NodeRuntimeBinding::Agent { .. } => self.agent_driver.run(invocation, control).await,
            NodeRuntimeBinding::GitDelivery { .. } => self.delivery.run(invocation, control).await,
        }
    }
}
