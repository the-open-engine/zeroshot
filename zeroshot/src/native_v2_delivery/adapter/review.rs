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
        observation: GitHubReviewObservation,
    ) -> Result<Self, DeliveryStop> {
        match observation.state {
            GitHubReviewState::Merged { merge_revision } if valid_revision(&merge_revision) => {
                Ok(Self::Merged(merge_revision))
            }
            GitHubReviewState::Merged { .. } => {
                Err(DeliveryStop::Outcome(WorkerOutcome::malformed()))
            }
            GitHubReviewState::Open { checks } => Ok(Self::from_open(
                checks,
                observation.pull_request_ready,
                observation.head_update_required,
            )),
            GitHubReviewState::Conflict => Ok(Self::Conflict),
            GitHubReviewState::Closed => Ok(Self::Closed),
        }
    }

    fn from_open(checks: GitHubChecks, ready: bool, behind: bool) -> Self {
        match checks {
            GitHubChecks::Failed { diagnostic } => Self::CiFailed(diagnostic),
            GitHubChecks::Pending => Self::Pending,
            GitHubChecks::NotRequired | GitHubChecks::Passed if behind => Self::Behind,
            GitHubChecks::NotRequired | GitHubChecks::Passed if ready => Self::PullRequestReady,
            GitHubChecks::NotRequired | GitHubChecks::Passed => Self::Mergeable,
        }
    }
}

pub(super) fn crash_outcome() -> DeliveryStop {
    DeliveryStop::Outcome(WorkerOutcome::declared_failure(WorkerErrorCode::Crash))
}

pub(super) async fn review_completion(
    drive: &ReviewDrive<'_>,
    label: &'static str,
    diagnostic: &str,
    merge_revision: Option<&str>,
) -> Result<ReviewStep, DeliveryStop> {
    let diagnostic = drive.adapter.review_context(diagnostic);
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
