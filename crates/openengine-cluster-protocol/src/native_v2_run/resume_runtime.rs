//! Runtime-file forms accepted when replacing a successor's runtime.
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use super::{
    ActiveMemoryConfig, DeclaredConnections, GlobalAgentsConfig, ModelId, ReasoningEffort, RunSize,
    RuntimePlan, SessionScope, MAX_MODEL_CONTEXT_WINDOW, MAX_MODEL_MAX_TOKENS,
    MIN_MODEL_CONTEXT_WINDOW, MIN_MODEL_MAX_TOKENS, deserialize_optional_model_limit,
};

/// A complete replacement, expanded against the predecessor's unchanged graph by its host.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunResumeRuntime {
    Exact { runtime: RuntimePlan },
    Uniform { runtime: UniformRuntimePlan },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UniformRuntimePlan {
    pub harness: crate::HarnessId,
    pub provider: crate::ProviderId,
    #[serde(default)]
    pub size: Option<RunSize>,
    #[serde(
        default,
        deserialize_with = "crate::RuntimePlan::deserialize_max_concurrency"
    )]
    #[schemars(range(max = 65_536_u64))]
    pub max_concurrency: Option<crate::PositiveInteger>,
    #[serde(default = "default_workspace_checkpoints")]
    pub workspace_checkpoints: bool,
    pub model: ModelId,
    #[serde(default)]
    pub effort: Option<ReasoningEffort>,
    #[serde(default, deserialize_with = "deserialize_context_window")]
    #[schemars(range(min = 1, max = 10_000_000))]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub harness_config: Option<crate::HarnessConfiguration>,
    #[serde(default, deserialize_with = "deserialize_max_tokens")]
    #[schemars(range(min = 1, max = 10_000_000))]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub session_scope: Option<SessionScope>,
    #[serde(default)]
    pub connections: Option<DeclaredConnections>,
    #[serde(default)]
    pub active_memory: ActiveMemoryConfig,
    #[serde(default)]
    pub global_agents: GlobalAgentsConfig,
}

const fn default_workspace_checkpoints() -> bool {
    true
}

fn deserialize_context_window<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_optional_model_limit(
        deserializer,
        "contextWindow",
        MIN_MODEL_CONTEXT_WINDOW,
        MAX_MODEL_CONTEXT_WINDOW,
    )
}

fn deserialize_max_tokens<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_optional_model_limit(
        deserializer,
        "maxTokens",
        MIN_MODEL_MAX_TOKENS,
        MAX_MODEL_MAX_TOKENS,
    )
}

impl UniformRuntimePlan {
    #[must_use]
    pub fn into_runtime_plan(
        self,
        nodes: std::collections::BTreeMap<crate::NodeName, crate::NodeRuntimeBinding>,
    ) -> RuntimePlan {
        RuntimePlan {
            harness: self.harness,
            provider: self.provider,
            size: self.size,
            max_concurrency: self.max_concurrency,
            workspace_checkpoints: self.workspace_checkpoints,
            active_memory: self.active_memory,
            global_agents: self.global_agents,
            nodes,
        }
    }
}

/// Resolve replacement declarations against the retained graph and source without starting work.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunResumeRequirementsParams {
    pub run_id: crate::RunId,
    pub runtime: RunResumeRuntime,
}

/// Effective connection declarations, including unchanged source/environment and delivery defaults.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunResumeRequirementsResult {
    pub connections: crate::RunConnectionRequirements,
}
