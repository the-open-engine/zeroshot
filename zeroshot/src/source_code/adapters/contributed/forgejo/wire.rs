//! The server facts the delivery authority reads: the version, the authenticated user, the
//! repository identity and permissions, and a branch. Only the fields the adapter reads are
//! modelled and unknown fields are ignored. The fields the delivery identity and the outcome are
//! read from are required, so a response that omits one is an error rather than an empty value;
//! every other field stays optional.

use openengine_cluster_protocol::SourceForgeBaseUrl;

use crate::source_code_provider::SourceDeliveryError;

pub(super) const REPOSITORY_ERROR: &str = "Forgejo repository identity must be <owner>/<name>";

#[derive(serde::Deserialize)]
pub(super) struct ServerVersion {
    pub version: String,
}

/// The authenticated user, as the access check reads it.
#[derive(serde::Deserialize)]
pub(super) struct User {
    #[serde(default)]
    pub login: String,
}

/// What the token may do with the repository.
#[derive(serde::Deserialize)]
pub(super) struct RepositoryPermissions {
    #[serde(default)]
    pub pull: bool,
    #[serde(default)]
    pub push: bool,
}

#[derive(serde::Deserialize)]
pub(super) struct Repository {
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub permissions: Option<RepositoryPermissions>,
    #[serde(default)]
    pub archived: bool,
    /// The merge styles the repository allows. `default_merge_style` names the one it chose, and the
    /// merge request sends that style when the repository allows it.
    #[serde(default)]
    pub default_merge_style: String,
    #[serde(default)]
    pub allow_merge_commits: bool,
    #[serde(default)]
    pub allow_rebase: bool,
    #[serde(default)]
    pub allow_rebase_explicit: bool,
    #[serde(default)]
    pub allow_squash_merge: bool,
    #[serde(default)]
    pub allow_fast_forward_only_merge: bool,
}

#[derive(serde::Deserialize, Default)]
pub(super) struct BranchCommit {
    pub id: String,
}

#[derive(serde::Deserialize)]
pub(super) struct Branch {
    pub name: String,
    pub commit: BranchCommit,
    /// Whether the effective branch rule enforces successful statuses.
    #[serde(default)]
    pub enable_status_check: bool,
    /// Contexts the effective branch rule requires; empty means every context.
    #[serde(default)]
    pub status_check_contexts: Vec<String>,
    /// Approvals the effective branch rule requires.
    #[serde(default)]
    pub required_approvals: u64,
}

/// One commit status. `status` is the result the server reports for the context.
#[derive(serde::Deserialize)]
pub(super) struct CommitStatus {
    pub id: u64,
    pub context: String,
    pub status: String,
    #[serde(default)]
    pub description: String,
}

/// One Actions run for a revision. `status` is the result the runner reported for it.
#[derive(serde::Deserialize)]
pub(super) struct ActionRun {
    pub id: u64,
    #[serde(default)]
    pub commit_sha: String,
    pub status: String,
    #[serde(default)]
    pub workflow_id: String,
}

/// The run list for one revision. Unlike the other list endpoints this one carries no
/// `x-total-count` header, so the total the pagination contract reads comes from the body.
#[derive(serde::Deserialize)]
pub(super) struct ActionRunList {
    pub workflow_runs: Vec<ActionRun>,
    pub total_count: u64,
}

/// One job of an Actions run. `status` is the result the job reported.
#[derive(serde::Deserialize)]
pub(super) struct ActionJob {
    pub id: u64,
    pub run_id: u64,
    #[serde(default)]
    pub name: String,
    pub status: String,
}

/// One pull review. `state` is `APPROVED`, `COMMENTED`, or a rejected state.
#[derive(serde::Deserialize)]
pub(super) struct PullReview {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub official: bool,
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub dismissed: bool,
}

/// The repository a pull request head points at. A pull whose head belongs to another repository
/// is a fork, which this adapter never adopts.
#[derive(serde::Deserialize)]
pub(super) struct PullRepository {
    #[serde(default)]
    pub full_name: String,
}

/// A pull branch. `label` is the branch name: `ref` reports the branch name only while the head
/// branch exists, and reports `refs/pull/<number>/head` once it has been deleted, which is the
/// normal state of a merged pull request.
#[derive(serde::Deserialize, Default)]
pub(super) struct PullBranch {
    #[serde(rename = "ref", default)]
    pub reference: String,
    #[serde(default)]
    pub label: String,
    pub sha: String,
    #[serde(default)]
    pub repo: Option<PullRepository>,
}

/// A pull request, as the delivery authority reads it. `state` is `open` or `closed`; anything
/// else keeps its reason when the observation reads it.
#[derive(serde::Deserialize)]
pub(super) struct PullRequest {
    pub number: u64,
    pub state: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub merge_commit_sha: Option<String>,
    #[serde(default)]
    pub mergeable: bool,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub base: PullBranch,
    pub head: PullBranch,
}

/// A link is built from the admitted instance, never from a value the server returned.
pub(super) fn repository_link(base_url: &SourceForgeBaseUrl, repository: &str) -> String {
    format!("{base_url}/{repository}")
}

pub(super) fn repository_identity(value: &str) -> String {
    match validate_repository(value) {
        Ok(()) => value.to_owned(),
        Err(_) => "<invalid repository>".to_owned(),
    }
}

pub(super) fn branch_identity(value: &str) -> String {
    if crate::source_code_provider::valid_git_branch(value) {
        value.to_owned()
    } else {
        "<invalid branch>".to_owned()
    }
}

pub(super) fn revision_identity(value: &str) -> String {
    if crate::source_code_provider::valid_git_revision(value) {
        value.to_owned()
    } else {
        "<invalid revision>".to_owned()
    }
}

/// An observed value is echoed only when it is plain, bounded text.
pub(super) fn observed(value: &str) -> String {
    if value.is_empty() {
        return "<empty>".to_owned();
    }
    if value.len() > 256 || value.contains(char::is_control) {
        return "<invalid value>".to_owned();
    }
    value.to_owned()
}

fn validate_repository(value: &str) -> Result<(), SourceDeliveryError> {
    let (owner, name) = value
        .split_once('/')
        .filter(|(owner, name)| !owner.is_empty() && !name.is_empty() && !name.contains('/'))
        .ok_or_else(|| SourceDeliveryError::identity(REPOSITORY_ERROR))?;
    (owner.len() <= 100 && name.len() <= 100)
        .then_some(())
        .ok_or_else(|| SourceDeliveryError::identity(REPOSITORY_ERROR))
}
