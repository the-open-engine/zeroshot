use super::ci_diagnostics::JobLog;
use http::Request;
use http_api::{GraphqlPages, GraphqlRead, repository_path};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use openengine_cluster_protocol::RunId;
use tokio::process::Command;
use crate::native_v2_delivery::command::{GitCommandOutput, capture, capture_sensitive};

use crate::native_v2_target_authority::OperatorDiagnosticStore;
use crate::source_code_provider::SourceReviewLinks;

use crate::native_v2_delivery::{
    SourceDeliveryRead, SourceDeliverySnapshot, SourceHeadReconciliation,
    SourceReconciliationOutcome, SourceDeliveryError, SourceChecks, SourceConflictOutcome,
    SourceConflictRequest, SourceDeliveryCredential, SourceDeliveryAuthority,
    SourceHeadSynchronization, SourceHeadUpdateOutcome, SourceMergeRequestOutcome,
    SourceMergeWithdrawal, SourcePushRequest, SourceReviewObservation, SourceReviewReceipt,
    SourceReviewRequest, SourceReviewFeedback, SourceReviewFeedbackItem, SourceReviewState,
    SourceTargetIntegration, SourceTargetReconciliation, valid_head_update, valid_revision,
};

mod access;
mod api;
mod comment;
mod instance;
pub(crate) use comment::{comment_destination, comment_destination_at, validate_comment_review_scope};
pub(super) use comment::valid_github_permalink as valid_comment_permalink;
mod conflict;
pub(crate) mod conversations;
mod feedback;
mod http;
mod http_api;
mod metadata;
mod observation;
mod push;
mod queue;
pub(crate) mod review_task;
mod target;

#[cfg(all(test, target_os = "linux"))]
mod ownership_tests;

const DEFAULT_API_DEADLINE: Duration = Duration::from_secs(2 * 60);
const DEFAULT_PUSH_DEADLINE: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Debug)]
pub struct GhCliAuthorityConfig {
    /// Pinned workspace owner for Git subprocesses.
    pub git_identity: Option<crate::execution::process::HostedProcessIdentity>,
    pub git_program: PathBuf,
    /// Retained for Rust source compatibility. Production API requests use the native client.
    pub gh_program: PathBuf,
    pub home_directory: PathBuf,
    pub api_deadline: Duration,
    pub push_deadline: Duration,
}

impl GhCliAuthorityConfig {
    #[must_use]
    pub fn hosted(home_directory: PathBuf) -> Self {
        Self {
            git_identity: None,
            git_program: PathBuf::from("/usr/bin/git"),
            gh_program: PathBuf::from("/usr/bin/gh"),
            home_directory,
            api_deadline: DEFAULT_API_DEADLINE,
            push_deadline: DEFAULT_PUSH_DEADLINE,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GhCliDeliveryAuthority {
    config: GhCliAuthorityConfig,
    operator_diagnostics: Option<OperatorDiagnosticReporter>,
    transport: Option<crate::native_v2_delivery::transport::GitTransport>,
    merge_effects: queue::MergeEffects,
    resolutions: conversations::PendingResolutions,
    published_replies: super::review_responses::PublishedReplies,
    base_url: openengine_cluster_protocol::SourceForgeBaseUrl,
    instance_verified: Arc<tokio::sync::OnceCell<()>>,
    http: Arc<tokio::sync::OnceCell<http::Http>>,
    network: super::network::ForgeNetwork,
}

#[derive(Clone, Debug)]
struct OperatorDiagnosticReporter {
    run_id: RunId,
    store: Arc<OperatorDiagnosticStore>,
}

impl GhCliDeliveryAuthority {
    #[must_use]
    pub fn new(config: GhCliAuthorityConfig) -> Self {
        Self {
            config,
            operator_diagnostics: None,
            transport: None,
            merge_effects: queue::MergeEffects::default(),
            resolutions: conversations::PendingResolutions::default(),
            published_replies: super::review_responses::PublishedReplies::default(),
            base_url: openengine_cluster_protocol::SourceForgeBaseUrl::new("https://github.com")
                .expect("static base URL"),
            instance_verified: Arc::new(tokio::sync::OnceCell::new()),
            http: Arc::new(tokio::sync::OnceCell::new()),
            network: Default::default(),
        }
    }

    pub(crate) fn with_network(mut self, network: super::network::ForgeNetwork) -> Self {
        self.network = network;
        self.http = Arc::new(tokio::sync::OnceCell::new());
        self.instance_verified = Arc::new(tokio::sync::OnceCell::new());
        self
    }

    #[must_use]
    pub fn with_git_transport(
        mut self,
        transport: crate::native_v2_delivery::transport::GitTransport,
    ) -> Self {
        self.transport = Some(transport);
        self
    }

    fn git_transport(
        &self,
        workspace: &std::path::Path,
        repository: &str,
    ) -> crate::native_v2_delivery::transport::GitTransport {
        let mut transport = self.transport.clone().unwrap_or_else(|| {
            crate::native_v2_delivery::transport::GitTransport::new(
                crate::native_v2_delivery::transport::GitTransportConfig {
                    git_program: self.config.git_program.clone(),
                    workspace: workspace.to_owned(),
                    home_directory: self.config.home_directory.clone(),
                    private_directory: self.config.home_directory.clone(),
                    identity: self.config.git_identity,
                    local_environment: None,
                    deadline: self.config.push_deadline,
                },
                format!("{}/{repository}.git", self.base_url),
            )
        });
        if let Some(reporter) = &self.operator_diagnostics {
            transport = transport
                .with_operator_diagnostics(reporter.run_id.clone(), reporter.store.clone());
        }
        transport
    }

    pub(crate) fn with_operator_diagnostics(
        mut self,
        run_id: RunId,
        store: Arc<OperatorDiagnosticStore>,
    ) -> Self {
        self.operator_diagnostics = Some(OperatorDiagnosticReporter { run_id, store });
        self
    }

    async fn find_review(
        &self,
        request: &SourceReviewRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Option<SourceReviewReceipt>, SourceDeliveryError> {
        if let Some(known) = &request.known_review {
            if known.repository != request.target.repository
                || known.target_branch != request.target.target_branch
                || known.head_branch != request.head_branch
            {
                return Err(SourceDeliveryError::identity(
                    "known PR differs from the admitted delivery target",
                ));
            }
            let path = repository_path(&request.target.repository, &["pulls", &known.review_id])?;
            let value = self
                .rest_review_sync(Request::get(&path), credential)
                .await?;
            let wire: PullRequestWire = self.decode_response(value, credential)?;
            let receipt = review_receipt(wire, request)?;
            if receipt.review_id != known.review_id {
                return Err(SourceDeliveryError::identity(
                    "the bound PR identity changed",
                ));
            }
            return Ok(Some(receipt));
        }
        let value = self
            .review_list(
                (&request.target.repository, &request.head_branch),
                request.include_terminal_reviews,
                credential,
            )
            .await?;
        let reviews: Vec<PullRequestWire> =
            serde_json::from_value(value).map_err(|_| SourceDeliveryError::Rejected)?;
        let mut exact = reviews
            .into_iter()
            .map(|review| review_receipt(review, request))
            .collect::<Result<Vec<_>, _>>()?;
        if exact.len() > 1 {
            return Err(SourceDeliveryError::Rejected);
        }
        Ok(exact.pop())
    }

    async fn review_list(
        &self,
        (repository, head_branch): (&str, &str),
        include_terminal: bool,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Value, SourceDeliveryError> {
        let path = repository_path(repository, &["pulls"])?;
        let head = format!("{}:{head_branch}", path[1]);
        let state = if include_terminal { "all" } else { "open" };
        self.rest(
            Request::Rest {
                method: Method::GET,
                path: &path,
                query: &[("state", state), ("head", &head)],
                body: None,
            },
            credential,
        )
        .await
    }

    async fn create_review(
        &self,
        request: &SourceReviewRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewReceipt, SourceDeliveryError> {
        let body = pull_request_body(request)?;
        let path = repository_path(&request.target.repository, &["pulls"])?;
        let value = self
            .rest(
                Request::Rest {
                    method: Method::POST,
                    path: &path,
                    query: &[],
                    body: Some(&json!({
                        "title": request.title,
                        "body": body,
                        "head": request.head_branch,
                        "base": request.target.target_branch,
                    })),
                },
                credential,
            )
            .await?;
        let review: PullRequestWire =
            serde_json::from_value(value).map_err(|_| SourceDeliveryError::Rejected)?;
        if review.title.as_deref() != Some(request.title.as_str())
            || review.body.as_deref() != Some(body.as_str())
        {
            return Err(SourceDeliveryError::Rejected);
        }
        review_receipt(review, request)
    }

    async fn refresh_review_metadata(
        &self,
        request: &SourceReviewRequest,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        let mut wire = self.pull_request(review, credential).await?;
        require_review_identity(&wire, review)?;
        let body = refresh_pull_request_body(wire.body.as_deref(), request)?;
        if wire.title.as_deref() == Some(request.title.as_str())
            && wire.body.as_deref() == Some(body.as_str())
        {
            return Ok(());
        }
        wire = self
            .patch_review(
                review,
                (
                    &json!({"title": request.title, "body": body}),
                    request.known_review.is_some(),
                ),
                credential,
            )
            .await?;
        if wire.title.as_deref() != Some(request.title.as_str())
            || wire.body.as_deref() != Some(body.as_str())
        {
            return Err(SourceDeliveryError::Rejected);
        }
        Ok(())
    }

    async fn patch_review(
        &self,
        review: &SourceReviewReceipt,
        (body, bound): (&Value, bool),
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<PullRequestWire, SourceDeliveryError> {
        let path = repository_path(&review.repository, &["pulls", &review.review_id])?;
        let request = Request::Rest {
            method: Method::PATCH,
            path: &path,
            query: &[],
            body: Some(body),
        };
        let value = if bound {
            self.rest_review_sync(request, credential).await?
        } else {
            self.rest(request, credential).await?
        };
        let wire = serde_json::from_value(value).map_err(|_| SourceDeliveryError::Rejected)?;
        require_review_identity(&wire, review)?;
        Ok(wire)
    }

    async fn policy_snapshot(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<PolicySnapshot, SourceDeliveryError> {
        let mut variables = review_variables(review)?;
        variables["head"] = Value::String(review.head_revision.clone());
        variables["branch"] = Value::String(review.target_branch.clone());
        let value = self
            .graphql_pages(
                GraphqlPages {
                    document: policy::POLICY_QUERY,
                    cursor_path: &[
                        "data",
                        "repository",
                        "pullRequest",
                        "commits",
                        "nodes",
                        "0",
                        "commit",
                        "statusCheckRollup",
                        "contexts",
                        "pageInfo",
                    ],
                    read: GraphqlRead::Policy,
                },
                variables,
                credential,
            )
            .await?;
        let mut snapshot = classify_policy(value, review)?;
        if matches!(
            snapshot.state,
            SourceReviewState::Open {
                checks: SourceChecks::NotRequired | SourceChecks::Passed
            }
        ) {
            if !crate::source_code_provider::valid_git_branch(&review.target_branch) {
                return Err(SourceDeliveryError::Rejected);
            }
            let path = repository_path(
                &review.repository,
                &["rules", "branches", &review.target_branch],
            )?;
            let rules = self.rest_pages(&path, credential, true).await?;
            snapshot.apply_branch_rules(rules)?;
        }
        Ok(snapshot)
    }

    async fn policy_snapshot_with_logs(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<PolicySnapshot, SourceDeliveryError> {
        let mut snapshot = self.policy_snapshot(review, credential).await?;
        let removal_failure =
            queue::observe_removal(self, review, &mut snapshot, credential).await?;
        let mut logs = Vec::new();
        for job in &snapshot.failed_job_ids {
            let log = job_log_excerpt(
                self.job_log_output(&review.repository, *job, credential)
                    .await,
            )?;
            logs.push((*job, log));
        }
        if let Some(failure) = removal_failure {
            failure.revalidate(self, review, &mut snapshot).await?;
        }
        include_check_logs(&mut snapshot, &logs);
        Ok(snapshot)
    }

    async fn pull_request(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<PullRequestWire, SourceDeliveryError> {
        let path = repository_path(&review.repository, &["pulls", &review.review_id])?;
        let value = self
            .rest_review_sync(Request::get(&path), credential)
            .await?;
        serde_json::from_value(value).map_err(|_| SourceDeliveryError::Rejected)
    }

    async fn merge_method(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
        queued: bool,
    ) -> Result<merge_policy::MergeMethod, SourceDeliveryError> {
        if queued {
            return Ok(merge_policy::MergeMethod::Queue);
        }
        let value = self
            .graphql_pages(
                GraphqlPages {
                    document: merge_policy::MERGE_POLICY_QUERY,
                    cursor_path: &[
                        "data",
                        "repository",
                        "pullRequest",
                        "baseRef",
                        "rules",
                        "pageInfo",
                    ],
                    read: GraphqlRead::Policy,
                },
                review_variables(review)?,
                credential,
            )
            .await?;
        merge_policy::classify(value, review)
    }

    async fn classify_rejected_merge(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
        (method, failure): (merge_policy::MergeMethod, &api::MutationFailure),
    ) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
        let error = &failure.error;
        if error.cleanup_unconfirmed() {
            return Err(error.clone());
        }
        let mut snapshot = self.policy_snapshot(review, credential).await?;
        if method == merge_policy::MergeMethod::Queue {
            let _ = queue::observe_removal(self, review, &mut snapshot, credential).await?;
        }
        if !error.authentication_failed()
            && matches!(error.api_status(), None | Some(405 | 409 | 422))
            && failure.rejection == Some(api::MutationRejection::RequiredCheckQueued)
            && matches!(snapshot.state, SourceReviewState::Open { .. })
        {
            return Ok(SourceMergeRequestOutcome::Pending);
        }
        if !error.authentication_failed()
            && matches!(error.api_status(), None | Some(405 | 409 | 422))
            && failure.rejection == Some(api::MutationRejection::Conversation)
            && !matches!(
                snapshot.state,
                SourceReviewState::Merged { .. } | SourceReviewState::Closed
            )
            && conversations::has_blocker(self, review, credential).await?
        {
            return Ok(SourceMergeRequestOutcome::ReviewBlocked);
        }
        let permanent_response = !error.retryable_operation();
        match snapshot.state {
            SourceReviewState::Merged { .. } => Ok(SourceMergeRequestOutcome::Accepted),
            _ if permanent_response => Err(SourceDeliveryError::Rejected),
            SourceReviewState::Conflict => Ok(SourceMergeRequestOutcome::Conflict),
            SourceReviewState::Open {
                checks: SourceChecks::NotRequired | SourceChecks::Passed,
            } if snapshot.head_update.is_some() => {
                Ok(SourceMergeRequestOutcome::HeadUpdateRequired)
            }
            // A changed or pending gate cannot explain away a permanent merge rejection.
            // Preserve the command failure unless GitHub confirms a specific reconciliation.
            SourceReviewState::Open { .. } | SourceReviewState::Closed => {
                Err(SourceDeliveryError::Rejected)
            }
        }
    }
}

#[async_trait]
impl SourceDeliveryAuthority for GhCliDeliveryAuthority {
    async fn check_access(
        &self,
        repository: &str,
        credential: SourceDeliveryCredential<'_>,
        stopping: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> Result<(), SourceDeliveryError> {
        access::check(self, repository, credential, stopping).await
    }

    async fn review_conversation_state(
        &self,
    ) -> Result<Option<serde_json::Value>, SourceDeliveryError> {
        conversations::checkpoint(self).await
    }

    async fn restore_review_conversation_state(
        &self,
        state: serde_json::Value,
    ) -> Result<(), SourceDeliveryError> {
        conversations::restore(self, state).await
    }

    fn review_links(
        &self,
        review: &SourceReviewReceipt,
    ) -> Result<Option<SourceReviewLinks>, SourceDeliveryError> {
        self.canonical_review_links(review).map(Some)
    }
    fn forge_credential_field(&self) -> Option<&'static str> {
        Some("GH_TOKEN")
    }

    async fn observe_delivery(
        &self,
        request: SourceDeliveryRead<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceDeliverySnapshot, SourceDeliveryError> {
        observation::observe(self, request, credential).await
    }

    async fn reconcile_delivery_target(
        &self,
        request: SourceTargetReconciliation<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceTargetIntegration, SourceDeliveryError> {
        target::reconcile(self, request, credential).await
    }

    async fn reconcile_delivery_head(
        &self,
        request: SourceHeadReconciliation<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
        head::reconcile(self, request, credential).await
    }

    async fn push_branch(
        &self,
        request: &SourcePushRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        push::push_branch(self, request, credential).await
    }

    async fn open_or_update_review(
        &self,
        request: &SourceReviewRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewReceipt, SourceDeliveryError> {
        let review = match self.find_review(request, credential).await? {
            Some(review) => {
                self.refresh_review_metadata(request, &review, credential)
                    .await?;
                Ok(review)
            }
            None => {
                self.confirm_review_head(request, credential).await?;
                self.create_review(request, credential).await
            }
        }?;
        connect_source_issue(self, request, &review, credential).await?;
        Ok(review)
    }

    async fn inspect_review(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewObservation, SourceDeliveryError> {
        let snapshot = self.policy_snapshot_with_logs(review, credential).await?;
        let head_update_required = snapshot.head_update.is_some();
        Ok(review.observation_with_readiness(
            snapshot.state,
            snapshot.pull_request_ready,
            head_update_required,
        ))
    }

    async fn inspect_review_feedback(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
        checkpoint: &dyn crate::source_code_provider::SourceReviewCheckpoint,
    ) -> Result<SourceReviewFeedback, SourceDeliveryError> {
        conversations::reconcile_resolutions(self, review, credential, checkpoint).await?;
        feedback::inspect(self, review, credential).await
    }

    async fn resolve_review_conversation(
        &self,
        request: crate::source_code_provider::SourceReviewResolutionRequest<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<crate::source_code_provider::SourceReviewResolution, SourceDeliveryError> {
        conversations::resolve(self, request, credential).await
    }

    async fn request_merge(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
        checkpoint: &dyn crate::source_code_provider::SourceReviewCheckpoint,
    ) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
        if conversations::reconcile_resolutions(self, review, credential, checkpoint).await? {
            return Ok(SourceMergeRequestOutcome::ReviewBlocked);
        }
        let mut snapshot = self.policy_snapshot(review, credential).await?;
        let _ = queue::observe_removal(self, review, &mut snapshot, credential).await?;
        let pull_request_id = snapshot.pull_request_id.clone();
        let queued = match merge_action(snapshot)? {
            MergeAction::Complete(outcome) => return Ok(outcome),
            MergeAction::Submit { queued } => queued,
        };
        let merge_method = self.merge_method(review, credential, queued).await?;
        let submitted =
            queue::merge(self, review, (merge_method, &pull_request_id), credential).await;
        match submitted {
            Ok(()) => Ok(SourceMergeRequestOutcome::Accepted),
            Err(failure) => match self
                .classify_rejected_merge(review, credential, (merge_method, &failure))
                .await
            {
                Ok(outcome) => Ok(outcome),
                Err(error) if error.retries_exhausted() => Err(self.policy_exhausted(
                    error.with_context(failure.error),
                    "during merge reconciliation",
                    credential,
                )),
                Err(_) => Err(failure.error.with_context(format!(
                    "Merge request for {}#{} into {} using {merge_method:?} failed. \
                     Check GitHub's rejection and base-branch policy before retrying delivery.",
                    review.repository, review.review_id, review.target_branch,
                ))),
            },
        }
    }

    async fn withdraw_merge_request(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceMergeWithdrawal, SourceDeliveryError> {
        queue::withdraw(self, review, credential).await
    }

    async fn update_review_head(
        &self,
        workspace: &std::path::Path,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceHeadUpdateOutcome, SourceDeliveryError> {
        let mut snapshot = self.policy_snapshot(review, credential).await?;
        if snapshot.head_update.is_some() {
            let _ = queue::observe_removal(self, review, &mut snapshot, credential).await?;
        }
        match snapshot.state {
            SourceReviewState::Conflict => Ok(SourceHeadUpdateOutcome::Conflict),
            SourceReviewState::Open { .. } => match snapshot.head_update {
                Some(update) => head::update_review_head(
                    self,
                    head::HeadUpdateRequest {
                        workspace,
                        review,
                        pull_request_id: &update.0,
                    },
                    credential,
                )
                .await
                .map(SourceHeadUpdateOutcome::Updated),
                None => Ok(SourceHeadUpdateOutcome::Pending),
            },
            SourceReviewState::Merged { .. } => Ok(SourceHeadUpdateOutcome::Pending),
            SourceReviewState::Closed => Err(SourceDeliveryError::Rejected),
        }
    }

    async fn synchronize_review_head(
        &self,
        request: SourceHeadSynchronization<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        head::synchronize_review_head(self, request, credential).await
    }

    async fn materialize_merge_conflict(
        &self,
        request: &SourceConflictRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceConflictOutcome, SourceDeliveryError> {
        conflict::materialize(self, request, credential).await
    }
}

fn job_log_excerpt(
    output: Result<JobLog, SourceDeliveryError>,
) -> Result<JobLog, SourceDeliveryError> {
    match output {
        Ok(output) => Ok(output),
        Err(error) if error.authentication_failed() || error.retryable_operation() => Err(error),
        Err(error) => Ok(JobLog::unavailable(format!(
            "GitHub job log unavailable: {error}"
        ))),
    }
}

enum MergeAction {
    Complete(SourceMergeRequestOutcome),
    Submit { queued: bool },
}

fn merge_action(snapshot: PolicySnapshot) -> Result<MergeAction, SourceDeliveryError> {
    let outcome = match (
        snapshot.state,
        snapshot.head_update,
        snapshot.pull_request_ready,
    ) {
        (SourceReviewState::Merged { .. }, _, _) => SourceMergeRequestOutcome::Accepted,
        (SourceReviewState::Conflict, _, _) => SourceMergeRequestOutcome::Conflict,
        (
            SourceReviewState::Open {
                checks: SourceChecks::NotRequired | SourceChecks::Passed,
            },
            None,
            false,
        ) => {
            return Ok(MergeAction::Submit {
                queued: snapshot.is_merge_queue_enabled,
            });
        }
        (
            SourceReviewState::Open {
                checks: SourceChecks::NotRequired | SourceChecks::Passed,
            },
            Some(_),
            false,
        ) => SourceMergeRequestOutcome::HeadUpdateRequired,
        (SourceReviewState::Open { .. }, _, _) => SourceMergeRequestOutcome::Pending,
        (SourceReviewState::Closed, _, _) => return Err(SourceDeliveryError::Rejected),
    };
    Ok(MergeAction::Complete(outcome))
}

#[cfg(test)]
const fn merge_method_argument(method: merge_policy::MergeMethod) -> Option<&'static str> {
    match method {
        merge_policy::MergeMethod::Queue => None,
        merge_policy::MergeMethod::Merge => Some("--merge"),
        merge_policy::MergeMethod::Squash => Some("--squash"),
        merge_policy::MergeMethod::Rebase => Some("--rebase"),
    }
}

mod head;
mod merge_policy;
mod policy;
mod source_issue;
mod wire;
#[cfg(test)]
use policy::query_arguments;
use policy::{PolicySnapshot, classify_policy, include_check_logs};
use source_issue::{connect_source_issue, pull_request_body, refresh_pull_request_body};
use wire::{PullRequestWire, require_review_identity, review_receipt};

#[cfg(test)]
pub(super) fn test_review_request() -> SourceReviewRequest {
    SourceReviewRequest {
        include_terminal_reviews: true,
        known_review: None,
        target: super::DeliveryTarget::new(
            "acme/project",
            "main",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("valid test delivery target"),
        head_branch: "zeroshot/v2-run".to_owned(),
        head_revision: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
        title: "fix: repair checkout".to_owned(),
        description: "Repair the checkout flow.".to_owned(),
        source_issue: None,
    }
}

fn review_variables(review: &SourceReviewReceipt) -> Result<Value, SourceDeliveryError> {
    let (owner, name) = review
        .repository
        .split_once('/')
        .ok_or(SourceDeliveryError::Rejected)?;
    let number = review
        .review_id
        .parse::<std::num::NonZeroU64>()
        .map_err(|_| SourceDeliveryError::Rejected)?;
    Ok(json!({"owner": owner, "name": name, "number": number.get()}))
}

#[cfg(test)]
fn review_query_arguments(
    review: &SourceReviewReceipt,
    query: &str,
) -> Result<Vec<String>, SourceDeliveryError> {
    let (owner, name) = review
        .repository
        .split_once('/')
        .filter(|(owner, name)| !owner.is_empty() && !name.is_empty() && !name.contains('/'))
        .ok_or(SourceDeliveryError::Rejected)?;
    let number = review
        .review_id
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or(SourceDeliveryError::Rejected)?;
    Ok(vec![
        "graphql".to_owned(),
        "--paginate".to_owned(),
        "--slurp".to_owned(),
        "-f".to_owned(),
        format!("query={query}"),
        "-F".to_owned(),
        format!("owner={owner}"),
        "-F".to_owned(),
        format!("name={name}"),
        "-F".to_owned(),
        format!("number={number}"),
    ])
}

fn git_command(
    authority: &GhCliDeliveryAuthority,
    workspace: &std::path::Path,
    credential: SourceDeliveryCredential<'_>,
) -> Command {
    let mut command = local_git_command(authority, workspace);
    command
        .env("GH_HOST", "github.com")
        .env("GH_TOKEN", credential.expose());
    command
}

fn local_git_command(authority: &GhCliDeliveryAuthority, workspace: &std::path::Path) -> Command {
    if let Some(transport) = &authority.transport {
        return transport.command(workspace);
    }
    let config = &authority.config;
    let mut command = crate::native_v2_delivery::command::local_git_command_with_identity(
        &config.git_program,
        workspace,
        config.git_identity,
    );
    command.env("HOME", &config.home_directory);
    command
}

async fn bounded_status(
    mut command: Command,
    deadline: Duration,
) -> Result<(), SourceDeliveryError> {
    capture(&mut command, deadline).await?.require_success()?;
    Ok(())
}

async fn bounded_git_output(
    command: &mut Command,
    deadline: Duration,
    maximum_bytes: usize,
) -> Result<GitCommandOutput, SourceDeliveryError> {
    let output = capture_sensitive(command, deadline, &[])
        .await?
        .require_success()?;
    if output.stdout_truncated() || output.stdout().len() > maximum_bytes {
        return Err(SourceDeliveryError::api(
            None,
            format!("Git output exceeded {maximum_bytes} bytes\n{output}"),
        ));
    }
    Ok(output)
}

#[cfg(test)]
#[path = "tests.rs"]
mod unit;
#[cfg(all(test, unix))]
pub(crate) use unit::policy_page as test_policy_page;
