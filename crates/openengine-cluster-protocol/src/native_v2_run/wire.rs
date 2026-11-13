use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ConnectionKey, EnvironmentVariableName, GraphSpec, IdempotencyKey, NodeName, RunId,
    RunConnectionValues, RunStatusResult, TargetConnectionResolver,
};

use super::{
    NodeRuntimeBinding, RunSize, ResolvedSource, RuntimeLane, HarnessId, ProviderId,
    ActiveMemoryConfig, GlobalAgentsConfig,
};

/// Portable runtime selection. Compatibility belongs to the host's installed harness registry.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimePlan {
    pub harness: HarnessId,
    pub provider: ProviderId,
    /// Omission asks the selected target to choose its advertised default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<RunSize>,
    /// Maximum owned leaf and auxiliary executions. Omission uses 64 on every placement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(deserialize_with = "RuntimePlan::deserialize_max_concurrency")]
    #[schemars(range(max = 65_536_u64))]
    pub max_concurrency: Option<crate::PositiveInteger>,
    /// Local workspace snapshots. Run history remains durable when disabled.
    #[serde(
        default = "default_workspace_checkpoints",
        skip_serializing_if = "workspace_checkpoints_enabled"
    )]
    pub workspace_checkpoints: bool,
    #[serde(default, skip_serializing_if = "ActiveMemoryConfig::is_default")]
    pub active_memory: ActiveMemoryConfig,
    #[serde(default, skip_serializing_if = "GlobalAgentsConfig::is_default")]
    pub global_agents: GlobalAgentsConfig,
    pub nodes: BTreeMap<NodeName, NodeRuntimeBinding>,
}

impl RuntimePlan {
    pub const DEFAULT_MAX_CONCURRENCY: u64 = 64;
    pub const MAX_CONCURRENCY: u64 = 65_536;

    /// Decode the same bounded concurrency value in full and uniform runtime plans.
    pub fn deserialize_max_concurrency<'de, D>(
        deserializer: D,
    ) -> Result<Option<crate::PositiveInteger>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Option::<crate::PositiveInteger>::deserialize(deserializer)?;
        if value.is_some_and(|limit| limit.get() > Self::MAX_CONCURRENCY) {
            return Err(serde::de::Error::custom("maxConcurrency exceeds 65536"));
        }
        Ok(value)
    }

    #[must_use]
    pub fn effective_max_concurrency(&self) -> u64 {
        self.max_concurrency
            .map_or(Self::DEFAULT_MAX_CONCURRENCY, |value| value.get())
    }

    #[must_use]
    pub fn new(
        lane: RuntimeLane,
        size: impl Into<Option<RunSize>>,
        nodes: BTreeMap<NodeName, NodeRuntimeBinding>,
    ) -> Self {
        Self {
            harness: lane.harness,
            provider: lane.provider,
            size: size.into(),
            max_concurrency: None,
            workspace_checkpoints: true,
            active_memory: ActiveMemoryConfig::default(),
            global_agents: GlobalAgentsConfig::default(),
            nodes,
        }
    }

    #[must_use]
    pub const fn active_memory(&self) -> &ActiveMemoryConfig {
        &self.active_memory
    }

    pub fn active_memory_mut(&mut self) -> &mut ActiveMemoryConfig {
        &mut self.active_memory
    }

    #[must_use]
    pub const fn global_agents(&self) -> &GlobalAgentsConfig {
        &self.global_agents
    }

    pub fn global_agents_mut(&mut self) -> &mut GlobalAgentsConfig {
        &mut self.global_agents
    }

    #[must_use]
    pub const fn size(&self) -> Option<&RunSize> {
        self.size.as_ref()
    }

    #[must_use]
    pub const fn nodes(&self) -> &BTreeMap<NodeName, NodeRuntimeBinding> {
        &self.nodes
    }

    #[must_use]
    pub fn nodes_mut(&mut self) -> &mut BTreeMap<NodeName, NodeRuntimeBinding> {
        &mut self.nodes
    }

    #[must_use]
    pub fn lane(&self) -> RuntimeLane {
        RuntimeLane {
            harness: self.harness.clone(),
            provider: self.provider.clone(),
        }
    }

    /// The binding's own lane, else the run-level lane; trusted delivery has no model lane.
    #[must_use]
    pub fn effective_lane(&self, binding: &NodeRuntimeBinding) -> Option<RuntimeLane> {
        match binding {
            NodeRuntimeBinding::Agent { lane, .. } => {
                Some(lane.clone().unwrap_or_else(|| self.lane()))
            }
            NodeRuntimeBinding::Request {}
            | NodeRuntimeBinding::Script { .. }
            | NodeRuntimeBinding::Decision { .. }
            | NodeRuntimeBinding::GitDelivery { .. }
            | NodeRuntimeBinding::CommentDelivery { .. } => None,
        }
    }

    /// Distinct effective lanes of the agent bindings, ordered by opaque harness/provider names.
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

const fn default_workspace_checkpoints() -> bool {
    true
}

const fn workspace_checkpoints_enabled(enabled: &bool) -> bool {
    *enabled
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
    /// Author and committer of every commit the successor's delivery creates. Omission keeps the
    /// target's default delivery identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_identity: Option<crate::CommitIdentity>,
    /// Person credited by a `Co-authored-by` trailer on every commit carrying the successor's
    /// changes. Author and committer stay as `commitIdentity` selects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_co_author: Option<crate::CommitIdentity>,
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
            .field("commit_identity", &self.commit_identity)
            .field("commit_co_author", &self.commit_co_author)
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
