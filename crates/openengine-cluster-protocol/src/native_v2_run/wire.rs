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
#[serde(deny_unknown_fields, tag = "harness", rename_all = "snake_case")]
pub enum RuntimePlan {
    Copilot {
        provider: CopilotProvider,
        size: RunSize,
        nodes: BTreeMap<NodeName, NodeRuntimeBinding>,
    },
    Codex {
        provider: CodexProvider,
        size: RunSize,
        nodes: BTreeMap<NodeName, NodeRuntimeBinding>,
    },
    Claude {
        provider: ClaudeProvider,
        size: RunSize,
        nodes: BTreeMap<NodeName, NodeRuntimeBinding>,
    },
}

impl RuntimePlan {
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub connections: RunConnectionValues,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_resolver: Option<TargetConnectionResolver>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_token: Option<String>,
}

impl std::fmt::Debug for RunResumeParams {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunResumeParams")
            .field("run_id", &self.run_id)
            .field("successor_run_id", &self.successor_run_id)
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
