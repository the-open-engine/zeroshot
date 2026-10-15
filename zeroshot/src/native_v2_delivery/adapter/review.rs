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

impl ReviewProgress {
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
