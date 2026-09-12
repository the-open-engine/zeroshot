use super::*;

mod conflict;
mod head;
mod input;
mod recovery;
mod review;
mod sync;
use input::delivery_input;
use review::{crash_outcome, review_completion, ReviewProgress, ReviewStep};

#[derive(Clone)]
pub struct NativeV2DeliveryAdapter {
    config: Arc<NativeV2DeliveryConfig>,
    authority: Arc<dyn GitHubDeliveryAuthority>,
    git: SystemGit,
    trusted_github_token: Option<Arc<str>>,
    pending_head: Arc<std::sync::Mutex<Option<head::PendingHead>>>,
}

impl NativeV2DeliveryAdapter {
    #[must_use]
    pub fn new(
        config: NativeV2DeliveryConfig,
        authority: Arc<dyn GitHubDeliveryAuthority>,
    ) -> Self {
        let git = SystemGit::new(config.git_program.clone());
        Self {
            config: Arc::new(config),
            authority,
            git,
            trusted_github_token: None,
            pending_head: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Supplies target-owned checkout/delivery authority without adding it to provider children.
    #[must_use]
    pub fn with_trusted_github_token(mut self, token: Option<Arc<str>>) -> Self {
        self.trusted_github_token = token;
        self
    }
}

struct DeliverySession {
    workspace: PathBuf,
    live: AtomicBool,
}

#[async_trait]
impl NodeSession for DeliverySession {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn is_live(&self) -> bool {
        self.live.load(Ordering::SeqCst) && self.workspace.is_dir()
    }

    async fn close(&self) {
        self.live.store(false, Ordering::SeqCst);
    }
}

#[async_trait]
impl SessionFactory for NativeV2DeliveryAdapter {
    async fn open(
        &self,
        invocation: &NodeInvocation,
        _environment: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        if !matches!(invocation.binding, NodeRuntimeBinding::GitDelivery { .. })
            || DeliveryMode::from_worker(&invocation.worker).is_none()
        {
            return Err(NodeRunnerError::InvalidRole);
        }
        Ok(Arc::new(DeliverySession {
            workspace: self.config.workspace.clone(),
            live: AtomicBool::new(true),
        }))
    }
}

#[async_trait]
impl NodeDriver for NativeV2DeliveryAdapter {
    async fn run(
        &self,
        invocation: DriverInvocation,
        mut control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let (session, mut credentials, mode) = match self.authorize(&invocation) {
            Ok(authorized) => authorized,
            Err(stop) => return stop.result(),
        };
        ensure_active(&control)?;
        let mut cancellation = control.cancellation();
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(NodeRunnerError::Cancelled),
            result = async {
                let review = self.prepare_review(DeliveryPreparation {
                    invocation: &invocation, session, credentials: &mut credentials, control: &control,
                }).await?;
                self.drive_review(ReviewDrive {
                    mode, response: &invocation.response, review, credentials, control: &mut control,
                }).await
            } => result,
        };
        match result {
            Ok(outcome) => Ok(outcome),
            Err(DeliveryStop::Repair(failure)) => {
                self.repair_outcome(&invocation, mode, failure).await
            }
            Err(stop) => stop.result(),
        }
    }
}

enum DeliveryStop {
    Runner(NodeRunnerError),
    Outcome(WorkerOutcome),
    Repair(recovery::RepairFailure),
}

struct DeliveryPreparation<'a, 'environment> {
    invocation: &'a DriverInvocation,
    session: &'a DeliverySession,
    credentials: &'a mut DeliveryCredentials<'environment>,
    control: &'a DriverControl,
}

impl DeliveryStop {
    fn result(self) -> Result<WorkerOutcome, NodeRunnerError> {
        match self {
            Self::Runner(error) => Err(error),
            Self::Outcome(outcome) => Ok(outcome),
            Self::Repair(_) => Ok(WorkerOutcome::declared_failure(WorkerErrorCode::Crash)),
        }
    }
}

impl From<NodeRunnerError> for DeliveryStop {
    fn from(error: NodeRunnerError) -> Self {
        Self::Runner(error)
    }
}

struct ReviewDrive<'a> {
    mode: DeliveryMode,
    response: &'a NodeResponseContract,
    review: GitHubReviewReceipt,
    credentials: DeliveryCredentials<'a>,
    control: &'a mut DriverControl,
}

struct DeliveryCredentials<'a> {
    environment: Option<&'a ResolvedEnvironment>,
    token: String,
}

impl<'a> DeliveryCredentials<'a> {
    fn current(&self) -> GitHubCredential<'_> {
        GitHubCredential(&self.token)
    }

    fn can_refresh(&self) -> bool {
        self.environment.is_some()
    }

    async fn refresh(&mut self) -> Result<(), DeliveryStop> {
        let Some(environment) = self.environment else {
            return Ok(());
        };
        let refreshed = crate::native_v2_runner::refresh_environment(environment)
            .await
            .map_err(|_| {
                recovery::repair("GitHub credential refresh is temporarily unavailable")
            })?;
        let credential = github_credential(&refreshed)
            .ok_or_else(|| DeliveryStop::Outcome(WorkerOutcome::authentication_refusal()))?;
        self.token = credential.expose().to_owned();
        Ok(())
    }
}

impl NativeV2DeliveryAdapter {
    fn authorize<'a>(
        &'a self,
        invocation: &'a DriverInvocation,
    ) -> Result<(&'a DeliverySession, DeliveryCredentials<'a>, DeliveryMode), DeliveryStop> {
        if invocation.role != NodeRole::GitDelivery
            || !matches!(
                invocation.node.binding,
                NodeRuntimeBinding::GitDelivery { .. }
            )
        {
            return Err(DeliveryStop::Runner(NodeRunnerError::InvalidRole));
        }
        let mode = DeliveryMode::from_worker(&invocation.node.worker)
            .ok_or(DeliveryStop::Runner(NodeRunnerError::InvalidRole))?;
        validate_delivery_contract(mode, &invocation.response)?;
        let (token, environment) = match self.trusted_github_token.as_deref() {
            Some(token) => (token.to_owned(), None),
            None => (
                github_credential(&invocation.environment)
                    .ok_or_else(|| DeliveryStop::Outcome(WorkerOutcome::authentication_refusal()))?
                    .expose()
                    .to_owned(),
                Some(&invocation.environment),
            ),
        };
        let session = invocation
            .session
            .as_any()
            .downcast_ref::<DeliverySession>()
            .ok_or(DeliveryStop::Runner(NodeRunnerError::InvalidRole))?;
        Ok((session, DeliveryCredentials { environment, token }, mode))
    }

    async fn prepare_review(
        &self,
        preparation: DeliveryPreparation<'_, '_>,
    ) -> Result<GitHubReviewReceipt, DeliveryStop> {
        let input = delivery_input(&preparation.invocation.node.input)?;
        self.resume_pending_head(
            preparation.credentials,
            preparation.control,
            &delivery_branch(preparation.invocation.node.reference.run_id.as_str()),
        )
        .await?;
        let head_revision = self
            .prepare_head(preparation.session, preparation.control, &input.title)
            .await?;
        let review_request = GitHubReviewRequest {
            target: self.config.target.clone(),
            head_branch: delivery_branch(preparation.invocation.node.reference.run_id.as_str()),
            head_revision,
            title: input.title,
            description: input.description,
            source_issue: input.source_issue,
        };
        let pending = GitHubReviewReceipt {
            review_id: String::new(),
            repository: review_request.target.repository.clone(),
            target_branch: review_request.target.target_branch.clone(),
            head_branch: review_request.head_branch.clone(),
            head_revision: review_request.head_revision.clone(),
        };
        self.push_review_head(&preparation, &review_request)
            .await
            .map_err(|stop| stop.with_review(&pending))?;
        let review = self
            .synchronize_review(
                &review_request,
                preparation.credentials,
                preparation.control,
            )
            .await
            .map_err(|stop| stop.with_review(&pending))?;
        if !valid_review(&review_request, &review) {
            return Err(DeliveryStop::Outcome(WorkerOutcome::malformed()));
        }
        emit(
            preparation.control,
            "delivery: review created or rediscovered",
        )
        .await?;
        Ok(review)
    }

    async fn prepare_head(
        &self,
        session: &DeliverySession,
        control: &DriverControl,
        commit_message: &str,
    ) -> Result<String, DeliveryStop> {
        emit(control, "delivery: preparing workspace revision").await?;
        self.git
            .prepare_revision(
                &session.workspace,
                &self.config.target.base_revision,
                commit_message,
            )
            .await
            .map_err(|error| recovery::repair(error.to_string()))
    }

    async fn push_review_head(
        &self,
        preparation: &DeliveryPreparation<'_, '_>,
        review: &GitHubReviewRequest,
    ) -> Result<(), DeliveryStop> {
        let push = GitHubPushRequest {
            workspace: preparation.session.workspace.clone(),
            target: review.target.clone(),
            head_branch: review.head_branch.clone(),
            head_revision: review.head_revision.clone(),
        };
        emit(preparation.control, "delivery: pushing run branch").await?;
        self.authority
            .push_branch(&push, preparation.credentials.current())
            .await?;
        Ok(())
    }

    async fn drive_review(
        &self,
        mut drive: ReviewDrive<'_>,
    ) -> Result<WorkerOutcome, DeliveryStop> {
        let mut completed_attempts = 0usize;
        loop {
            if let Some(outcome) = self.drive_review_step(&mut drive).await? {
                return Ok(outcome);
            }
            completed_attempts = completed_attempts.saturating_add(1);
            if !self.config.poll.has_next(completed_attempts) {
                emit(
                    drive.control,
                    "delivery: authoritative confirmation timed out",
                )
                .await?;
                return Ok(WorkerOutcome::declared_failure(WorkerErrorCode::Timeout));
            }
            wait_for_poll(drive.control, self.config.poll.interval).await?;
        }
    }

    async fn drive_review_step(
        &self,
        drive: &mut ReviewDrive<'_>,
    ) -> Result<Option<WorkerOutcome>, DeliveryStop> {
        ensure_active(drive.control)?;
        let progress = self
            .observe_review(drive)
            .await
            .map_err(|stop| stop.with_review(&drive.review))?;
        match self
            .advance_review(drive, progress)
            .await
            .map_err(|stop| stop.with_review(&drive.review))?
        {
            ReviewStep::Continue => Ok(None),
            ReviewStep::Complete(outcome) => Ok(Some(outcome)),
        }
    }

    async fn advance_review(
        &self,
        drive: &mut ReviewDrive<'_>,
        progress: ReviewProgress,
    ) -> Result<ReviewStep, DeliveryStop> {
        match drive.mode {
            DeliveryMode::PullRequest => self.advance_pull_request(drive, progress).await,
            DeliveryMode::MergeV1 | DeliveryMode::Merge => {
                self.advance_merge(drive, progress).await
            }
        }
    }

    async fn advance_pull_request(
        &self,
        drive: &ReviewDrive<'_>,
        progress: ReviewProgress,
    ) -> Result<ReviewStep, DeliveryStop> {
        match progress {
            ReviewProgress::CiFailed(_)
            | ReviewProgress::Mergeable
            | ReviewProgress::Pending
            | ReviewProgress::Conflict => {
                review_completion(
                    drive,
                    DELIVERY_OPENED_LABEL,
                    "GitHub authoritatively confirmed the pull request is open",
                    None,
                )
                .await
            }
            ReviewProgress::Merged(_) | ReviewProgress::Closed => Err(crash_outcome()),
        }
    }

    async fn advance_merge(
        &self,
        drive: &mut ReviewDrive<'_>,
        progress: ReviewProgress,
    ) -> Result<ReviewStep, DeliveryStop> {
        match progress {
            ReviewProgress::Merged(merge_revision) => {
                review_completion(
                    drive,
                    DELIVERY_MERGED_LABEL,
                    "GitHub authoritatively confirmed merge",
                    Some(&merge_revision),
                )
                .await
            }
            ReviewProgress::Conflict => {
                self.complete_conflict(drive, "GitHub authoritatively reported a merge conflict")
                    .await
            }
            ReviewProgress::CiFailed(diagnostic) => {
                review_completion(drive, DELIVERY_CI_FAILED_LABEL, &diagnostic, None).await
            }
            ReviewProgress::Mergeable => self.advance_mergeable(drive).await,
            ReviewProgress::Pending => {
                emit(drive.control, "delivery: waiting for GitHub merge policy").await?;
                Ok(ReviewStep::Continue)
            }
            ReviewProgress::Closed => Err(crash_outcome()),
        }
    }

    async fn advance_mergeable(
        &self,
        drive: &mut ReviewDrive<'_>,
    ) -> Result<ReviewStep, DeliveryStop> {
        match self.request_merge(drive).await? {
            GitHubMergeRequestOutcome::Accepted => {
                emit(
                    drive.control,
                    "delivery: waiting for authoritative merge confirmation",
                )
                .await?;
            }
            GitHubMergeRequestOutcome::Pending => {
                emit(drive.control, "delivery: merge request is not yet accepted").await?;
            }
            GitHubMergeRequestOutcome::HeadUpdateRequired => {
                return self.advance_review_head(drive).await;
            }
            GitHubMergeRequestOutcome::Conflict => {
                return self
                    .complete_conflict(
                        drive,
                        "GitHub authoritatively rejected merge due to conflict",
                    )
                    .await;
            }
        }
        Ok(ReviewStep::Continue)
    }

    async fn observe_review(
        &self,
        drive: &mut ReviewDrive<'_>,
    ) -> Result<ReviewProgress, DeliveryStop> {
        let observation = match self
            .authority
            .inspect_review(&drive.review, drive.credentials.current())
            .await
        {
            Ok(observation) => observation,
            Err(error) => {
                drive
                    .credentials
                    .refresh_after(error, drive.control)
                    .await?;
                self.authority
                    .inspect_review(&drive.review, drive.credentials.current())
                    .await
                    .map_err(DeliveryStop::from)?
            }
        };
        if !valid_observation(&drive.review, &observation) {
            return Err(DeliveryStop::Outcome(WorkerOutcome::malformed()));
        }
        ReviewProgress::from_state(observation.state)
    }

    async fn request_merge(
        &self,
        drive: &mut ReviewDrive<'_>,
    ) -> Result<GitHubMergeRequestOutcome, DeliveryStop> {
        emit(drive.control, "delivery: requesting merge").await?;
        match self
            .authority
            .request_merge(&drive.review, drive.credentials.current())
            .await
        {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                drive
                    .credentials
                    .refresh_after(error, drive.control)
                    .await?;
                self.authority
                    .request_merge(&drive.review, drive.credentials.current())
                    .await
                    .map_err(DeliveryStop::from)
            }
        }
    }
}
