//! Hosted merge-plan admission and aggregate lifecycle contracts.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    IdempotencyKey, RunConnectionValues, RunId, RunProfileName, RunProfileSelector, RunTitle,
    SourceBranchId, SourceRepositoryId, SourceRevisionId,
};

pub const MERGE_PLANS_KIND: &str = "zeroshot.merge-plans/v1";
pub const MERGE_PLAN_SCHEMA: &str = "zeroshot.merge-plan/v1";
pub const MAX_MERGE_PLAN_RUNS: usize = 64;

/// Merge-plan IDs share the host-assigned opaque identity representation used by runs.
pub type MergePlanId = RunId;
/// Symbolic plan run names use the existing compact ASCII identifier contract.
pub type MergePlanRunName = RunProfileName;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MergePlanSource {
    pub repository: SourceRepositoryId,
    pub branch: SourceBranchId,
}

/// One unresolved merge-plan run whose source revision is assigned only after dependencies pass.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MergePlanRunRequest {
    pub name: MergePlanRunName,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<MergePlanRunName>,
    pub initial_input: serde_json::Value,
}

/// Atomic merge-plan submission. Secret values deliberately do not implement Debug.
#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MergePlanSubmitRequest {
    pub submission_key: IdempotencyKey,
    pub title: RunTitle,
    pub expires_at: String,
    pub source: MergePlanSource,
    pub profile: RunProfileSelector,
    pub runs: Vec<MergePlanRunRequest>,
    #[serde(default, skip_serializing_if = "RunConnectionValues::is_empty")]
    pub connections: RunConnectionValues,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_token: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MergePlanState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Expired,
}

impl MergePlanState {
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
pub enum MergePlanRunState {
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

impl MergePlanRunState {
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
pub struct MergePlanRunStatus {
    pub name: MergePlanRunName,
    pub run_id: RunId,
    pub state: MergePlanRunState,
    pub needs: Vec<MergePlanRunName>,
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
pub struct MergePlan {
    pub plan_id: MergePlanId,
    pub title: RunTitle,
    pub state: MergePlanState,
    pub repository: SourceRepositoryId,
    pub branch: SourceBranchId,
    pub submitted_at: String,
    pub expires_at: String,
    pub runs: Vec<MergePlanRunStatus>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetMergePlanRoutes {
    pub create: String,
    pub status: String,
    pub force: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TargetMergePlansDiscovery {
    pub kind: String,
    pub base_url: String,
    pub route_templates: TargetMergePlanRoutes,
}

#[cfg(test)]
mod tests {
    use openengine_cluster_testkit::assertions::AssertValue;
    use serde_json::json;

    use super::*;

    #[test]
    fn plan_run_names_use_the_compact_ascii_identifier_contract() {
        for valid in ["backend", "frontend_2", "integrate.final"] {
            assert!(MergePlanRunName::new(valid).is_ok());
        }
        for invalid in ["", "-leading", "has space", "has/slash", &"x".repeat(65)] {
            assert!(MergePlanRunName::new(invalid).is_err());
        }
    }

    #[test]
    fn plan_status_requires_explicit_nullable_lifecycle_fields() {
        let missing = serde_json::from_value::<MergePlanRunStatus>(json!({
            "name": "backend",
            "runId": "run-1",
            "state": "blocked",
            "needs": []
        }));
        assert!(missing.is_err());

        let status = serde_json::from_value::<MergePlanRunStatus>(json!({
            "name": "backend",
            "runId": "run-1",
            "state": "blocked",
            "needs": [],
            "sourceRevision": null,
            "readyAt": null,
            "queueExpiresAt": null,
            "terminalAt": null,
            "waitingReason": null,
            "errorCode": null
        }))
        .assert_value();
        assert_eq!(status.name.as_str(), "backend");
    }
}
