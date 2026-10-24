use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use openengine_cluster_protocol::RunId;
use tokio::process::Command;
use crate::native_v2_delivery::command::capture;

use crate::native_v2_target_authority::OperatorDiagnosticStore;

use crate::native_v2_delivery::{
    SourceDeliveryRead, SourceDeliverySnapshot, SourceHeadReconciliation,
    SourceReconciliationOutcome, SourceDeliveryError, SourceChecks, SourceConflictMaterialization,
    SourceConflictOutcome, SourceConflictRequest, SourceDeliveryCredential,
    SourceDeliveryAuthority, SourceHeadSynchronization, SourceHeadUpdateOutcome,
    SourceMergeRequestOutcome, SourceMergeWithdrawal, SourcePushRequest, SourceReviewObservation,
    SourceReviewReceipt, SourceReviewRequest, SourceReviewFeedback, SourceReviewFeedbackItem,
    SourceReviewState, SourceTargetIntegration, SourceTargetReconciliation, valid_head_update,
    valid_revision,
};

mod api;
mod comment;
pub(crate) use comment::comment_destination;
pub use comment::validate_comment_review_scope;
mod conflict;
mod feedback;
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
    /// Pinned workspace owner for Git subprocesses; GitHub API commands retain the controller identity.
    pub git_identity: Option<crate::execution::process::HostedProcessIdentity>,
    pub git_program: PathBuf,
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
    queue_effects: queue::QueueEffects,
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
            queue_effects: queue::QueueEffects::default(),
        }
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
                format!("https://github.com/{repository}.git"),
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

    fn workspace_git(&self) -> crate::native_v2_delivery::git::SystemGit {
        crate::native_v2_delivery::git::SystemGit::new(self.config.git_program.clone())
            .with_identity(self.config.git_identity)
    }

    async fn find_review(
        &self,
        request: &SourceReviewRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Option<SourceReviewReceipt>, SourceDeliveryError> {
        let value = self
            .api(
                &review_list_arguments(&request.target, &request.head_branch)?,
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

    async fn create_review(
        &self,
        request: &SourceReviewRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewReceipt, SourceDeliveryError> {
        let body = pull_request_body(request)?;
        let value = self
            .api(
                &[
                    format!("repos/{}/pulls", request.target.repository),
                    "--method".to_owned(),
                    "POST".to_owned(),
                    "-f".to_owned(),
                    format!("title={}", request.title),
                    "-f".to_owned(),
                    format!("body={body}"),
                    "-f".to_owned(),
                    format!("head={}", request.head_branch),
                    "-f".to_owned(),
                    format!("base={}", request.target.target_branch),
                ],
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
                &[format!("title={}", request.title), format!("body={body}")],
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
        fields: &[String],
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<PullRequestWire, SourceDeliveryError> {
        let mut arguments = vec![
            format!("repos/{}/pulls/{}", review.repository, review.review_id),
            "--method".to_owned(),
            "PATCH".to_owned(),
        ];
        for field in fields {
            arguments.extend(["-f".to_owned(), field.clone()]);
        }
        let value = self.api(&arguments, credential).await?;
        let wire = serde_json::from_value(value).map_err(|_| SourceDeliveryError::Rejected)?;
        require_review_identity(&wire, review)?;
        Ok(wire)
    }

    async fn policy_snapshot(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<PolicySnapshot, SourceDeliveryError> {
        let value = self.api(&query_arguments(review)?, credential).await?;
        classify_policy(value, review)
    }

    async fn policy_snapshot_with_logs(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<PolicySnapshot, SourceDeliveryError> {
        let mut snapshot = self.policy_snapshot(review, credential).await?;
        let mut logs = Vec::new();
        for job in &snapshot.failed_job_ids {
            let log = job_log_excerpt(
                self.job_log_output(&review.repository, *job, credential)
                    .await,
            )?;
            logs.push((*job, log));
        }
        include_check_logs(&mut snapshot, &logs);
        Ok(snapshot)
    }

    async fn pull_request(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<PullRequestWire, SourceDeliveryError> {
        let value = self
            .api(
                &[
                    format!("repos/{}/pulls/{}", review.repository, review.review_id),
                    "--method".to_owned(),
                    "GET".to_owned(),
                ],
                credential,
            )
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
            .api(&merge_policy::query_arguments(review)?, credential)
            .await?;
        merge_policy::classify(value, review)
    }

    async fn submit_merge(
        &self,
        review: &SourceReviewReceipt,
        selected: (merge_policy::MergeMethod, &str),
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        let (merge_method, pull_request_id) = selected;
        if merge_method == merge_policy::MergeMethod::Queue {
            return queue::enqueue(self, review, pull_request_id, credential).await;
        }
        let mut command = clean_command(&self.config, &self.config.gh_program, credential);
        command.args([
            "pr",
            "merge",
            &review.review_id,
            "--repo",
            &review.repository,
            "--match-head-commit",
            &review.head_revision,
        ]);
        command.args(merge_method_argument(merge_method));
        api::command_status(command, self.config.api_deadline, credential).await
    }

    async fn classify_rejected_merge(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
        failure: &SourceDeliveryError,
    ) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
        if failure.cleanup_unconfirmed() {
            return Err(failure.clone());
        }
        let snapshot = self.policy_snapshot(review, credential).await?;
        let permanent_response = !failure.retryable_operation();
        match snapshot.state {
            SourceReviewState::Merged { .. } => Ok(SourceMergeRequestOutcome::Accepted),
            _ if permanent_response => Err(SourceDeliveryError::Rejected),
            SourceReviewState::Conflict => Ok(SourceMergeRequestOutcome::Conflict),
            SourceReviewState::Open { .. } if snapshot.head_update.is_some() => {
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
    ) -> Result<SourceReviewFeedback, SourceDeliveryError> {
        feedback::inspect(self, review, credential).await
    }

    async fn request_merge(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
        let snapshot = self.policy_snapshot(review, credential).await?;
        let pull_request_id = snapshot.pull_request_id.clone();
        let queued = match merge_action(snapshot)? {
            MergeAction::Complete(outcome) => return Ok(outcome),
            MergeAction::Submit { queued } => queued,
        };
        let merge_method = self.merge_method(review, credential, queued).await?;
        let submitted = self
            .submit_merge(review, (merge_method, &pull_request_id), credential)
            .await;
        match submitted {
            Ok(()) => Ok(SourceMergeRequestOutcome::Accepted),
            Err(error) => match self
                .classify_rejected_merge(review, credential, &error)
                .await
            {
                Ok(outcome) => Ok(outcome),
                Err(_) => Err(error.with_context(format!(
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
        let snapshot = self.policy_snapshot(review, credential).await?;
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
    output: Result<Vec<u8>, SourceDeliveryError>,
) -> Result<String, SourceDeliveryError> {
    match output {
        Ok(output) => Ok(check_log_tail(&output)),
        Err(error) if error.authentication_failed() || error.retryable_operation() => Err(error),
        Err(error) => Ok(format!("GitHub job log unavailable: {error}")),
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
use policy::{PolicySnapshot, classify_policy, include_check_logs, query_arguments};
use source_issue::{connect_source_issue, pull_request_body, refresh_pull_request_body};
use api::check_log_tail;
use wire::{PullRequestWire, require_review_identity, review_receipt};

#[cfg(test)]
pub(super) fn test_review_request() -> SourceReviewRequest {
    SourceReviewRequest {
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

fn review_list_arguments(
    target: &super::DeliveryTarget,
    head_branch: &str,
) -> Result<Vec<String>, SourceDeliveryError> {
    let owner = target
        .repository
        .split_once('/')
        .map(|(owner, _)| owner)
        .ok_or(SourceDeliveryError::Rejected)?;
    Ok(vec![
        format!("repos/{}/pulls", target.repository),
        "--method".to_owned(),
        "GET".to_owned(),
        "-f".to_owned(),
        "state=all".to_owned(),
        "-f".to_owned(),
        format!("head={owner}:{head_branch}"),
        "-f".to_owned(),
        format!("base={}", target.target_branch),
    ])
}

fn clean_command(
    config: &GhCliAuthorityConfig,
    program: &PathBuf,
    credential: SourceDeliveryCredential<'_>,
) -> Command {
    let mut command = Command::new(program);
    command
        .kill_on_drop(true)
        .env_clear()
        .env("HOME", &config.home_directory)
        .env("LANG", "C")
        .env("GH_HOST", "github.com")
        .env("GH_TOKEN", credential.expose())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut environment = BTreeMap::from([(
        "HOME".to_owned(),
        config.home_directory.to_string_lossy().into_owned(),
    )]);
    crate::execution::platform::process_environment(&mut environment);
    command.envs(environment);
    command
}

fn git_command(
    config: &GhCliAuthorityConfig,
    workspace: &std::path::Path,
    credential: SourceDeliveryCredential<'_>,
) -> Command {
    let mut command = local_git_command(config, workspace);
    command
        .env("GH_HOST", "github.com")
        .env("GH_TOKEN", credential.expose());
    command
}

fn local_git_command(config: &GhCliAuthorityConfig, workspace: &std::path::Path) -> Command {
    let mut command = crate::native_v2_delivery::command::local_git_command_with_identity(
        &config.git_program,
        workspace,
        config.git_identity,
    );
    command.env("HOME", &config.home_directory);
    command
}

async fn configure_delivery_identity(
    config: &GhCliAuthorityConfig,
    workspace: &std::path::Path,
) -> Result<(), SourceDeliveryError> {
    for (key, value) in [
        ("user.name", "Zeroshot"),
        ("user.email", "delivery@zeroshot.invalid"),
    ] {
        let mut command = local_git_command(config, workspace);
        command.args(["config", "--local", "--replace-all", key, value]);
        bounded_status(command, config.api_deadline).await?;
    }
    Ok(())
}

fn git_error(error: crate::native_v2_delivery::git::GitError) -> SourceDeliveryError {
    match error {
        crate::native_v2_delivery::git::GitError::Command(failure) => {
            SourceDeliveryError::Command(failure)
        }
        error => SourceDeliveryError::repairable(error.to_string()),
    }
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
) -> Result<String, SourceDeliveryError> {
    let output = capture(command, deadline).await?.require_success()?;
    if output.stdout_truncated || output.stdout.len() > maximum_bytes {
        return Err(SourceDeliveryError::api(
            None,
            format!("Git output exceeded {maximum_bytes} bytes\n{output}"),
        ));
    }
    Ok(output.stdout)
}

#[cfg(test)]
#[path = "tests.rs"]
mod unit;
