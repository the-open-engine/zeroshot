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
            SourceReviewState::Merged { .. } => {
                Err(DeliveryStop::Outcome(WorkerOutcome::malformed()))
            }
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

pub(super) async fn failed_review(
    control: &DriverControl,
    detail: &str,
) -> Result<ReviewStep, DeliveryStop> {
    let error = NodeRunnerError::DriverDetail(detail.to_owned());
    let _ = report_provider_error("Git delivery", &error, &[], control).await;
    Err(crash_outcome())
}

pub(super) fn crash_outcome() -> DeliveryStop {
    DeliveryStop::Outcome(WorkerOutcome::declared_failure(WorkerErrorCode::Crash))
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
