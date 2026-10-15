//! Native-v2's graph-visible Git delivery node.
//!
//! Two graph worker references select PR or merge completion on one shared implementation. Both
//! commit the workspace, push one deterministic run branch, and create or rediscover one stable
//! review. Only authoritative open/merge observations produce successful inline receipts;
//! conflict and CI failure remain routable attempt results.

mod adapter;
mod authority_error;
pub(crate) mod command;
pub(crate) mod contract;
pub(crate) mod git;
#[doc(hidden)]
pub mod git_auth;
mod review_head;
#[cfg(test)]
mod tests;
pub mod transport;
#[cfg(all(test, unix))]
#[path = "native_v2_delivery/tests/transport_http.rs"]
mod transport_http;
pub mod validation;

pub use command::GitCommandFailure;
pub use crate::source_code_provider::*;
pub use contract::{is_matching_success_receipt, validate_delivery_contract};
#[cfg(test)]
pub(crate) use contract::{delivery_diagnostic_schema, delivery_result_schema, delivery_signal_labels};

use std::any::Any;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use openengine_cluster_protocol::RunId;
use openengine_cluster_protocol::{EnumLabel, FieldName, WorkerErrorCode, WorkerOutcome, WorkerRef};
use serde_json::{Map, Value, json};

use crate::native_v2_contract::{
    EnvironmentVariableName, GIT_DELIVERY_MERGE_V2_WORKER_REF, GIT_DELIVERY_MERGE_V3_WORKER_REF,
    GIT_DELIVERY_MERGE_WORKER_REF, GIT_DELIVERY_PR_V2_WORKER_REF, GIT_DELIVERY_PR_WORKER_REF,
    GIT_DELIVERY_PUSH_WORKER_REF, NodeInvocation, NodeRuntimeBinding, PullRequestFeedback,
};
use crate::native_v2_runner::{
    DriverControl, DriverInvocation, LiveOutput, LiveOutputStream, NodeDriver,
    NodeResponseContract, NodeRole, NodeRunnerError, NodeSession, ResolvedEnvironment,
    SessionFactory,
};

use self::git::SystemGit;
pub(crate) use self::review_head::valid_head_update;
pub(crate) use crate::source_code_provider::{
    valid_delivery_repository as valid_repository, valid_git_branch as valid_branch,
    valid_git_revision as valid_revision,
};

pub const GITHUB_TOKEN_ENV: &str = "GH_TOKEN";
pub const DELIVERY_SIGNAL_FIELD: &str = "delivery";
pub const DELIVERY_PUSHED_LABEL: &str = "pushed";
pub const DELIVERY_OPENED_LABEL: &str = "opened";
pub const DELIVERY_READY_LABEL: &str = "ready";
pub const DELIVERY_MERGED_LABEL: &str = "merged";
pub const DELIVERY_CONFLICT_LABEL: &str = "conflict";
pub const DELIVERY_CI_FAILED_LABEL: &str = "ci_failed";
pub const DELIVERY_REPAIR_REQUIRED_LABEL: &str = "repair_required";

const DELIVERY_V1_RESULT_VERSION: &str = "v1";
const DELIVERY_V2_RESULT_VERSION: &str = "v2";
const DELIVERY_V3_RESULT_VERSION: &str = "v3";
const DELIVERY_VERSION_FIELD: &str = "version";
const DELIVERY_MODE_FIELD: &str = "mode";
const DELIVERY_OUTCOME_FIELD: &str = "outcome";
const DELIVERY_REPOSITORY_FIELD: &str = "repository";
const DELIVERY_TARGET_BRANCH_FIELD: &str = "targetBranch";
const DELIVERY_HEAD_REVISION_FIELD: &str = "headRevision";
const DELIVERY_HEAD_BRANCH_FIELD: &str = "headBranch";
const DELIVERY_MERGE_REVISION_FIELD: &str = "mergeRevision";
const DELIVERY_PULL_REQUEST_ID_FIELD: &str = "pullRequestId";

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(20);
const REVIEW_SYNC_ATTEMPTS: usize = 10;
const REVIEW_SYNC_DEADLINE: Duration = Duration::from_secs(60);
const REVIEW_SYNC_INTERVAL: Duration = Duration::from_secs(1);
const REVIEW_SYNC_MAX_INTERVAL: Duration = Duration::from_secs(8);
const MAX_TOKEN_BYTES: usize = 4_096;
const MAX_REVIEW_ID_BYTES: usize = 256;
const MAX_CONFLICT_DIAGNOSTIC_BYTES: usize = 8 * 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryMode {
    Push,
    PullRequest,
    PullRequestV2,
    /// The shipped merge@1 receipt contract, retained for stored and in-flight graphs.
    MergeV1,
    /// The merge@2 receipt contract with an authoritative merge revision.
    Merge,
    /// The feedback-aware merge@3 receipt contract.
    MergeV3,
}

impl DeliveryMode {
    #[cfg(test)]
    pub(crate) const fn worker_ref(self) -> &'static str {
        match self {
            Self::Push => GIT_DELIVERY_PUSH_WORKER_REF,
            Self::PullRequest => GIT_DELIVERY_PR_WORKER_REF,
            Self::PullRequestV2 => GIT_DELIVERY_PR_V2_WORKER_REF,
            Self::MergeV1 => GIT_DELIVERY_MERGE_WORKER_REF,
            Self::Merge => GIT_DELIVERY_MERGE_V2_WORKER_REF,
            Self::MergeV3 => GIT_DELIVERY_MERGE_V3_WORKER_REF,
        }
    }

    #[must_use]
    pub fn from_worker(worker: &WorkerRef) -> Option<Self> {
        match worker.as_str() {
            GIT_DELIVERY_PUSH_WORKER_REF => Some(Self::Push),
            GIT_DELIVERY_PR_WORKER_REF => Some(Self::PullRequest),
            GIT_DELIVERY_PR_V2_WORKER_REF => Some(Self::PullRequestV2),
            GIT_DELIVERY_MERGE_WORKER_REF => Some(Self::MergeV1),
            GIT_DELIVERY_MERGE_V2_WORKER_REF => Some(Self::Merge),
            GIT_DELIVERY_MERGE_V3_WORKER_REF => Some(Self::MergeV3),
            _ => None,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Push => "push",
            Self::PullRequest | Self::PullRequestV2 => "pr",
            Self::MergeV1 | Self::Merge | Self::MergeV3 => "merge",
        }
    }

    const fn success_outcome(self) -> &'static str {
        match self {
            Self::Push => DELIVERY_PUSHED_LABEL,
            Self::PullRequest => DELIVERY_OPENED_LABEL,
            Self::PullRequestV2 => DELIVERY_READY_LABEL,
            Self::MergeV1 | Self::Merge | Self::MergeV3 => DELIVERY_MERGED_LABEL,
        }
    }

    const fn result_version(self) -> &'static str {
        match self {
            Self::Push | Self::PullRequest | Self::MergeV1 => DELIVERY_V1_RESULT_VERSION,
            Self::PullRequestV2 | Self::Merge => DELIVERY_V2_RESULT_VERSION,
            Self::MergeV3 => DELIVERY_V3_RESULT_VERSION,
        }
    }

    const fn includes_merge_revision(self) -> bool {
        matches!(self, Self::Merge | Self::MergeV3)
    }

    const fn creates_review(self) -> bool {
        !matches!(self, Self::Push)
    }

    pub(crate) const fn considers_feedback(self) -> bool {
        matches!(self, Self::PullRequestV2 | Self::MergeV3)
    }

    /// Provider capabilities used by the existing workflow node.
    #[must_use]
    pub const fn required_capabilities(
        self,
    ) -> &'static [crate::source_code_provider::SourceCapability] {
        use crate::source_code_provider::SourceCapability::{Read, Push, PullRequest, Checks, Merge};
        match self {
            Self::Push => &[Read, Push],
            Self::PullRequest => &[Read, Push, PullRequest],
            Self::PullRequestV2 => &[Read, Push, PullRequest, Checks],
            Self::MergeV1 | Self::Merge | Self::MergeV3 => {
                &[Read, Push, PullRequest, Checks, Merge]
            }
        }
    }

    const fn is_merge(self) -> bool {
        matches!(self, Self::MergeV1 | Self::Merge | Self::MergeV3)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeliveryPollPolicy {
    maximum_attempts: Option<usize>,
    pub interval: Duration,
}

impl DeliveryPollPolicy {
    pub fn new(attempts: usize, interval: Duration) -> Result<Self, DeliveryConfigError> {
        if attempts == 0 {
            return Err(DeliveryConfigError::PollAttempts);
        }
        Ok(Self {
            maximum_attempts: Some(attempts),
            interval,
        })
    }

    const fn until_cancelled(interval: Duration) -> Self {
        Self {
            maximum_attempts: None,
            interval,
        }
    }

    const fn has_next(self, completed_attempts: usize) -> bool {
        match self.maximum_attempts {
            Some(maximum) => completed_attempts < maximum,
            None => true,
        }
    }
}

impl Default for DeliveryPollPolicy {
    fn default() -> Self {
        Self::until_cancelled(DEFAULT_POLL_INTERVAL)
    }
}

#[derive(Clone, Debug)]
pub struct NativeV2DeliveryConfig {
    pub delivery_run_id: RunId,
    pub adopt_existing_delivery: bool,
    /// Pinned workspace owner for hosted Git; local execution inherits the caller.
    pub git_identity: Option<crate::execution::process::HostedProcessIdentity>,
    pub workspace: PathBuf,
    pub git_program: PathBuf,
    pub target: DeliveryTarget,
    pub poll: DeliveryPollPolicy,
}

#[derive(Clone, Debug)]
pub struct DeliveryLineage {
    delivery_run_id: RunId,
    adopt_existing_delivery: bool,
}

impl DeliveryLineage {
    #[must_use]
    pub fn new(delivery_run_id: RunId, adopt_existing_delivery: bool) -> Self {
        Self {
            delivery_run_id,
            adopt_existing_delivery,
        }
    }
}

impl NativeV2DeliveryConfig {
    #[must_use]
    pub fn for_hosted_workspace(
        lineage: DeliveryLineage,
        workspace: PathBuf,
        target: DeliveryTarget,
        identity: crate::execution::process::HostedProcessIdentity,
    ) -> Self {
        Self {
            delivery_run_id: lineage.delivery_run_id,
            adopt_existing_delivery: lineage.adopt_existing_delivery,
            workspace,
            git_identity: Some(identity),
            git_program: PathBuf::from("/usr/bin/git"),
            target,
            poll: DeliveryPollPolicy::default(),
        }
    }
}

pub use adapter::NativeV2DeliveryAdapter;
mod token_file;
pub use token_file::TrustedGitHubTokenFile;
pub(crate) use token_file::{GITHUB_TOKEN_FILE_ENV, validate_token as validate_github_delivery_token};

fn forge_credential<'a>(environment: &'a ResolvedEnvironment, field: &str) -> Option<&'a str> {
    let name = EnvironmentVariableName::new(field).ok()?;
    let token = environment.get(&name)?;
    (!token.trim().is_empty() && token.len() <= MAX_TOKEN_BYTES).then_some(token)
}

struct DeliveryResult<'a> {
    mode: DeliveryMode,
    outcome: &'a str,
    review: &'a SourceReviewReceipt,
    merge_revision: Option<&'a str>,
}

fn delivery_outcome(
    result: DeliveryResult<'_>,
    diagnostic: &str,
) -> Result<WorkerOutcome, NodeRunnerError> {
    if !valid_delivery_result(&result) {
        return Err(NodeRunnerError::Driver);
    }
    let field = FieldName::new(DELIVERY_SIGNAL_FIELD).map_err(|_| NodeRunnerError::Driver)?;
    let label = EnumLabel::new(result.outcome).map_err(|_| NodeRunnerError::Driver)?;
    Ok(WorkerOutcome::Verifier {
        output: delivery_result(&result),
        signals: BTreeMap::from([(field, label)]),
        diagnostic: json!({"message":diagnostic}),
        artifacts: Vec::new(),
    })
}

fn valid_delivery_result(result: &DeliveryResult<'_>) -> bool {
    if result.outcome == DELIVERY_REPAIR_REQUIRED_LABEL {
        return result.merge_revision.is_none();
    }
    valid_delivery_identity(result)
        && valid_mode_outcome(result.mode, result.outcome)
        && valid_delivery_revision(result)
}

fn valid_delivery_identity(result: &DeliveryResult<'_>) -> bool {
    valid_repository(&result.review.repository)
        && valid_branch(&result.review.target_branch)
        && valid_branch(&result.review.head_branch)
        && valid_revision(&result.review.head_revision)
        && if matches!(result.mode, DeliveryMode::Push) {
            result.review.review_id.is_empty()
        } else {
            valid_review_id(&result.review.review_id)
        }
}

fn valid_delivery_revision(result: &DeliveryResult<'_>) -> bool {
    match (
        result.mode.is_merge(),
        result.outcome,
        result.merge_revision,
    ) {
        (true, DELIVERY_MERGED_LABEL, Some(revision)) => valid_revision(revision),
        (_, _, None) => true,
        _ => false,
    }
}

fn valid_mode_outcome(mode: DeliveryMode, outcome: &str) -> bool {
    if outcome == DELIVERY_REPAIR_REQUIRED_LABEL {
        return true;
    }
    match mode {
        DeliveryMode::Push => outcome == DELIVERY_PUSHED_LABEL,
        DeliveryMode::PullRequest => outcome == DELIVERY_OPENED_LABEL,
        DeliveryMode::PullRequestV2 => matches!(
            outcome,
            DELIVERY_READY_LABEL | DELIVERY_CONFLICT_LABEL | DELIVERY_CI_FAILED_LABEL
        ),
        DeliveryMode::MergeV1 | DeliveryMode::Merge | DeliveryMode::MergeV3 => matches!(
            outcome,
            DELIVERY_MERGED_LABEL | DELIVERY_CONFLICT_LABEL | DELIVERY_CI_FAILED_LABEL
        ),
    }
}

fn conflict_diagnostic(
    authority_diagnostic: &str,
    materialization: &SourceConflictMaterialization,
) -> Option<String> {
    if !valid_revision(&materialization.target_revision)
        || materialization.conflicted_paths.is_empty()
        || !materialization
            .conflicted_paths
            .iter()
            .all(|path| valid_conflicted_path(path))
    {
        return None;
    }
    let paths = serde_json::to_string(&materialization.conflicted_paths).ok()?;
    let diagnostic = format!(
        "{authority_diagnostic}; targetRevision={}; conflictedPaths={paths}",
        materialization.target_revision
    );
    (diagnostic.len() <= MAX_CONFLICT_DIAGNOSTIC_BYTES).then_some(diagnostic)
}

fn valid_conflicted_path(value: &str) -> bool {
    use std::path::Component;

    !value.is_empty()
        && !std::path::Path::new(value).is_absolute()
        && std::path::Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn delivery_result(result: &DeliveryResult<'_>) -> Value {
    let mut fields = Map::from_iter([
        (
            DELIVERY_VERSION_FIELD.to_owned(),
            Value::String(result.mode.result_version().to_owned()),
        ),
        (
            DELIVERY_MODE_FIELD.to_owned(),
            Value::String(result.mode.label().to_owned()),
        ),
        (
            DELIVERY_OUTCOME_FIELD.to_owned(),
            Value::String(result.outcome.to_owned()),
        ),
        (
            DELIVERY_REPOSITORY_FIELD.to_owned(),
            Value::String(result.review.repository.clone()),
        ),
        (
            DELIVERY_TARGET_BRANCH_FIELD.to_owned(),
            Value::String(result.review.target_branch.clone()),
        ),
        (
            DELIVERY_HEAD_REVISION_FIELD.to_owned(),
            Value::String(result.review.head_revision.clone()),
        ),
    ]);
    if result.mode == DeliveryMode::Push {
        fields.insert(
            DELIVERY_HEAD_BRANCH_FIELD.to_owned(),
            Value::String(result.review.head_branch.clone()),
        );
    } else {
        fields.insert(
            DELIVERY_PULL_REQUEST_ID_FIELD.to_owned(),
            Value::String(result.review.review_id.clone()),
        );
    }
    if result.mode.includes_merge_revision() {
        fields.insert(
            DELIVERY_MERGE_REVISION_FIELD.to_owned(),
            Value::String(result.merge_revision.unwrap_or_default().to_owned()),
        );
    }
    Value::Object(fields)
}

fn valid_review(request: &SourceReviewRequest, review: &SourceReviewReceipt) -> bool {
    valid_review_id(&review.review_id)
        && review.repository == request.target.repository
        && review.target_branch == request.target.target_branch
        && review.head_branch == request.head_branch
        && review.head_revision == request.head_revision
}

fn valid_observation(review: &SourceReviewReceipt, observation: &SourceReviewObservation) -> bool {
    observation.review_id == review.review_id
        && observation.repository == review.repository
        && observation.target_branch == review.target_branch
        && observation.head_branch == review.head_branch
        && observation.head_revision == review.head_revision
}

fn delivery_branch(run_id: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(run_id.as_bytes());
    let suffix = digest
        .get(..10)
        .unwrap_or(digest.as_slice())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("zeroshot/v2-{suffix}")
}

async fn wait_for_poll(control: &DriverControl, interval: Duration) -> Result<(), NodeRunnerError> {
    let mut cancellation = control.cancellation();
    tokio::select! {
        _ = cancellation.cancelled() => Err(NodeRunnerError::Cancelled),
        () = tokio::time::sleep(interval) => Ok(()),
    }
}

fn ensure_active(control: &DriverControl) -> Result<(), NodeRunnerError> {
    (!control.is_cancelled())
        .then_some(())
        .ok_or(NodeRunnerError::Cancelled)
}

async fn emit(control: &DriverControl, message: &str) -> Result<(), NodeRunnerError> {
    let mut remaining = message;
    while !remaining.is_empty() {
        let mut end = remaining
            .len()
            .min(crate::native_v2_runner::MAX_LIVE_OUTPUT_BYTES);
        while !remaining.is_char_boundary(end) {
            end -= 1;
        }
        control
            .emit(LiveOutput::new(
                LiveOutputStream::System,
                &remaining[..end],
            )?)
            .await?;
        remaining = &remaining[end..];
    }
    Ok(())
}

fn valid_review_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_REVIEW_ID_BYTES && !value.chars().any(char::is_control)
}

/// Derives delivery identity from admitted provenance and the workflow's intended operation.
pub fn source_target(
    source: &crate::native_v2_contract::ResolvedSource,
    mode: DeliveryMode,
) -> Result<DeliveryTarget, DeliveryConfigError> {
    let repository = if mode.creates_review() {
        source
            .remote
            .as_ref()
            .and_then(|remote| remote.forge.as_ref())
            .map_or(source.repository.as_str(), |forge| {
                forge.repository.as_str()
            })
    } else {
        source.repository.as_str()
    };
    DeliveryTarget::new(repository, source.branch.as_str(), source.revision.as_str())
}
