use super::*;

struct ReviewSyncInvocation<'a> {
    request: &'a GitHubReviewRequest,
    credential: GitHubCredential<'a>,
    control: &'a DriverControl,
    remaining: Duration,
}

enum ReviewSyncProgress {
    Complete(GitHubReviewReceipt),
    Failed(GitHubAuthorityError),
    TimedOut,
}

struct ReviewSyncFailure<'a> {
    control: &'a DriverControl,
    attempt: usize,
    error: GitHubAuthorityError,
    deadline: tokio::time::Instant,
    retry_interval: Duration,
}

enum ReviewSyncDisposition {
    Retry,
    Stop,
    TimedOut,
}

impl NativeV2DeliveryAdapter {
    pub(super) async fn synchronize_review(
        &self,
        request: &GitHubReviewRequest,
        credential: GitHubCredential<'_>,
        control: &DriverControl,
    ) -> Result<GitHubReviewReceipt, DeliveryStop> {
        let deadline = tokio::time::Instant::now() + REVIEW_SYNC_DEADLINE;
        let mut retry_interval = REVIEW_SYNC_INTERVAL;
        for attempt in 1..=REVIEW_SYNC_ATTEMPTS {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let error = match self
                .review_sync_attempt(ReviewSyncInvocation {
                    request,
                    credential,
                    control,
                    remaining,
                })
                .await?
            {
                ReviewSyncProgress::Complete(review) => return Ok(review),
                ReviewSyncProgress::Failed(error) => error,
                ReviewSyncProgress::TimedOut => return review_sync_timeout(control).await,
            };
            match handle_review_sync_failure(ReviewSyncFailure {
                control,
                attempt,
                error,
                deadline,
                retry_interval,
            })
            .await?
            {
                ReviewSyncDisposition::Retry => {}
                ReviewSyncDisposition::Stop => return Err(crash_outcome()),
                ReviewSyncDisposition::TimedOut => return review_sync_timeout(control).await,
            }
            retry_interval = retry_interval
                .saturating_mul(2)
                .min(REVIEW_SYNC_MAX_INTERVAL);
        }
        Err(crash_outcome())
    }

    async fn review_sync_attempt(
        &self,
        invocation: ReviewSyncInvocation<'_>,
    ) -> Result<ReviewSyncProgress, DeliveryStop> {
        if invocation.remaining.is_zero() {
            return Ok(ReviewSyncProgress::TimedOut);
        }
        ensure_active(invocation.control)?;
        let mut cancellation = invocation.control.cancellation();
        let result = tokio::select! {
            _ = cancellation.cancelled() => return Err(NodeRunnerError::Cancelled.into()),
            result = tokio::time::timeout(
                invocation.remaining,
                self.authority.open_or_update_review(
                    invocation.request,
                    invocation.credential,
                ),
            ) => result,
        };
        Ok(match result {
            Ok(Ok(review)) => ReviewSyncProgress::Complete(review),
            Ok(Err(error)) => ReviewSyncProgress::Failed(error),
            Err(_) => ReviewSyncProgress::TimedOut,
        })
    }
}

async fn wait_for_review_sync(
    control: &DriverControl,
    deadline: tokio::time::Instant,
    interval: Duration,
) -> Result<bool, DeliveryStop> {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    let wait = interval.min(remaining);
    if wait.is_zero() {
        return Ok(false);
    }
    let mut cancellation = control.cancellation();
    tokio::select! {
        _ = cancellation.cancelled() => Err(NodeRunnerError::Cancelled.into()),
        () = tokio::time::sleep(wait) => Ok(true),
    }
}

async fn handle_review_sync_failure(
    failure: ReviewSyncFailure<'_>,
) -> Result<ReviewSyncDisposition, DeliveryStop> {
    let retry = failure.attempt < REVIEW_SYNC_ATTEMPTS && failure.error.retryable_review_sync();
    let diagnostic = if retry {
        format!(
            "delivery: GitHub review synchronization attempt {} failed: {}; retrying",
            failure.attempt, failure.error
        )
    } else {
        format!(
            "delivery: GitHub review synchronization failed after {} attempt(s): {}",
            failure.attempt, failure.error
        )
    };
    emit(failure.control, &diagnostic).await?;
    if !retry {
        return Ok(ReviewSyncDisposition::Stop);
    }
    if wait_for_review_sync(failure.control, failure.deadline, failure.retry_interval).await? {
        Ok(ReviewSyncDisposition::Retry)
    } else {
        Ok(ReviewSyncDisposition::TimedOut)
    }
}

async fn review_sync_timeout(control: &DriverControl) -> Result<GitHubReviewReceipt, DeliveryStop> {
    emit(control, "delivery: GitHub review synchronization timed out").await?;
    Err(crash_outcome())
}
