use super::*;

const HEAD_SYNC_ATTEMPTS: usize = 5;

#[derive(Clone)]
pub(super) struct PendingHead {
    previous: GitHubReviewReceipt,
    updated: GitHubReviewReceipt,
}

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
                self.complete_conflict(
                    drive,
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
        *self
            .pending_head
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(PendingHead {
            previous,
            updated: drive.review.clone(),
        });
        self.resume_pending_head(
            &mut drive.credentials,
            drive.control,
            &drive.review.head_branch,
        )
        .await?;
        emit(drive.control, "delivery: adopted updated pull request head").await?;
        Ok(ReviewStep::Continue)
    }

    pub(super) async fn resume_pending_head(
        &self,
        credentials: &mut DeliveryCredentials<'_>,
        control: &DriverControl,
        head_branch: &str,
    ) -> Result<(), DeliveryStop> {
        let pending = self
            .pending_head
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(pending) = pending else {
            return Ok(());
        };
        if pending.updated.head_branch != head_branch {
            return Err(DeliveryStop::Outcome(WorkerOutcome::malformed()));
        }
        self.synchronize_pending_head(&pending, credentials, control)
            .await
            .map_err(|stop| stop.with_review(&pending.updated))?;
        *self
            .pending_head
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        Ok(())
    }

    async fn synchronize_pending_head(
        &self,
        pending: &PendingHead,
        credentials: &DeliveryCredentials<'_>,
        control: &DriverControl,
    ) -> Result<(), DeliveryStop> {
        for attempt in 0..HEAD_SYNC_ATTEMPTS {
            ensure_active(control)?;
            match self
                .authority
                .synchronize_review_head(
                    GitHubHeadSynchronization {
                        workspace: &self.config.workspace,
                        previous: &pending.previous,
                        updated: &pending.updated,
                    },
                    credentials.current(),
                )
                .await
            {
                Ok(()) => return Ok(()),
                Err(GitHubAuthorityError::Unavailable) if attempt + 1 < HEAD_SYNC_ATTEMPTS => {
                    emit(
                        control,
                        "delivery: waiting to adopt GitHub pull request head",
                    )
                    .await?;
                    wait_for_poll(control, self.config.poll.interval).await?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(recovery::repair(
            "authorized GitHub head could not be adopted",
        ))
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
            Err(error) => {
                drive
                    .credentials
                    .refresh_after(error, drive.control)
                    .await?;
                self.authority
                    .update_review_head(
                        &self.config.workspace,
                        &drive.review,
                        drive.credentials.current(),
                    )
                    .await
                    .map_err(DeliveryStop::from)?
            }
        };
        Ok(outcome)
    }
}
