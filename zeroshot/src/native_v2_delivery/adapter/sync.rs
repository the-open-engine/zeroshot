use std::future::Future;

use super::*;
use crate::execution::driver::DriverCancellation;
use crate::native_v2_runner::EnvironmentRefreshError;

struct ReviewSyncInvocation<'a> {
    request: &'a GitHubReviewRequest,
    credential: GitHubCredential<'a>,
    control: &'a DriverControl,
    remaining: Duration,
}

struct ReviewSyncState<'a, 'environment> {
    request: &'a GitHubReviewRequest,
    credentials: &'a mut DeliveryCredentials<'environment>,
    control: &'a DriverControl,
    deadline: tokio::time::Instant,
    credential_refreshed: bool,
}

impl ReviewSyncState<'_, '_> {
    fn invocation(&self) -> ReviewSyncInvocation<'_> {
        ReviewSyncInvocation {
            request: self.request,
            credential: self.credentials.current(),
            control: self.control,
            remaining: self
                .deadline
                .saturating_duration_since(tokio::time::Instant::now()),
        }
    }

    fn should_refresh_credential(&self, progress: &ReviewSyncProgress) -> bool {
        matches!(
            progress,
            ReviewSyncProgress::Failed(error) if error.authentication_failed()
        ) && self.credentials.can_refresh()
            && !self.credential_refreshed
    }

    async fn refresh_credential(&mut self) -> Result<CredentialRefreshProgress, DeliveryStop> {
        emit(self.control, "delivery: refreshing GitHub credential").await?;
        let mut interval = REVIEW_SYNC_INTERVAL;
        loop {
            let progress = self.refresh_attempt().await?;
            match progress {
                CredentialRefreshProgress::Complete => {
                    self.credential_refreshed = true;
                    return Ok(progress);
                }
                CredentialRefreshProgress::TimedOut => return Ok(progress),
                CredentialRefreshProgress::Unavailable => {}
            }
            emit(
                self.control,
                "delivery: GitHub credential refresh temporarily unavailable; retrying",
            )
            .await?;
            if !wait_for_review_sync(self.control, self.deadline, interval).await? {
                return Ok(CredentialRefreshProgress::TimedOut);
            }
            interval = interval.saturating_mul(2).min(REVIEW_SYNC_MAX_INTERVAL);
        }
    }

    async fn refresh_attempt(&mut self) -> Result<CredentialRefreshProgress, DeliveryStop> {
        ensure_active(self.control)?;
        let remaining = self
            .deadline
            .saturating_duration_since(tokio::time::Instant::now());
        refresh_within_deadline(
            self.credentials.try_refresh(),
            self.control.cancellation(),
            remaining,
        )
        .await
    }
}

enum ReviewSyncProgress {
    Complete(GitHubReviewReceipt),
    Failed(GitHubAuthorityError),
    TimedOut,
}

enum CredentialRefreshProgress {
    Complete,
    Unavailable,
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
        credentials: &mut DeliveryCredentials<'_>,
        control: &DriverControl,
    ) -> Result<GitHubReviewReceipt, DeliveryStop> {
        let mut retry = preflight::OperationRetry::default();
        loop {
            let error = match self
                .synchronize_review_batch(request, credentials, control)
                .await
            {
                Err(DeliveryStop::Retry(error)) => error,
                result => return result,
            };
            self.retry_operation(
                error,
                preflight::OperationContext {
                    credentials,
                    control,
                    retry: &mut retry,
                },
            )
            .await?;
        }
    }

    async fn synchronize_review_batch(
        &self,
        request: &GitHubReviewRequest,
        credentials: &mut DeliveryCredentials<'_>,
        control: &DriverControl,
    ) -> Result<GitHubReviewReceipt, DeliveryStop> {
        let deadline = tokio::time::Instant::now() + REVIEW_SYNC_DEADLINE;
        let mut retry_interval = REVIEW_SYNC_INTERVAL;
        let mut last_failure = None;
        let mut state = ReviewSyncState {
            request,
            credentials,
            control,
            deadline,
            credential_refreshed: false,
        };
        for attempt in 1..=REVIEW_SYNC_ATTEMPTS {
            let progress = self.review_sync_progress(&mut state).await?;
            let error = match progress {
                ReviewSyncProgress::Complete(review) => return Ok(review),
                ReviewSyncProgress::Failed(error) => {
                    last_failure = Some(error.clone());
                    error
                }
                ReviewSyncProgress::TimedOut => {
                    return Err(last_failure.map_or_else(
                        || temporary_sync_failure("GitHub review synchronization timed out").into(),
                        |error: GitHubAuthorityError| {
                            error
                                .with_context("GitHub review synchronization timed out")
                                .into()
                        },
                    ));
                }
            };
            match handle_review_sync_failure(ReviewSyncFailure {
                control,
                attempt,
                error: error.clone(),
                deadline,
                retry_interval,
            })
            .await?
            {
                ReviewSyncDisposition::Retry => {}
                ReviewSyncDisposition::Stop => return Err(error.into()),
                ReviewSyncDisposition::TimedOut => {
                    return Err(error
                        .with_context("GitHub review synchronization timed out")
                        .into());
                }
            }
            retry_interval = retry_interval
                .saturating_mul(2)
                .min(REVIEW_SYNC_MAX_INTERVAL);
        }
        Err(last_failure.map_or_else(
            || temporary_sync_failure("GitHub review synchronization batch exhausted").into(),
            DeliveryStop::from,
        ))
    }

    async fn review_sync_progress(
        &self,
        state: &mut ReviewSyncState<'_, '_>,
    ) -> Result<ReviewSyncProgress, DeliveryStop> {
        let mut progress = self.review_sync_attempt(state.invocation()).await?;
        if state.should_refresh_credential(&progress) {
            progress = match state.refresh_credential().await? {
                CredentialRefreshProgress::Complete => {
                    self.review_sync_attempt(state.invocation()).await?
                }
                CredentialRefreshProgress::TimedOut | CredentialRefreshProgress::Unavailable => {
                    ReviewSyncProgress::Failed(temporary_sync_failure(
                        "GitHub credential refresh remained unavailable through the synchronization deadline",
                    ))
                }
            };
        }
        Ok(progress)
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
            Err(_) => ReviewSyncProgress::Failed(temporary_sync_failure(
                "GitHub review synchronization request timed out",
            )),
        })
    }
}

fn temporary_sync_failure(message: &str) -> GitHubAuthorityError {
    GitHubAuthorityError::Unavailable.with_context(message)
}

async fn refresh_within_deadline<F>(
    refresh: F,
    mut cancellation: DriverCancellation,
    remaining: Duration,
) -> Result<CredentialRefreshProgress, DeliveryStop>
where
    F: Future<Output = Result<(), EnvironmentRefreshError>>,
{
    if remaining.is_zero() {
        return Ok(CredentialRefreshProgress::TimedOut);
    }
    let result = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(NodeRunnerError::Cancelled.into()),
        result = tokio::time::timeout(remaining, refresh) => result,
    };
    match result {
        Ok(Ok(())) => Ok(CredentialRefreshProgress::Complete),
        Ok(Err(EnvironmentRefreshError::Unavailable)) => Ok(CredentialRefreshProgress::Unavailable),
        Ok(Err(error)) => Err(refresh_failure(error)),
        Err(_) => Ok(CredentialRefreshProgress::TimedOut),
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

#[cfg(test)]
#[path = "sync/tests.rs"]
mod tests;

#[cfg(test)]
mod outages;
