use super::*;

const HEAD_SYNC_ATTEMPTS: usize = 5;

impl NativeV2DeliveryAdapter {
    pub(super) async fn advance_review_head(
        &self,
        drive: &mut ReviewDrive<'_>,
    ) -> Result<ReviewStep, DeliveryStop> {
        emit(drive.control, "delivery: updating pull request branch").await?;
        let outcome = self.request_head_update(drive).await?;
        match outcome {
            GitHubHeadUpdateOutcome::Updated(updated) => {
                self.adopt_updated_head(drive, updated).await
            }
            GitHubHeadUpdateOutcome::Pending => {
                emit(
                    drive.control,
                    "delivery: pull request update is not yet available",
                )
                .await?;
                Ok(ReviewStep::Continue)
            }
            GitHubHeadUpdateOutcome::Conflict => {
                review_completion(
                    drive,
                    DELIVERY_CONFLICT_LABEL,
                    "GitHub authoritatively rejected branch update due to conflict",
                )
                .await
            }
        }
    }

    async fn adopt_updated_head(
        &self,
        drive: &mut ReviewDrive<'_>,
        updated: GitHubReviewReceipt,
    ) -> Result<ReviewStep, DeliveryStop> {
        if !valid_head_update(&drive.review, &updated) {
            return Err(DeliveryStop::Outcome(WorkerOutcome::malformed()));
        }
        let previous = std::mem::replace(&mut drive.review, updated);
        emit(
            drive.control,
            "delivery: GitHub authorized updated pull request head",
        )
        .await?;
        self.synchronize_updated_head(drive, &previous).await?;
        emit(drive.control, "delivery: adopted updated pull request head").await?;
        Ok(ReviewStep::Continue)
    }

    async fn synchronize_updated_head(
        &self,
        drive: &mut ReviewDrive<'_>,
        previous: &GitHubReviewReceipt,
    ) -> Result<(), DeliveryStop> {
        for attempt in 0..HEAD_SYNC_ATTEMPTS {
            ensure_active(drive.control)?;
            match self
                .authority
                .synchronize_review_head(
                    GitHubHeadSynchronization {
                        workspace: &self.config.workspace,
                        previous,
                        updated: &drive.review,
                    },
                    drive.credentials.current(),
                )
                .await
            {
                Ok(()) => return Ok(()),
                Err(GitHubAuthorityError::Unavailable) if attempt + 1 < HEAD_SYNC_ATTEMPTS => {
                    emit(
                        drive.control,
                        "delivery: waiting to adopt GitHub pull request head",
                    )
                    .await?;
                    drive.credentials.refresh().await?;
                    wait_for_poll(drive.control, self.config.poll.interval).await?;
                }
                Err(GitHubAuthorityError::Unavailable | GitHubAuthorityError::Rejected) => {
                    return Err(crash_outcome());
                }
            }
        }
        Err(crash_outcome())
    }

    async fn request_head_update(
        &self,
        drive: &mut ReviewDrive<'_>,
    ) -> Result<GitHubHeadUpdateOutcome, DeliveryStop> {
        let outcome = match self
            .authority
            .update_review_head(
                &self.config.workspace,
                &drive.review,
                drive.credentials.current(),
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(_) => {
                emit(drive.control, "delivery: refreshing GitHub credential").await?;
                drive.credentials.refresh().await?;
                self.authority
                    .update_review_head(
                        &self.config.workspace,
                        &drive.review,
                        drive.credentials.current(),
                    )
                    .await
                    .map_err(|_| crash_outcome())?
            }
        };
        Ok(outcome)
    }
}
