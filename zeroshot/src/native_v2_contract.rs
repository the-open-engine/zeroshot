//! Minimal, secret-free composition contracts for the native-v2 engine.
//!
//! `GraphSpec` remains the graph language. This module only binds executable graph leaves to one
//! graph-wide harness/provider lane and defines the neutral values exchanged by admission, the
//! reducer, the runner, and the run ledger. It deliberately contains no admission or execution
//! policy.

use std::fmt;
use std::marker::PhantomData;
use std::num::NonZeroU64;

use openengine_cluster_protocol::{
    CompiledGraphIr, GraphSpec, IdempotencyKey, NodeInstructions, NodeName, RunId, TokenCount,
    WorkerOutcome, WorkerRef,
};
pub use openengine_cluster_protocol::{
    ClaudeProvider, CodexProvider, CopilotProvider, ConnectionKey, DeclaredConnections,
    DeclaredEnvironment, EnvironmentVariableName, ModelId, NodeRuntimeBinding, PullRequestFeedback,
    ReasoningEffort, ResolvedSource, RunSize, RunSubmission, RunTitle, RuntimePlan, SessionScope,
    SourceBranchId, SourceRepositoryId, SourceRevisionId, MAX_DECLARED_CONNECTIONS,
    MAX_DECLARED_ENVIRONMENT_NAMES,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Graph-visible push-only delivery worker.
pub const GIT_DELIVERY_PUSH_WORKER_REF: &str = "builtin.git-delivery.push@1";
/// Legacy open-only PR delivery worker.
pub const GIT_DELIVERY_PR_WORKER_REF: &str = "builtin.git-delivery.pr@1";
/// PR delivery that waits for technical readiness and considers feedback.
pub const GIT_DELIVERY_PR_V2_WORKER_REF: &str = "builtin.git-delivery.pr@2";
/// Graph-visible merge delivery worker backed by the shared Git delivery implementation.
pub const GIT_DELIVERY_MERGE_WORKER_REF: &str = "builtin.git-delivery.merge@1";
/// Merge delivery with an authoritative merge revision in its v2 receipt.
pub const GIT_DELIVERY_MERGE_V2_WORKER_REF: &str = "builtin.git-delivery.merge@2";
/// Feedback-aware merge delivery with an authoritative v3 receipt.
pub const GIT_DELIVERY_MERGE_V3_WORKER_REF: &str = "builtin.git-delivery.merge@3";
/// Conventional connection key used by built-in GitHub checkout and delivery behavior.
pub const GITHUB_CONNECTION_KEY: &str = "github";

/// Identity-neutral request used before a host resolves mutable source selection.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunSubmissionIntent {
    pub title: RunTitle,
    pub graph: GraphSpec,
    pub initial_input: Value,
    pub runtime: RuntimePlan,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<SourceBranchId>,
    pub submission_key: IdempotencyKey,
}

impl From<&RunSubmission> for RunSubmissionIntent {
    fn from(submission: &RunSubmission) -> Self {
        Self {
            title: submission.title.clone(),
            graph: submission.graph.clone(),
            initial_input: submission.initial_input.clone(),
            runtime: submission.runtime.clone(),
            branch: None,
            submission_key: submission.submission_key.clone(),
        }
    }
}

/// Admission's secret-free output. The compiler promotes the unchanged `GraphSpec` to the existing
/// verified `CompiledGraphIr`; later stages never execute raw graph syntax.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AdmittedRun {
    pub title: RunTitle,
    pub graph: CompiledGraphIr,
    pub initial_input: Value,
    pub runtime: RuntimePlan,
    pub source: ResolvedSource,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("identity must be greater than zero")]
pub struct IdentityError;

/// Positive numeric identity whose marker keeps distinct identity domains type-safe.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PositiveIdentity<Tag> {
    value: NonZeroU64,
    marker: PhantomData<fn() -> Tag>,
}

impl<Tag> PositiveIdentity<Tag> {
    pub fn new(value: u64) -> Result<Self, IdentityError> {
        let value = NonZeroU64::new(value).ok_or(IdentityError)?;
        Ok(Self {
            value,
            marker: PhantomData,
        })
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.value.get()
    }
}

impl<Tag> TryFrom<u64> for PositiveIdentity<Tag> {
    type Error = IdentityError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<Tag> fmt::Display for PositiveIdentity<Tag> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(formatter)
    }
}

impl<Tag> Serialize for PositiveIdentity<Tag> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u64(self.value.get())
    }
}

impl<'de, Tag> Deserialize<'de> for PositiveIdentity<Tag> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NodeInstanceIdentity {}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExecutionIdentity {}

pub type NodeInstanceId = PositiveIdentity<NodeInstanceIdentity>;
pub type ExecutionId = PositiveIdentity<ExecutionIdentity>;

/// Stable address for one dispatch. A node instance survives loop revisits; an execution does not.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExecutionRef {
    pub run_id: RunId,
    pub node: NodeName,
    pub node_instance: NodeInstanceId,
    pub execution: ExecutionId,
}

/// Provider-neutral usage reported for exactly one launched agent invocation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TokenUsageDelta {
    pub input_tokens: TokenCount,
    pub output_tokens: TokenCount,
    pub cache_read_input_tokens: Option<TokenCount>,
    pub cache_creation_input_tokens: Option<TokenCount>,
}

pub(crate) fn parse_token_usage_delta(
    value: Option<&Value>,
    cache_read_key: Option<&str>,
    cache_creation_key: Option<&str>,
) -> Option<TokenUsageDelta> {
    let usage = value?.as_object()?;
    Some(TokenUsageDelta {
        input_tokens: token_count(usage.get("input_tokens")?)?,
        output_tokens: token_count(usage.get("output_tokens")?)?,
        cache_read_input_tokens: optional_token_count(usage, cache_read_key)?,
        cache_creation_input_tokens: optional_token_count(usage, cache_creation_key)?,
    })
}

fn optional_token_count(
    usage: &serde_json::Map<String, Value>,
    key: Option<&str>,
) -> Option<Option<TokenCount>> {
    let Some(key) = key else {
        return Some(None);
    };
    match usage.get(key) {
        Some(value) => token_count(value).map(Some),
        None => Some(None),
    }
}

fn token_count(value: &Value) -> Option<TokenCount> {
    TokenCount::new(value.as_u64()?).ok()
}

/// Secret-free runner request produced by the reducer/supervisor boundary.
///
/// Workspace access and resolved environment values are runtime capabilities and do not belong in
/// this durable value.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NodeInvocation {
    pub reference: ExecutionRef,
    pub worker: WorkerRef,
    pub instructions: Option<NodeInstructions>,
    pub input: Value,
    pub binding: NodeRuntimeBinding,
}

/// Normalized completion returned to the supervisor and safe to append to the run ledger.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NodeCompletion {
    pub reference: ExecutionRef,
    pub outcome: WorkerOutcome,
}

#[cfg(test)]
#[path = "native_v2_contract/tests.rs"]
mod tests;
