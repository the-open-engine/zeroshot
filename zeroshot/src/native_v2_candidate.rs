//! Composition root for the integrated native-v2 capsule candidate.
//!
//! The cloud controller owns admission, durability, observation, and runtime allocation. Inside
//! one allocated capsule this module binds one agent adapter per runtime lane together with the
//! trusted Git delivery lane and hands the resulting runner to the private capsule transport.

#[path = "native_v2_candidate/provider_access.rs"]
mod provider_access;
pub(crate) use provider_access::{ProviderAccessPlacement, materialize_provider_access};

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use openengine_cluster_protocol::GitTransportCredentials;
use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::native_v2_comment_delivery::{
    NativeV2CommentDeliveryAdapter, NativeV2CommentDeliveryConfig, CommentPublicationStore,
};
use crate::source_code_provider::SourceCommentAuthority;
use crate::native_v2_claude::{ClaudeAdapter, ClaudeAdapterConfig, ClaudeAdapterConfigError};
use crate::native_v2_codex::{NativeV2CodexAdapter, NativeV2CodexConfig};
use crate::native_v2_copilot::{CopilotAdapter, CopilotConfig};
use crate::native_v2_pi::{PiAdapter, PiAdapterConfigError, PiConfig};
use crate::native_v2_contract::{
    AdmittedRun, CopilotProvider, NodeInvocation, NodeRuntimeBinding, RuntimeLane,
};
use crate::native_v2_delivery::validation::TrustedDeliveryValidator;
use crate::native_v2_delivery::{
    SourceDeliveryAuthority, NativeV2DeliveryAdapter, NativeV2DeliveryConfig,
    TrustedGitHubTokenFile,
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
    Pi(PiConfig),
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
            Self::Pi(config) => RuntimeLane::Pi {
                provider: config.provider,
            },
        }
    }

    fn workspace(&self) -> &Path {
        match self {
            Self::Copilot(config) => &config.workspace,
            Self::Codex(config) => &config.workspace,
            Self::Claude(config) => &config.workspace,
            Self::Pi(config) => &config.workspace,
        }
    }
}

// Library embedders may supply a host-owned recall capability; storage and orchestration stay internal.
pub use crate::active_memory::hooks::{ActiveMemoryHooks, ActiveMemorySource, HookQuery};

pub struct NativeV2CandidateConfig {
    pub active_memory: Option<Arc<ActiveMemoryHooks>>,
    pub lanes: Vec<NativeV2HarnessConfig>,
    pub workspace: PathBuf,
    pub delivery: Option<NativeV2CandidateDelivery>,
    pub comment_delivery: Option<NativeV2CandidateCommentDelivery>,
}

pub struct NativeV2CandidateCommentDelivery {
    pub config: NativeV2CommentDeliveryConfig,
    pub authority: Arc<dyn SourceCommentAuthority>,
    pub store: Arc<dyn CommentPublicationStore>,
}

/// All delivery authority is present together, only when the graph declares delivery.
pub struct NativeV2CandidateDelivery {
    pub config: NativeV2DeliveryConfig,
    pub authority: Arc<dyn SourceDeliveryAuthority>,
    pub git_credentials: Option<GitTransportCredentials>,
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
    Pi(#[from] PiAdapterConfigError),
    #[error(transparent)]
    Runner(#[from] NodeRunnerError),
}

/// Builds the complete capsule-local runner for one admitted native-v2 run.
pub fn build_native_v2_candidate(
    admitted: &AdmittedRun,
    config: NativeV2CandidateConfig,
) -> Result<NativeNodeRunner, NativeV2CandidateError> {
    build_candidate(
        admitted,
        config,
        CandidatePlacement::Capsule,
        TrustedDeliveryCapabilities::default(),
    )
}

/// Hosted candidate with a trusted Git credential visible only to checkout and delivery.
pub fn build_native_v2_candidate_with_github_token(
    admitted: &AdmittedRun,
    config: NativeV2CandidateConfig,
    github_token: Option<Arc<str>>,
) -> Result<NativeNodeRunner, NativeV2CandidateError> {
    build_candidate(
        admitted,
        config,
        CandidatePlacement::Capsule,
        TrustedDeliveryCapabilities {
            token: github_token,
            ..Default::default()
        },
    )
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
        TrustedDeliveryCapabilities::default(),
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
        TrustedDeliveryCapabilities {
            token: github_token,
            ..Default::default()
        },
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
        TrustedDeliveryCapabilities::default(),
    )
}

#[derive(Default)]
pub(crate) struct TrustedDeliveryCapabilities {
    pub(crate) token: Option<Arc<str>>,
    pub(crate) token_file: Option<TrustedGitHubTokenFile>,
    pub(crate) validator: Option<TrustedDeliveryValidator>,
}

pub(crate) fn build_local_native_v2_candidate_with_authority(
    admitted: &AdmittedRun,
    config: NativeV2CandidateConfig,
    authority: TrustedDeliveryCapabilities,
) -> Result<NativeNodeRunner, NativeV2CandidateError> {
    build_candidate(
        admitted,
        config,
        CandidatePlacement::Local(SessionBoundary::Run),
        authority,
    )
}

fn build_candidate(
    admitted: &AdmittedRun,
    config: NativeV2CandidateConfig,
    placement: CandidatePlacement,
    trusted: TrustedDeliveryCapabilities,
) -> Result<NativeNodeRunner, NativeV2CandidateError> {
    validate_config(admitted, &config)?;
    let NativeV2CandidateConfig {
        lanes,
        delivery,
        active_memory,
        comment_delivery,
        ..
    } = config;
    let delivery = delivery.map(|delivery| {
        Arc::new(
            NativeV2DeliveryAdapter::new(delivery.config, delivery.authority)
                .with_trusted_github_token(trusted.token.clone())
                .with_trusted_github_token_file(trusted.token_file.clone())
                .with_trusted_delivery_validator(trusted.validator)
                .with_git_credentials(delivery.git_credentials),
        )
    });
    let agents = lane_agents(lanes, placement, active_memory)?;
    let comment_delivery = comment_delivery.map(|delivery| {
        Arc::new(
            NativeV2CommentDeliveryAdapter::new(
                delivery.config,
                delivery.authority,
                delivery.store,
            )
            .with_trusted_github_token(trusted.token)
            .with_trusted_github_token_file(trusted.token_file),
        )
    });
    assemble_runner(
        admitted,
        agents,
        CandidateDeliveries {
            git: delivery,
            comment: comment_delivery,
        },
        placement,
    )
}

fn lane_agents(
    lanes: Vec<NativeV2HarnessConfig>,
    placement: CandidatePlacement,
    memory: Option<Arc<crate::active_memory::hooks::ActiveMemoryHooks>>,
) -> Result<BTreeMap<RuntimeLane, CandidateAgents>, NativeV2CandidateError> {
    lanes
        .into_iter()
        .map(|harness| {
            Ok((
                harness.lane(),
                build_lane_agents(harness, placement, memory.clone())?,
            ))
        })
        .collect()
}

fn build_lane_agents(
    harness: NativeV2HarnessConfig,
    placement: CandidatePlacement,
    memory: Option<Arc<crate::active_memory::hooks::ActiveMemoryHooks>>,
) -> Result<CandidateAgents, NativeV2CandidateError> {
    let on_failure = memory.clone();
    let mut agents = match harness {
        NativeV2HarnessConfig::Copilot(config) => CandidateAgents::new(Arc::new(
            (if placement.is_local() {
                CopilotAdapter::new_local(config)
            } else {
                CopilotAdapter::new(config)
            })
            .with_active_memory(memory),
        )),
        NativeV2HarnessConfig::Codex(config) => CandidateAgents::new(Arc::new(
            (if placement.is_local() {
                NativeV2CodexAdapter::new_local(config)
            } else {
                NativeV2CodexAdapter::new(config)
            })
            .with_active_memory(memory),
        )),
        NativeV2HarnessConfig::Claude(config) => CandidateAgents::new(Arc::new(
            (if placement.is_local() {
                ClaudeAdapter::new_local(config)?
            } else {
                ClaudeAdapter::new(config)?
            })
            .with_active_memory(memory),
        )),
        NativeV2HarnessConfig::Pi(config) => CandidateAgents::new(Arc::new(
            (if placement.is_local() {
                PiAdapter::new_local(config)?
            } else {
                PiAdapter::new(config)?
            })
            .with_active_memory(memory),
        )),
    };
    agents.memory = on_failure;
    Ok(agents)
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
        .any(|harness| harness.workspace() != config.workspace.as_path())
        || config
            .delivery
            .as_ref()
            .is_some_and(|delivery| delivery.config.workspace != config.workspace)
    {
        return Err(NativeV2CandidateError::WorkspaceMismatch);
    }
    let declares_delivery = admitted
        .runtime
        .nodes()
        .values()
        .any(|binding| matches!(binding, NodeRuntimeBinding::GitDelivery { .. }));
    let declares_comment = admitted
        .runtime
        .nodes()
        .values()
        .any(|binding| matches!(binding, NodeRuntimeBinding::CommentDelivery { .. }));
    if declares_delivery != config.delivery.is_some()
        || declares_comment != config.comment_delivery.is_some()
    {
        return Err(NativeV2CandidateError::RuntimeMismatch);
    }
    Ok(())
}

fn assemble_runner(
    admitted: &AdmittedRun,
    agents: BTreeMap<RuntimeLane, CandidateAgents>,
    deliveries: CandidateDeliveries,
    placement: CandidatePlacement,
) -> Result<NativeNodeRunner, NativeV2CandidateError> {
    let routes = Arc::new(CandidateNodeLane {
        default_lane: admitted.runtime.lane(),
        agents,
        delivery: deliveries.git,
        comment_delivery: deliveries.comment,
    });
    Ok(match placement.session_boundary() {
        SessionBoundary::Run => NativeNodeRunner::new(admitted, routes.clone(), routes)?,
        SessionBoundary::Owner => {
            NativeNodeRunner::new_owner_scoped(admitted, routes.clone(), routes)?
        }
    })
}

struct CandidateDeliveries {
    git: Option<Arc<NativeV2DeliveryAdapter>>,
    comment: Option<Arc<NativeV2CommentDeliveryAdapter>>,
}

struct CandidateAgents {
    driver: Arc<dyn NodeDriver>,
    sessions: Arc<dyn SessionFactory>,
    memory: Option<Arc<crate::active_memory::hooks::ActiveMemoryHooks>>,
}

impl CandidateAgents {
    fn new<T>(agent: Arc<T>) -> Self
    where
        T: NodeDriver + SessionFactory + 'static,
    {
        Self {
            memory: None,
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
    delivery: Option<Arc<NativeV2DeliveryAdapter>>,
    comment_delivery: Option<Arc<NativeV2CommentDeliveryAdapter>>,
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
            NodeRuntimeBinding::GitDelivery { .. } | NodeRuntimeBinding::CommentDelivery { .. } => {
                Err(NodeRunnerError::Driver)
            }
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
            NodeRuntimeBinding::CommentDelivery { .. } => {
                self.comment_delivery
                    .as_ref()
                    .ok_or(NodeRunnerError::Driver)?
                    .open(invocation, environment)
                    .await
            }
            NodeRuntimeBinding::GitDelivery { .. } => {
                self.delivery
                    .as_ref()
                    .ok_or(NodeRunnerError::Driver)?
                    .open(invocation, environment)
                    .await
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
                let result = agents.driver.run(invocation, control).await;
                if result.is_err()
                    || matches!(
                        result,
                        Ok(openengine_cluster_protocol::WorkerOutcome::Error { .. })
                    )
                {
                    if let Some(memory) = &agents.memory {
                        memory.execution_failed();
                    }
                }
                result
            }
            NodeRuntimeBinding::CommentDelivery { .. } => {
                self.comment_delivery
                    .as_ref()
                    .ok_or(NodeRunnerError::Driver)?
                    .run(invocation, control)
                    .await
            }
            NodeRuntimeBinding::GitDelivery { .. } => {
                self.delivery
                    .as_ref()
                    .ok_or(NodeRunnerError::Driver)?
                    .run(invocation, control)
                    .await
            }
        }
    }
}
