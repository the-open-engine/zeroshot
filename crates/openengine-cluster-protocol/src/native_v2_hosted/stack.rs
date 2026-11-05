//! Hosted stack admission and aggregate lifecycle contracts.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    IdempotencyKey, RunConnectionValues, RunId, RunProfileName, RunProfileSelector, RunTitle,
    SourceBranchId, SourceRepositoryId, SourceRevisionId,
};

pub const STACKS_KIND: &str = "zeroshot.stacks/v1";
pub const STACK_SCHEMA: &str = "zeroshot.stack/v1";
pub const MAX_STACK_RUNS: usize = 64;

/// Stack IDs share the host-assigned opaque identity representation used by runs.
pub type StackId = RunId;
/// Symbolic stack run names use the existing compact ASCII identifier contract.
pub type StackRunName = RunProfileName;

/// How every run in a stack delivers its change.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StackDelivery {
    /// Each run merges into the base branch before its dependents start.
    Merge,
    /// The runs form one linear stack; each run opens a pull request against the branch of the
    /// run it needs, so the stack arrives as a chain of pull requests.
    StackedPullRequests,
}

impl StackDelivery {
    /// Manifest and wire spelling of this delivery mode.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::StackedPullRequests => "stacked_pull_requests",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StackSource {
    pub repository: SourceRepositoryId,
    pub branch: SourceBranchId,
}

/// One unresolved stack run whose source revision is assigned only after dependencies pass.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StackRunRequest {
    pub name: StackRunName,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<StackRunName>,
    pub initial_input: serde_json::Value,
}

/// Atomic stack submission. Secret values deliberately do not implement Debug.
#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StackSubmitRequest {
    pub submission_key: IdempotencyKey,
    pub title: RunTitle,
    pub expires_at: String,
    pub delivery: StackDelivery,
    pub source: StackSource,
    pub profile: RunProfileSelector,
    /// Stacked pull-request submissions are already linear: each run needs only its predecessor.
    pub runs: Vec<StackRunRequest>,
    /// Shared preparation for every stack run; omission permits a host default, `{}` selects base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<crate::RuntimeEnvironment>,
    #[serde(default, skip_serializing_if = "RunConnectionValues::is_empty")]
    pub connections: RunConnectionValues,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_token: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StackState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Expired,
}

impl StackState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Expired
        )
    }

    #[must_use]
    pub const fn succeeded(self) -> bool {
        matches!(self, Self::Succeeded)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StackRunState {
    Blocked,
    Materializing,
    Queued,
    Provisioning,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    Expired,
}

impl StackRunState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Expired
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StackRunStatus {
    pub name: StackRunName,
    pub run_id: RunId,
    pub state: StackRunState,
    pub needs: Vec<StackRunName>,
    /// Branch the run started from once materialized; a stacked pull request targets it.
    #[serde(with = "explicit_nullable")]
    #[schemars(with = "Option<SourceBranchId>")]
    pub source_branch: Option<SourceBranchId>,
    #[serde(with = "explicit_nullable")]
    #[schemars(with = "Option<SourceRevisionId>")]
    pub source_revision: Option<SourceRevisionId>,
    #[serde(with = "explicit_nullable")]
    #[schemars(with = "Option<String>")]
    pub ready_at: Option<String>,
    #[serde(with = "explicit_nullable")]
    #[schemars(with = "Option<String>")]
    pub queue_expires_at: Option<String>,
    #[serde(with = "explicit_nullable")]
    #[schemars(with = "Option<String>")]
    pub terminal_at: Option<String>,
    #[serde(with = "explicit_nullable")]
    #[schemars(with = "Option<String>")]
    pub waiting_reason: Option<String>,
    #[serde(with = "explicit_nullable")]
    #[schemars(with = "Option<String>")]
    pub error_code: Option<String>,
}

mod explicit_nullable {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<T, S>(value: &Option<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        T: Serialize,
        S: Serializer,
    {
        value.serialize(serializer)
    }

    pub(super) fn deserialize<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        T: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Option::deserialize(deserializer)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Stack {
    pub stack_id: StackId,
    pub title: RunTitle,
    pub state: StackState,
    pub repository: SourceRepositoryId,
    pub branch: SourceBranchId,
    pub submitted_at: String,
    pub expires_at: String,
    pub runs: Vec<StackRunStatus>,
}

/// Route templates relative to `baseUrl`; `status` and `force` contain one `{stack_id}` segment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetStackRoutes {
    pub create: String,
    pub status: String,
    pub force: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TargetStacksDiscovery {
    pub kind: String,
    pub base_url: String,
    pub route_templates: TargetStackRoutes,
}

#[cfg(test)]
#[path = "stack/tests.rs"]
mod tests;
