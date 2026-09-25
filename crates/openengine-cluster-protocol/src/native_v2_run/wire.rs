use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ConnectionKey, EnvironmentVariableName, GraphSpec, IdempotencyKey, NodeName, RunId,
    RunConnectionValues, RunStatusResult, TargetConnectionResolver,
};

use super::{
    ClaudeProvider, CodexProvider, CopilotProvider, NodeRuntimeBinding, RunSize, ResolvedSource,
};

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(
    deny_unknown_fields,
    tag = "harness",
    rename_all = "snake_case",
    bound(deserialize = "E: Deserialize<'de>")
)]
pub enum RuntimePlan<E = super::RuntimeEnvironment> {
    Copilot {
        provider: CopilotProvider,
        size: RunSize,
        nodes: BTreeMap<NodeName, NodeRuntimeBinding>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        environment: Option<E>,
    },
    Codex {
        provider: CodexProvider,
        size: RunSize,
        nodes: BTreeMap<NodeName, NodeRuntimeBinding>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        environment: Option<E>,
    },
    Claude {
        provider: ClaudeProvider,
        size: RunSize,
        nodes: BTreeMap<NodeName, NodeRuntimeBinding>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        environment: Option<E>,
    },
}

impl<E> RuntimePlan<E> {
    #[must_use]
    pub const fn size(&self) -> RunSize {
        match self {
            Self::Copilot { size, .. } | Self::Codex { size, .. } | Self::Claude { size, .. } => {
                *size
            }
        }
    }

    #[must_use]
    pub const fn nodes(&self) -> &BTreeMap<NodeName, NodeRuntimeBinding> {
        match self {
            Self::Copilot { nodes, .. }
            | Self::Codex { nodes, .. }
            | Self::Claude { nodes, .. } => nodes,
        }
    }

    #[must_use]
    pub const fn environment(&self) -> Option<&E> {
        match self {
            Self::Copilot { environment, .. }
            | Self::Codex { environment, .. }
            | Self::Claude { environment, .. } => environment.as_ref(),
        }
    }

    /// Replace authoring references with the exact definition selected for this run.
    #[must_use]
    pub fn map_environment<T>(self, map: impl FnOnce(Option<E>) -> Option<T>) -> RuntimePlan<T> {
        match self {
            Self::Copilot {
                provider,
                size,
                nodes,
                environment,
            } => RuntimePlan::Copilot {
                provider,
                size,
                nodes,
                environment: map(environment),
            },
            Self::Codex {
                provider,
                size,
                nodes,
                environment,
            } => RuntimePlan::Codex {
                provider,
                size,
                nodes,
                environment: map(environment),
            },
            Self::Claude {
                provider,
                size,
                nodes,
                environment,
            } => RuntimePlan::Claude {
                provider,
                size,
                nodes,
                environment: map(environment),
            },
        }
    }
}

/// Author-owned profile runtime. Environments are reusable resources, never inline definitions.
pub type ProfileRuntimePlan = RuntimePlan<super::EnvironmentReference>;

impl RuntimePlan {
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
        if let Some(environment) = self.environment() {
            for (key, fields) in environment.connections.iter() {
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
    pub source: ResolvedSource,
    pub submission_key: IdempotencyKey,
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
