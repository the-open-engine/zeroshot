use super::*;

pub(super) enum ReviewProgress {
    Merged(String),
    CiFailed(String),
    Behind,
    PullRequestReady,
    Mergeable,
    Pending,
    Conflict,
    Closed,
}

pub(super) enum ReviewStep {
    Continue,
    Complete(WorkerOutcome),
}

impl ReviewStep {
    pub(super) fn completion(self) -> Option<WorkerOutcome> {
        match self {
            Self::Continue => None,
            Self::Complete(outcome) => Some(outcome),
        }
    }
}

impl ReviewProgress {
    pub(super) fn is_terminal(&self) -> bool {
        matches!(self, Self::Merged(_) | Self::Closed)
    }

    pub(super) fn from_observation(
        observation: SourceReviewObservation,
    ) -> Result<Self, DeliveryStop> {
        match observation.state {
            SourceReviewState::Merged { merge_revision } if valid_revision(&merge_revision) => {
                Ok(Self::Merged(merge_revision))
            }
            SourceReviewState::Merged { .. } => Err(DeliveryStop::failure(
                WorkerOutcome::malformed(),
                "Forge reported a merged review without a valid merge revision",
            )),
            SourceReviewState::Open { checks } => Ok(Self::from_open(
                checks,
                observation.pull_request_ready,
                observation.head_update_required,
            )),
            SourceReviewState::Conflict => Ok(Self::Conflict),
            SourceReviewState::Closed => Ok(Self::Closed),
        }
    }

    fn from_open(checks: SourceChecks, ready: bool, behind: bool) -> Self {
        match checks {
            SourceChecks::Failed { diagnostic } => Self::CiFailed(diagnostic),
            SourceChecks::Pending => Self::Pending,
            SourceChecks::NotRequired | SourceChecks::Passed if behind => Self::Behind,
            SourceChecks::NotRequired | SourceChecks::Passed if ready => Self::PullRequestReady,
            SourceChecks::NotRequired | SourceChecks::Passed => Self::Mergeable,
        }
    }
}

pub(super) fn failed_review(detail: &str) -> Result<ReviewStep, DeliveryStop> {
    Err(DeliveryStop::failure(
        WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
        detail,
    ))
}

pub(super) async fn review_completion(
    drive: &ReviewDrive<'_, '_>,
    label: &'static str,
    diagnostic: &str,
    merge_revision: Option<&str>,
) -> Result<ReviewStep, DeliveryStop> {
    let diagnostic = drive
        .adapter
        .redact_feedback(drive.credentials, drive.adapter.review_context(diagnostic));
    emit(drive.control, &diagnostic).await?;
    validate_delivery_contract(drive.mode, drive.response).map_err(DeliveryStop::Runner)?;
    delivery_outcome(
        DeliveryResult {
            mode: drive.mode,
            outcome: label,
            review: &drive.review,
            merge_revision,
        },
        &diagnostic,
    )
    .map(ReviewStep::Complete)
    .map_err(DeliveryStop::Runner)
}

#[cfg(test)]
#[path = "review/tests.rs"]
mod tests;

use crate::native_v2_delivery::validation::{
    DeliveryValidationDecision, DeliveryValidationRequest, DeliveryValidationStage,
};

struct ValidationCandidate<'a> {
    review: &'a SourceReviewReceipt,
    manifest: (&'a str, &'a str),
    stage: DeliveryValidationStage,
}

impl NativeV2DeliveryAdapter {
    pub(super) async fn validate_before_publication(
        &self,
        preparation: &DeliveryPreparation<'_, '_>,
        request: &SourceReviewRequest,
        pending: &SourceReviewReceipt,
    ) -> Result<(), DeliveryStop> {
        let mut completed_attempts = 0usize;
        loop {
            match self
                .validate_candidate(
                    ValidationCandidate {
                        review: pending,
                        manifest: (&request.title, &request.description),
                        stage: DeliveryValidationStage::Publish,
                    },
                    preparation.control,
                )
                .await?
            {
                DeliveryValidationDecision::Accepted => return Ok(()),
                DeliveryValidationDecision::Repair { reason } => {
                    return Err(recovery::repair(reason).with_review(pending));
                }
                DeliveryValidationDecision::Pending => {
                    completed_attempts = completed_attempts.saturating_add(1);
                    if !self.config.poll.has_next(completed_attempts) {
                        return Err(DeliveryStop::failure(
                            WorkerOutcome::declared_failure(WorkerErrorCode::Timeout),
                            "trusted publication validation timed out",
                        ));
                    }
                    emit(
                        preparation.control,
                        "delivery: waiting for trusted publication validation",
                    )
                    .await?;
                    wait_for_poll(preparation.control, self.config.poll.interval).await?;
                }
            }
        }
    }

    pub(super) async fn validate_review(
        &self,
        drive: &ReviewDrive<'_, '_>,
        stage: DeliveryValidationStage,
    ) -> Result<bool, DeliveryStop> {
        let manifest = delivery_input(drive.manifest)?;
        match self
            .validate_candidate(
                ValidationCandidate {
                    review: &drive.review,
                    manifest: (&manifest.title, &manifest.description),
                    stage,
                },
                drive.control,
            )
            .await?
        {
            DeliveryValidationDecision::Accepted => Ok(true),
            DeliveryValidationDecision::Repair { reason } => {
                Err(recovery::repair(reason).with_review(&drive.review))
            }
            DeliveryValidationDecision::Pending => {
                emit(
                    drive.control,
                    "delivery: waiting for trusted merge validation",
                )
                .await?;
                Ok(false)
            }
        }
    }

    async fn validate_candidate(
        &self,
        candidate: ValidationCandidate<'_>,
        control: &DriverControl,
    ) -> Result<DeliveryValidationDecision, DeliveryStop> {
        let Some(validator) = &self.trusted_delivery_validator else {
            return Ok(DeliveryValidationDecision::Accepted);
        };
        ensure_active(control)?;
        let ValidationCandidate {
            review,
            manifest,
            stage,
        } = candidate;
        let tree = self
            .git
            .revision_tree(&self.config.workspace, &review.head_revision)
            .await
            .map_err(recovery::git_failure)?;
        let known = self.delivery_state();
        let request = DeliveryValidationRequest {
            stage,
            repository: &review.repository,
            target_branch: &review.target_branch,
            head_branch: &review.head_branch,
            head_revision: &review.head_revision,
            tree: &tree,
            review_base_revision: known.review_base_revision.as_deref(),
            title: manifest.0,
            description: manifest.1,
            review_id: (!review.review_id.is_empty()).then_some(review.review_id.as_str()),
        };
        let decision = validator.validate(request, control.cancellation()).await?;
        ensure_active(control)?;
        if matches!(decision, DeliveryValidationDecision::Accepted) {
            let (head, dirty) = self
                .git
                .workspace_state(&self.config.workspace)
                .await
                .map_err(recovery::git_failure)?;
            if head != review.head_revision || dirty {
                return Err(NodeRunnerError::DriverDetail(
                    "Trusted delivery validation changed the candidate workspace".to_owned(),
                )
                .into());
            }
        }
        Ok(decision)
    }
}

// A failed/cancelled merge delivery cannot hand a live autonomous merge to an agent.
impl NativeV2DeliveryAdapter {
    pub(super) async fn finish_merge_handoff(
        &self,
        contract: (DeliveryMode, &NodeResponseContract),
        credentials: &mut DeliveryCredentials<'_>,
        outcome: Result<WorkerOutcome, NodeRunnerError>,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let (mode, response) = contract;
        let state = self.delivery_state();
        let latest = state
            .pending_head
            .and_then(|pending| pending.updated)
            .or(state.published);
        let Some(review) = handoff_review(mode, &outcome, latest) else {
            return outcome;
        };
        let withdrawn = self.withdraw_owned_merge(&review, credentials).await;
        if matches!(&outcome, Err(NodeRunnerError::CleanupUnconfirmed)) {
            return Err(NodeRunnerError::CleanupUnconfirmed);
        }
        match withdrawn {
            Ok(SourceMergeWithdrawal::Withdrawn) => outcome,
            Ok(SourceMergeWithdrawal::Merged { merge_revision })
                if valid_revision(&merge_revision) =>
            {
                validate_delivery_contract(mode, response)?;
                delivery_outcome(
                    DeliveryResult {
                        mode,
                        outcome: DELIVERY_MERGED_LABEL,
                        review: &review,
                        merge_revision: Some(&merge_revision),
                    },
                    "forge confirmed the exact candidate merged before queue withdrawal",
                )
            }
            other => Err(self.withdrawal_error((&review, credentials), other.err(), outcome)),
        }
    }

    async fn withdraw_owned_merge(
        &self,
        review: &SourceReviewReceipt,
        credentials: &mut DeliveryCredentials<'_>,
    ) -> Result<SourceMergeWithdrawal, SourceDeliveryError> {
        let result = self
            .authority
            .withdraw_merge_request(review, credentials.current())
            .await;
        if matches!(&result, Err(error) if error.authentication_failed())
            && credentials.can_refresh()
            && credentials.try_refresh().await.is_ok()
        {
            return self
                .authority
                .withdraw_merge_request(review, credentials.current())
                .await;
        }
        result
    }

    fn withdrawal_error(
        &self,
        context: (&SourceReviewReceipt, &DeliveryCredentials<'_>),
        cause: Option<SourceDeliveryError>,
        original: Result<WorkerOutcome, NodeRunnerError>,
    ) -> NodeRunnerError {
        if cause
            .as_ref()
            .is_some_and(SourceDeliveryError::cleanup_unconfirmed)
        {
            return NodeRunnerError::CleanupUnconfirmed;
        }
        let (review, credentials) = context;
        let cause = cause.map_or_else(
            || "invalid authoritative merged revision".to_owned(),
            |error| error.to_string(),
        );
        let original = match original {
            Err(error) => error.to_string(),
            Ok(WorkerOutcome::Verifier { diagnostic, .. }) => diagnostic.to_string(),
            Ok(value) => format!("{value:?}"),
        };
        NodeRunnerError::DriverDetail(self.redact_feedback(credentials, format!(
            "Cannot confirm safe merge withdrawal for {}#{} into {} at {}. No repair handoff is authorized.\n{cause}\nOriginal delivery result: {original}",
            review.repository, review.review_id, review.target_branch, review.head_revision,
        )))
    }
}

fn handoff_review(
    mode: DeliveryMode,
    outcome: &Result<WorkerOutcome, NodeRunnerError>,
    published: Option<SourceReviewReceipt>,
) -> Option<SourceReviewReceipt> {
    if !matches!(
        mode,
        DeliveryMode::MergeV1 | DeliveryMode::Merge | DeliveryMode::MergeV3
    ) || matches!(outcome, Ok(WorkerOutcome::Verifier { output, .. }) if output.get("outcome").and_then(Value::as_str) == Some(DELIVERY_MERGED_LABEL))
    {
        return None;
    }
    published.filter(|review| !review.review_id.is_empty())
}
