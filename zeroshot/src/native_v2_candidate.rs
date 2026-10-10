//! Composition root for the integrated native-v2 capsule candidate.
//!
//! The cloud controller owns admission, durability, observation, and runtime allocation. Inside
//! one allocated capsule this module binds one agent adapter per runtime lane together with the
//! trusted Git delivery lane and hands the resulting runner to the private capsule transport.

#[path = "native_v2_candidate/provider_access.rs"]
mod provider_access;
pub(crate) use provider_access::{ProviderAccessPlacement, materialize_provider_access};

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::native_v2_claude::{ClaudeAdapter, ClaudeAdapterConfig, ClaudeAdapterConfigError};
use crate::native_v2_codex::{NativeV2CodexAdapter, NativeV2CodexConfig};
use crate::native_v2_copilot::{CopilotAdapter, CopilotConfig};
use crate::native_v2_contract::{
    AdmittedRun, CopilotProvider, NodeInvocation, NodeRuntimeBinding, RuntimeLane,
};
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

/// Configuration of one lane's agent adapter: a harness and the provider it is fixed to.
pub enum NativeV2HarnessConfig {
    Copilot(CopilotConfig),
    Codex(NativeV2CodexConfig),
    Claude(ClaudeAdapterConfig),
}

impl NativeV2HarnessConfig {
    #[must_use]
    pub fn lane(&self) -> RuntimeLane {
        match self {
            Self::Copilot(_) => RuntimeLane::Copilot {
                provider: CopilotProvider::Github,
            },
            Self::Codex(config) => RuntimeLane::Codex {
                provider: config.provider,
            },
            Self::Claude(config) => RuntimeLane::Claude {
                provider: config.provider,
            },
        }
    }

    fn workspace(&self) -> &Path {
        match self {
            Self::Copilot(config) => &config.workspace,
            Self::Codex(config) => &config.workspace,
            Self::Claude(config) => &config.workspace,
        }
    }
}

pub struct NativeV2CandidateConfig {
    pub lanes: Vec<NativeV2HarnessConfig>,
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
    #[error("candidate lanes do not match the admitted graph runtime")]
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
    let NativeV2CandidateConfig {
        lanes,
        delivery,
        github,
    } = config;
    let delivery = Arc::new(
        NativeV2DeliveryAdapter::new(delivery, github).with_trusted_github_token(github_token),
    );
    let agents = lane_agents(lanes, placement)?;
    assemble_runner(admitted, agents, delivery, placement)
}

fn lane_agents(
    lanes: Vec<NativeV2HarnessConfig>,
    placement: CandidatePlacement,
) -> Result<BTreeMap<RuntimeLane, CandidateAgents>, NativeV2CandidateError> {
    lanes
        .into_iter()
        .map(|harness| Ok((harness.lane(), build_lane_agents(harness, placement)?)))
        .collect()
}

fn build_lane_agents(
    harness: NativeV2HarnessConfig,
    placement: CandidatePlacement,
) -> Result<CandidateAgents, NativeV2CandidateError> {
    Ok(match harness {
        NativeV2HarnessConfig::Copilot(config) => {
            CandidateAgents::new(Arc::new(if placement.is_local() {
                CopilotAdapter::new_local(config)
            } else {
                CopilotAdapter::new(config)
            }))
        }
        NativeV2HarnessConfig::Codex(config) => {
            CandidateAgents::new(Arc::new(if placement.is_local() {
                NativeV2CodexAdapter::new_local(config)
            } else {
                NativeV2CodexAdapter::new(config)
            }))
        }
        NativeV2HarnessConfig::Claude(config) => {
            CandidateAgents::new(Arc::new(if placement.is_local() {
                ClaudeAdapter::new_local(config)?
            } else {
                ClaudeAdapter::new(config)?
            }))
        }
    })
}

fn validate_config(
    admitted: &AdmittedRun,
    config: &NativeV2CandidateConfig,
) -> Result<(), NativeV2CandidateError> {
    let mut lanes = BTreeSet::new();
    for harness in &config.lanes {
        if !lanes.insert(harness.lane()) {
            return Err(NativeV2CandidateError::RuntimeMismatch);
        }
    }
    if lanes != admitted.runtime.lanes() {
        return Err(NativeV2CandidateError::RuntimeMismatch);
    }
    if config
        .lanes
        .iter()
        .any(|harness| harness.workspace() != config.delivery.workspace.as_path())
    {
        return Err(NativeV2CandidateError::WorkspaceMismatch);
    }
    Ok(())
}

fn assemble_runner(
    admitted: &AdmittedRun,
    agents: BTreeMap<RuntimeLane, CandidateAgents>,
    delivery: Arc<NativeV2DeliveryAdapter>,
    placement: CandidatePlacement,
) -> Result<NativeNodeRunner, NativeV2CandidateError> {
    let routes = Arc::new(CandidateNodeLane {
        default_lane: admitted.runtime.lane(),
        agents,
        delivery,
    });
    Ok(match placement.session_boundary() {
        SessionBoundary::Run => NativeNodeRunner::new(admitted, routes.clone(), routes)?,
        SessionBoundary::Owner => {
            NativeNodeRunner::new_owner_scoped(admitted, routes.clone(), routes)?
        }
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

/// Routes only by the admitted closed binding: agent nodes use their effective lane's adapter,
/// never another lane's, and the graph-visible delivery verifier uses trusted Git delivery.
struct CandidateNodeLane {
    default_lane: RuntimeLane,
    agents: BTreeMap<RuntimeLane, CandidateAgents>,
    delivery: Arc<NativeV2DeliveryAdapter>,
}

impl CandidateNodeLane {
    fn agents_for(
        &self,
        binding: &NodeRuntimeBinding,
    ) -> Result<&CandidateAgents, NodeRunnerError> {
        match binding {
            NodeRuntimeBinding::Agent { lane, .. } => self
                .agents
                .get(&lane.unwrap_or(self.default_lane))
                .ok_or(NodeRunnerError::Driver),
            NodeRuntimeBinding::GitDelivery { .. } => Err(NodeRunnerError::Driver),
        }
    }
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
                self.agents_for(&invocation.binding)?
                    .sessions
                    .open(invocation, environment)
                    .await
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
            NodeRuntimeBinding::Agent { .. } => {
                let agents = self.agents_for(&invocation.node.binding)?;
                agents.driver.run(invocation, control).await
            }
            NodeRuntimeBinding::GitDelivery { .. } => self.delivery.run(invocation, control).await,
        }
    }
}
