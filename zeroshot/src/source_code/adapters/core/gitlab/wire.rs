use serde::Deserialize;
use openengine_cluster_protocol::SourceForgeBaseUrl;
use crate::source_code_provider::{
    SourceDeliveryCredential, SourceDeliveryError, SourceReviewReceipt, SourceReviewRequest,
    valid_git_branch, valid_git_revision,
};

#[derive(Deserialize)]
pub(super) struct ServerMetadata {
    pub version: String,
    pub enterprise: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Project {
    pub id: u64,
    pub path_with_namespace: String,
    pub web_url: String,
    pub merge_method: String,
    #[serde(default)]
    pub only_allow_merge_if_pipeline_succeeds: Option<bool>,
    #[serde(default)]
    pub allow_merge_on_skipped_pipeline: Option<bool>,
    #[serde(default)]
    pub only_allow_merge_if_all_discussions_are_resolved: Option<bool>,
    #[serde(default)]
    pub merge_trains_enabled: Option<bool>,
}

impl Project {
    pub(super) fn validate(
        &self,
        repository: &str,
        base: &SourceForgeBaseUrl,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        let matching = self.path_with_namespace == repository || self.id.to_string() == repository;
        let expected = format!(
            "{}/{}",
            base.as_str().trim_end_matches('/'),
            self.path_with_namespace
        );
        let path_valid = valid_repository_path(&self.path_with_namespace);
        if self.id == 0
            || !matching
            || !path_valid
            || self.web_url != expected
            || !matches!(self.merge_method.as_str(), "merge" | "rebase_merge" | "ff")
        {
            return Err(super::api::identity_error(
                format!(
                    "GitLab project response changed admitted identity: expected project {repository}, observed project {} (ID {})",
                    if path_valid {
                        &self.path_with_namespace
                    } else {
                        "<invalid project path>"
                    },
                    self.id,
                ),
                credential,
            ));
        }
        Ok(())
    }
}

pub(super) fn valid_repository_path(value: &str) -> bool {
    value.len() <= 2048 && value.split('/').count() >= 2 && value.split('/').all(valid_path_segment)
}

fn valid_path_segment(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Commit {
    pub id: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Branch {
    pub name: String,
    pub commit: Commit,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Pipeline {
    pub id: u64,
    #[serde(default)]
    pub project_id: Option<u64>,
    pub sha: String,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct MergeRequest {
    pub id: u64,
    pub iid: u64,
    pub source_project_id: u64,
    pub target_project_id: u64,
    pub source_branch: String,
    pub target_branch: String,
    #[serde(default, deserialize_with = "nullable_revision")]
    pub sha: String,
    pub state: String,
    pub detailed_merge_status: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub has_conflicts: bool,
    #[serde(default)]
    pub blocking_discussions_resolved: Option<bool>,
    #[serde(default)]
    pub head_pipeline: Option<Pipeline>,
    #[serde(default)]
    pub merge_commit_sha: Option<String>,
    #[serde(default)]
    pub squash_commit_sha: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub merge_when_pipeline_succeeds: bool,
}

impl MergeRequest {
    pub(super) fn receipt(
        &self,
        request: &SourceReviewRequest,
        project: u64,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewReceipt, SourceDeliveryError> {
        let receipt = SourceReviewReceipt::for_request(request, self.iid.to_string());
        self.require_review(&receipt, project, credential)?;
        Ok(receipt)
    }

    pub(super) fn require_review(
        &self,
        receipt: &SourceReviewReceipt,
        project: u64,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        if self.sha.is_empty() {
            return Err(super::api::delivery_context(
                SourceDeliveryError::review_head_not_visible(),
                super::review::merge_request_identity(receipt),
                credential,
            ));
        }
        if !self.matches_review(receipt, project) {
            return Err(super::api::identity_error(
                format!(
                    "GitLab {} changed the bound review/head: observed merge request !{} in source project {} and target project {}, source branch {}, target branch {}, head revision {}. Restore the bound review identity before resuming.",
                    super::review::merge_request_identity(receipt),
                    self.iid,
                    self.source_project_id,
                    self.target_project_id,
                    branch_identity(&self.source_branch),
                    branch_identity(&self.target_branch),
                    revision_identity(&self.sha),
                ),
                credential,
            ));
        }
        Ok(())
    }

    fn matches_review(&self, receipt: &SourceReviewReceipt, project: u64) -> bool {
        self.id > 0
            && self.iid > 0
            && self.iid.to_string() == receipt.review_id
            && self.source_project_id == project
            && self.target_project_id == project
            && self.source_branch == receipt.head_branch
            && self.target_branch == receipt.target_branch
            && self.sha == receipt.head_revision
            && valid_git_revision(&self.sha)
    }
}

pub(super) fn branch_identity(value: &str) -> &str {
    if valid_git_branch(value) {
        value
    } else {
        "<invalid branch>"
    }
}

pub(super) fn revision_identity(value: &str) -> &str {
    if valid_git_revision(value) {
        value
    } else {
        "<invalid revision>"
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Actor {
    pub id: u64,
}

#[derive(Deserialize)]
pub(super) struct Member {
    pub id: u64,
    pub state: AccountState,
    #[serde(default)]
    pub membership_state: MembershipState,
    pub access_level: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AccountState {
    Active,
    Blocked,
    LdapBlocked,
    BlockedPendingApproval,
    Deactivated,
    Banned,
}

// EE serializes membership separately from account state; CE omits this field.
// Only omission is edition-dependent: explicit null and unknown values remain errors.
#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum MembershipState {
    Active,
    Awaiting,
    #[default]
    #[serde(skip)]
    Unreported,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Note {
    pub id: u64,
    pub author: Actor,
    pub body: String,
    pub noteable_type: String,
    pub noteable_id: u64,
    pub project_id: u64,
    #[serde(default)]
    pub noteable_iid: Option<u64>,
    #[serde(default)]
    pub system: bool,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub position: Option<serde_json::Value>,
    #[serde(default)]
    pub resolved: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Discussion {
    pub id: String,
    pub notes: Vec<Note>,
}

pub(super) fn valid_number(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
        && value
            .parse::<u64>()
            .is_ok_and(|id| id > 0 && id.to_string() == value)
}

fn nullable_revision<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}
