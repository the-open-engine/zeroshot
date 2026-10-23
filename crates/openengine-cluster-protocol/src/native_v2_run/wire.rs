use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ConnectionKey, EnvironmentVariableName, GraphSpec, IdempotencyKey, NodeName, RunId,
    RunConnectionValues, RunStatusResult, TargetConnectionResolver,
};

use super::{
    ClaudeProvider, CodexProvider, CopilotProvider, NodeRuntimeBinding, PiProvider, RunSize,
    ResolvedSource, RuntimeLane, ActiveMemoryConfig, GlobalAgentsConfig,
};

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, tag = "harness", rename_all = "snake_case")]
pub enum RuntimePlan {
    Copilot {
        provider: CopilotProvider,
        size: RunSize,
        #[serde(
            default,
            rename = "activeMemory",
            skip_serializing_if = "ActiveMemoryConfig::is_default"
        )]
        active_memory: ActiveMemoryConfig,
        #[serde(
            default,
            rename = "globalAgents",
            skip_serializing_if = "GlobalAgentsConfig::is_default"
        )]
        global_agents: GlobalAgentsConfig,
        nodes: BTreeMap<NodeName, NodeRuntimeBinding>,
    },
    Codex {
        provider: CodexProvider,
        size: RunSize,
        #[serde(
            default,
            rename = "activeMemory",
            skip_serializing_if = "ActiveMemoryConfig::is_default"
        )]
        active_memory: ActiveMemoryConfig,
        #[serde(
            default,
            rename = "globalAgents",
            skip_serializing_if = "GlobalAgentsConfig::is_default"
        )]
        global_agents: GlobalAgentsConfig,
        nodes: BTreeMap<NodeName, NodeRuntimeBinding>,
    },
    Claude {
        provider: ClaudeProvider,
        size: RunSize,
        #[serde(
            default,
            rename = "activeMemory",
            skip_serializing_if = "ActiveMemoryConfig::is_default"
        )]
        active_memory: ActiveMemoryConfig,
        #[serde(
            default,
            rename = "globalAgents",
            skip_serializing_if = "GlobalAgentsConfig::is_default"
        )]
        global_agents: GlobalAgentsConfig,
        nodes: BTreeMap<NodeName, NodeRuntimeBinding>,
    },
    Pi {
        provider: PiProvider,
        size: RunSize,
        #[serde(
            default,
            rename = "activeMemory",
            skip_serializing_if = "ActiveMemoryConfig::is_default"
        )]
        active_memory: ActiveMemoryConfig,
        #[serde(
            default,
            rename = "globalAgents",
            skip_serializing_if = "GlobalAgentsConfig::is_default"
        )]
        global_agents: GlobalAgentsConfig,
        nodes: BTreeMap<NodeName, NodeRuntimeBinding>,
    },
}

impl RuntimePlan {
    #[must_use]
    pub const fn active_memory(&self) -> &ActiveMemoryConfig {
        match self {
            Self::Copilot { active_memory, .. }
            | Self::Codex { active_memory, .. }
            | Self::Claude { active_memory, .. }
            | Self::Pi { active_memory, .. } => active_memory,
        }
    }

    pub fn active_memory_mut(&mut self) -> &mut ActiveMemoryConfig {
        match self {
            Self::Copilot { active_memory, .. }
            | Self::Codex { active_memory, .. }
            | Self::Claude { active_memory, .. }
            | Self::Pi { active_memory, .. } => active_memory,
        }
    }

    #[must_use]
    pub const fn global_agents(&self) -> &GlobalAgentsConfig {
        match self {
            Self::Copilot { global_agents, .. }
            | Self::Codex { global_agents, .. }
            | Self::Claude { global_agents, .. }
            | Self::Pi { global_agents, .. } => global_agents,
        }
    }

    pub fn global_agents_mut(&mut self) -> &mut GlobalAgentsConfig {
        match self {
            Self::Copilot { global_agents, .. }
            | Self::Codex { global_agents, .. }
            | Self::Claude { global_agents, .. }
            | Self::Pi { global_agents, .. } => global_agents,
        }
    }

    #[must_use]
    pub const fn size(&self) -> RunSize {
        match self {
            Self::Copilot { size, .. }
            | Self::Codex { size, .. }
            | Self::Claude { size, .. }
            | Self::Pi { size, .. } => *size,
        }
    }

    #[must_use]
    pub const fn nodes(&self) -> &BTreeMap<NodeName, NodeRuntimeBinding> {
        match self {
            Self::Copilot { nodes, .. }
            | Self::Codex { nodes, .. }
            | Self::Claude { nodes, .. }
            | Self::Pi { nodes, .. } => nodes,
        }
    }

    #[must_use]
    pub fn nodes_mut(&mut self) -> &mut BTreeMap<NodeName, NodeRuntimeBinding> {
        match self {
            Self::Copilot { nodes, .. }
            | Self::Codex { nodes, .. }
            | Self::Claude { nodes, .. }
            | Self::Pi { nodes, .. } => nodes,
        }
    }

    #[must_use]
    pub const fn lane(&self) -> RuntimeLane {
        match self {
            Self::Copilot { provider, .. } => RuntimeLane::Copilot {
                provider: *provider,
            },
            Self::Codex { provider, .. } => RuntimeLane::Codex {
                provider: *provider,
            },
            Self::Claude { provider, .. } => RuntimeLane::Claude {
                provider: *provider,
            },
            Self::Pi { provider, .. } => RuntimeLane::Pi {
                provider: *provider,
            },
        }
    }

    /// The binding's own lane, else the run-level lane; Git delivery bindings have no lane.
    #[must_use]
    pub const fn effective_lane(&self, binding: &NodeRuntimeBinding) -> Option<RuntimeLane> {
        match binding {
            NodeRuntimeBinding::Agent {
                lane: Some(lane), ..
            } => Some(*lane),
            NodeRuntimeBinding::Agent { lane: None, .. } => Some(self.lane()),
            NodeRuntimeBinding::GitDelivery { .. } => None,
        }
    }

    /// Distinct effective lanes of the agent bindings, in harness and then provider order.
    #[must_use]
    pub fn lanes(&self) -> BTreeSet<RuntimeLane> {
        self.nodes()
            .values()
            .filter_map(|binding| self.effective_lane(binding))
            .collect()
    }

    /// Whether any agent binding carries its own `lane`, even one equal to the run-level lane.
    #[must_use]
    pub fn has_lane_overrides(&self) -> bool {
        self.nodes()
            .values()
            .any(|binding| matches!(binding, NodeRuntimeBinding::Agent { lane: Some(_), .. }))
    }

    /// Union of the fields required from each connection key across all executable nodes.
    #[must_use]
    pub fn connection_requirements(
        &self,
    ) -> BTreeMap<ConnectionKey, BTreeSet<EnvironmentVariableName>> {
        let mut requirements = BTreeMap::<ConnectionKey, BTreeSet<EnvironmentVariableName>>::new();
        for binding in self.nodes().values() {
            for (key, fields) in binding.declared_connections().iter() {
                requirements
                    .entry(key.clone())
                    .or_default()
                    .extend(fields.iter().cloned());
            }
        }
        requirements
    }
}

/// Immutable, secret-free native-v2 submission admitted by the selected target.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunSubmission {
    pub title: super::RunTitle,
    pub graph: GraphSpec,
    pub initial_input: Value,
    pub runtime: RuntimePlan,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<super::RuntimeEnvironment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ResolvedSource>,
    pub submission_key: IdempotencyKey,
}

impl RunSubmission {
    /// Whether the immutable source needs the generic Git transport discovery marker.
    #[must_use]
    pub fn requires_git_transport(&self) -> bool {
        self.source
            .as_ref()
            .is_some_and(|source| source.requires_git_transport())
    }

    /// Exact fields required by nodes, preparation, and the admitted source transport.
    #[must_use]
    pub fn connection_requirements(
        &self,
    ) -> BTreeMap<ConnectionKey, BTreeSet<EnvironmentVariableName>> {
        run_connection_requirements_with_source(
            &self.runtime,
            self.environment.as_ref(),
            self.source.as_ref(),
        )
    }
}

/// Combine node and preparation requirements without resolving any connection values.
#[must_use]
pub fn run_connection_requirements(
    runtime: &RuntimePlan,
    environment: Option<&super::RuntimeEnvironment>,
) -> BTreeMap<ConnectionKey, BTreeSet<EnvironmentVariableName>> {
    let mut requirements = runtime.connection_requirements();
    if let Some(environment) = environment {
        for (key, fields) in environment.connections.iter() {
            requirements
                .entry(key.clone())
                .or_default()
                .extend(fields.iter().cloned());
        }
    }
    requirements
}

/// Combine all immutable requirements without resolving or forwarding credentials.
#[must_use]
pub fn run_connection_requirements_with_source(
    runtime: &RuntimePlan,
    environment: Option<&super::RuntimeEnvironment>,
    source: Option<&ResolvedSource>,
) -> BTreeMap<ConnectionKey, BTreeSet<EnvironmentVariableName>> {
    let mut requirements = run_connection_requirements(runtime, environment);
    if let Some(remote) = source.and_then(|source| source.remote.as_ref()) {
        for (key, fields) in remote.connection_requirements() {
            requirements.entry(key).or_default().extend(fields);
        }
    }
    requirements
}

/// Trusted controller bootstrap admission. The host assigns the only public run identity before
/// controller start; the immutable submission remains identity-neutral.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunSubmitParams {
    pub run_id: RunId,
    pub submission: RunSubmission,
}

/// A successful submission returns the one public identity used by every later run method.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunSubmitResult {
    pub run_id: RunId,
}

#[derive(Clone, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunResumeParams {
    pub run_id: RunId,
    pub successor_run_id: RunId,
    /// Omission preserves restarting from the latest retained workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<super::RunResumeFrom>,
    /// Fresh static connection values for the successor attempt.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub connections: RunConnectionValues,
    /// Run-scoped callback used to resolve fresh dynamic connection values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_resolver: Option<TargetConnectionResolver>,
    /// Fresh GitHub credential for private source checkout or Git delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_token: Option<String>,
    /// Fresh generic Git authority, excluded from immutable source and submission identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_credentials: Option<super::GitTransportCredentials>,
}

impl std::fmt::Debug for RunResumeParams {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunResumeParams")
            .field("run_id", &self.run_id)
            .field("successor_run_id", &self.successor_run_id)
            .field("from", &self.from)
            .field("connections", &self.connections.keys().collect::<Vec<_>>())
            .field("connection_resolver", &self.connection_resolver)
            .field(
                "github_token",
                &self.github_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("git_credentials", &self.git_credentials)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunResumeResult {
    pub run_id: RunId,
    pub resumed_from: RunId,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunDiscardWorkspaceParams {
    pub run_id: RunId,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunDiscardWorkspaceResult {
    pub run_id: RunId,
    pub discarded: bool,
}

/// The MVP inventory has no filters or pagination controls.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunListParams {}

/// Current durable projections for every retained run.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunListResult {
    pub runs: Vec<RunStatusResult>,
}
