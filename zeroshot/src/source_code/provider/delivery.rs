//! Provider-neutral trusted-delivery contracts. Git transport and forge effects are supplied
//! by registered adapters; private credential capabilities live only for each invocation.
use std::fmt;
use std::path::PathBuf;
use async_trait::async_trait;
use super::SourceDeliveryError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeliveryTarget {
    pub repository: String,
    pub target_branch: String,
    pub base_revision: String,
}

impl DeliveryTarget {
    pub fn new(
        repository: impl Into<String>,
        target_branch: impl Into<String>,
        base_revision: impl Into<String>,
    ) -> Result<Self, DeliveryConfigError> {
        let target = Self {
            repository: repository.into(),
            target_branch: target_branch.into(),
            base_revision: base_revision.into(),
        };
        if !valid_delivery_repository(&target.repository) {
            return Err(DeliveryConfigError::Repository);
        }
        if !valid_git_branch(&target.target_branch) {
            return Err(DeliveryConfigError::Branch);
        }
        if !valid_git_revision(&target.base_revision) {
            return Err(DeliveryConfigError::Revision);
        }
        Ok(target)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DeliveryConfigError {
    #[error("repository identity must be bounded, nonempty text")]
    Repository,
    #[error("target branch is not a bounded Git branch name")]
    Branch,
    #[error("base revision must be a lowercase 40- or 64-character Git revision")]
    Revision,
    #[error("delivery polling requires at least one observation")]
    PollAttempts,
}

/// Borrowed credential authority. It is intentionally neither serializable nor printable.
#[derive(Clone, Copy)]
pub struct SourceDeliveryCredential<'a> {
    token: &'a str,
    github_git_token: Option<&'a str>,
    git: Option<SourceGitCredential<'a>>,
}

/// Borrowed private Git authority; it is never part of a request, receipt, or stored identity.
#[derive(Clone, Copy)]
pub enum SourceGitCredential<'a> {
    Https {
        username: &'a str,
        password: &'a str,
    },
    Ssh {
        private_key: &'a str,
        known_hosts: &'a str,
    },
}

impl fmt::Debug for SourceGitCredential<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SourceGitCredential([REDACTED])")
    }
}

impl<'a> SourceDeliveryCredential<'a> {
    #[must_use]
    pub const fn token(token: &'a str) -> Self {
        Self {
            token,
            git: None,
            github_git_token: None,
        }
    }

    #[must_use]
    pub const fn with_forge_token(mut self, token: &'a str) -> Self {
        self.token = token;
        self
    }

    /// Legacy GitHub Git authority remains separate from another forge's API token.
    #[must_use]
    pub const fn with_github_git_token(mut self, token: &'a str) -> Self {
        self.github_git_token = Some(token);
        self
    }

    #[must_use]
    pub const fn github_git_token(self) -> Option<&'a str> {
        self.github_git_token
    }

    #[must_use]
    pub const fn with_git(mut self, credential: SourceGitCredential<'a>) -> Self {
        self.git = Some(credential);
        self
    }

    #[must_use]
    pub const fn git(self) -> Option<SourceGitCredential<'a>> {
        self.git
    }

    #[must_use]
    pub fn expose(self) -> &'a str {
        self.token
    }
}

impl fmt::Debug for SourceDeliveryCredential<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SourceDeliveryCredential([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePushRequest {
    pub workspace: PathBuf,
    pub target: DeliveryTarget,
    pub head_branch: String,
    pub head_revision: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceReviewRequest {
    pub target: DeliveryTarget,
    pub head_branch: String,
    pub head_revision: String,
    pub title: String,
    pub description: String,
    pub source_issue: Option<SourceDeliveryIssue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceDeliveryIssue {
    pub number: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceReviewReceipt {
    pub review_id: String,
    pub repository: String,
    pub target_branch: String,
    pub head_branch: String,
    pub head_revision: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceChecks {
    NotRequired,
    Pending,
    Passed,
    Failed { diagnostic: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceReviewState {
    Open { checks: SourceChecks },
    Merged { merge_revision: String },
    Conflict,
    Closed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceReviewObservation {
    pub review_id: String,
    pub repository: String,
    pub target_branch: String,
    pub head_branch: String,
    pub head_revision: String,
    pub state: SourceReviewState,
    pub pull_request_ready: bool,
    pub head_update_required: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceReviewFeedback {
    pub items: Vec<SourceReviewFeedbackItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceReviewFeedbackItem {
    pub key: String,
    pub version: String,
    pub author: String,
    pub location: Option<String>,
    pub body: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceConflictRequest {
    pub workspace: PathBuf,
    pub review: SourceReviewReceipt,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceConflictMaterialization {
    pub target_revision: String,
    pub conflicted_paths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceConflictOutcome {
    Materialized(SourceConflictMaterialization),
    ObservationChanged,
}

/// Target-owned, bounded source-provider effects. Implementations must bound every network operation.
#[async_trait]
pub trait SourceDeliveryAuthority: Send + Sync {
    /// One provider-owned API credential field. Git-only providers require no forge credential.
    fn forge_credential_field(&self) -> Option<&'static str> {
        None
    }

    /// Observes the bound PR and branch without committing, pushing, or changing metadata.
    async fn observe_delivery(
        &self,
        request: SourceDeliveryRead<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceDeliverySnapshot, SourceDeliveryError>;

    /// Integrates an exact current target revision before the run branch is first published.
    async fn reconcile_delivery_target(
        &self,
        request: SourceTargetReconciliation<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceTargetIntegration, SourceDeliveryError>;

    /// Fetches the observed head and preserves local work while reconciling its published ancestry.
    async fn reconcile_delivery_head(
        &self,
        request: SourceHeadReconciliation<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReconciliationOutcome, SourceDeliveryError>;

    async fn push_branch(
        &self,
        request: &SourcePushRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError>;

    /// Opens the review or updates the existing run-stable review after an authored loop revisit.
    async fn open_or_update_review(
        &self,
        request: &SourceReviewRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewReceipt, SourceDeliveryError>;

    async fn inspect_review(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewObservation, SourceDeliveryError>;

    /// Reads every visible PR discussion surface behind one stable review/head identity fence.
    async fn inspect_review_feedback(
        &self,
        _review: &SourceReviewReceipt,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewFeedback, SourceDeliveryError> {
        Ok(SourceReviewFeedback { items: Vec::new() })
    }

    /// Requests provider-native integration after the provider reports its merge policy ready.
    /// Acceptance is not proof; only a later merged observation confirms success.
    async fn request_merge(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceMergeRequestOutcome, SourceDeliveryError>;

    /// Confirms that no deferred merge can race a repair/error handoff, or reports an exact merge.
    /// The default is valid only for authorities that never leave autonomous merge effects.
    /// Authorities with queues or deferred auto-merge must override and confirm withdrawal.
    async fn withdraw_merge_request(
        &self,
        _review: &SourceReviewReceipt,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceMergeWithdrawal, SourceDeliveryError> {
        Ok(SourceMergeWithdrawal::Withdrawn)
    }

    /// Advances a stale non-queue review head through one provider-authorized CAS transition.
    async fn update_review_head(
        &self,
        _workspace: &std::path::Path,
        _review: &SourceReviewReceipt,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceHeadUpdateOutcome, SourceDeliveryError> {
        Ok(SourceHeadUpdateOutcome::Pending)
    }

    /// Adopts one provider-authorized review-head transition in the local workspace.
    /// Existing authorities that adopt before returning `Updated` may use this no-op default.
    async fn synchronize_review_head(
        &self,
        _request: SourceHeadSynchronization<'_>,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        Ok(())
    }

    /// Fetches and integrates the exact current target revision without exposing credentials to
    /// an agent. A materialized conflict must remain in the workspace; a changed observation must
    /// leave the original review head clean so delivery can re-observe GitHub.
    async fn materialize_merge_conflict(
        &self,
        _request: &SourceConflictRequest,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceConflictOutcome, SourceDeliveryError> {
        Err(SourceDeliveryError::Rejected)
    }
}

#[derive(Clone, Copy)]
pub struct SourceHeadSynchronization<'a> {
    pub workspace: &'a std::path::Path,
    pub previous: &'a SourceReviewReceipt,
    pub updated: &'a SourceReviewReceipt,
}

impl SourceReviewReceipt {
    pub(crate) fn observation(&self, state: SourceReviewState) -> SourceReviewObservation {
        self.observation_with_readiness(state, false, false)
    }

    pub(crate) fn observation_with_readiness(
        &self,
        state: SourceReviewState,
        pull_request_ready: bool,
        head_update_required: bool,
    ) -> SourceReviewObservation {
        SourceReviewObservation {
            review_id: self.review_id.clone(),
            repository: self.repository.clone(),
            target_branch: self.target_branch.clone(),
            head_branch: self.head_branch.clone(),
            head_revision: self.head_revision.clone(),
            state,
            pull_request_ready,
            head_update_required,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceMergeRequestOutcome {
    Accepted,
    Pending,
    HeadUpdateRequired,
    Conflict,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceMergeWithdrawal {
    Withdrawn,
    Merged { merge_revision: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceHeadUpdateOutcome {
    Updated(SourceReviewReceipt),
    Pending,
    Conflict,
}

/// Identity-constrained read before any delivery mutation.
#[derive(Clone, Copy)]
pub struct SourceDeliveryRead<'a> {
    pub target: &'a DeliveryTarget,
    pub head_branch: &'a str,
    pub known_review: Option<&'a SourceReviewReceipt>,
    pub include_review: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceDeliverySnapshot {
    pub review: Option<SourceReviewObservation>,
    pub head_revision: Option<String>,
}

#[derive(Clone, Copy)]
pub struct SourceHeadReconciliation<'a> {
    pub workspace: &'a std::path::Path,
    pub published: &'a SourceReviewReceipt,
    pub observed: &'a SourceReviewReceipt,
    pub commit_message: &'a str,
    pub authorized_update: bool,
    pub adopting_existing: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceReconciliationOutcome {
    Unchanged,
    Adopted,
    NeedsWork(String),
    Refused(String),
}

/// Target integration retains the admitted source revision as provenance.
#[derive(Clone, Copy)]
pub struct SourceTargetReconciliation<'a> {
    pub workspace: &'a std::path::Path,
    pub target: &'a DeliveryTarget,
    pub commit_message: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTargetIntegration {
    pub target_revision: String,
    pub outcome: SourceReconciliationOutcome,
}

pub(crate) fn valid_delivery_repository(value: &str) -> bool {
    !value.is_empty() && value.len() <= 2048 && !value.chars().any(char::is_control)
}

pub(crate) fn valid_git_branch(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.starts_with('-')
        && !value.contains("..")
        && !value.ends_with('.')
        && !value.ends_with('/')
        && !value.bytes().any(|byte| {
            byte.is_ascii_control()
                || byte.is_ascii_whitespace()
                || matches!(byte, b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
        })
}

pub(crate) fn valid_git_revision(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(crate) const DELIVERY_GENERATED_BODY_START: &str =
    "<!-- zeroshot-delivery:generated:v1:start -->";
pub(crate) const DELIVERY_GENERATED_BODY_END: &str = "<!-- zeroshot-delivery:generated:v1:end -->";

pub(crate) fn valid_delivery_description(description: &str) -> bool {
    !description.contains(DELIVERY_GENERATED_BODY_START)
        && !description.contains(DELIVERY_GENERATED_BODY_END)
}

impl SourceReviewReceipt {
    /// Build the returned identity only after the provider allocates or rediscovers a review ID.
    #[must_use]
    pub fn for_request(request: &SourceReviewRequest, review_id: impl Into<String>) -> Self {
        Self {
            review_id: review_id.into(),
            repository: request.target.repository.clone(),
            target_branch: request.target.target_branch.clone(),
            head_branch: request.head_branch.clone(),
            head_revision: request.head_revision.clone(),
        }
    }
}
