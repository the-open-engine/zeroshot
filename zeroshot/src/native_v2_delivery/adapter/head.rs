use super::*;

#[derive(Clone)]
pub(super) struct PendingHead {
    pub(super) previous: GitHubReviewReceipt,
    pub(super) updated: Option<GitHubReviewReceipt>,
}

struct RecoveredHead {
    observed: GitHubReviewReceipt,
    failure: recovery::RepairFailure,
    outcome: GitHubReconciliationOutcome,
}

impl NativeV2DeliveryAdapter {
    pub(super) async fn advance_review_head(
        &self,
        drive: &mut ReviewDrive<'_>,
    ) -> Result<ReviewStep, DeliveryStop> {
        emit(drive.control, "delivery: updating pull request branch").await?;
        let outcome = match self.request_head_update(drive).await {
            Ok(outcome) => outcome,
            Err(DeliveryStop::Repair(failure)) => {
                return self.recover_head_update(drive, failure).await;
            }
            Err(stop) => return Err(stop),
        };
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
        self.record_review_base(None);
        let previous = std::mem::replace(&mut drive.review, updated);
        // Record before an awaited log/output operation can fail or cancellation can intervene.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_head = Some(PendingHead {
            previous,
            updated: Some(drive.review.clone()),
        });
        emit(
            drive.control,
            "delivery: GitHub authorized updated pull request head",
        )
        .await?;
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
        let Some(pending) = self.delivery_state().pending_head else {
            return Ok(());
        };
        let Some(updated) = &pending.updated else {
            return Ok(());
        };
        if updated.head_branch != head_branch {
            return Err(
                identity_failure(control, "pending update belongs to a different branch").await,
            );
        }
        let mut refreshed = preflight::OperationRetry::default();
        loop {
            ensure_active(control)?;
            let request = GitHubHeadSynchronization {
                workspace: &self.config.workspace,
                previous: &pending.previous,
                updated,
            };
            match self
                .authority
                .synchronize_review_head(request, credentials.current())
                .await
            {
                Ok(()) => {
                    self.record_published(updated.clone());
                    return Ok(());
                }
                Err(error) => self
                    .retry_operation(
                        error,
                        preflight::OperationContext {
                            credentials,
                            control,
                            retry: &mut refreshed,
                        },
                    )
                    .await
                    .map_err(|stop| stop.with_review(updated))?,
            }
        }
    }

    async fn request_head_update(
        &self,
        drive: &mut ReviewDrive<'_>,
    ) -> Result<GitHubHeadUpdateOutcome, DeliveryStop> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_head = Some(PendingHead {
            previous: drive.review.clone(),
            updated: None,
        });
        let mut refreshed = preflight::OperationRetry::default();
        loop {
            ensure_active(drive.control)?;
            match self
                .authority
                .update_review_head(
                    &self.config.workspace,
                    &drive.review,
                    drive.credentials.current(),
                )
                .await
            {
                Ok(outcome) => return Ok(outcome),
                // Observe an ambiguous mutation before issuing it again, even for a transport error.
                Err(error) if !error.authentication_failed() => return Err(error.into()),
                Err(error) => {
                    self.retry_operation(error, drive.operation_context(&mut refreshed))
                        .await?
                }
            }
        }
    }

    async fn observe_updated_head(
        &self,
        drive: &mut ReviewDrive<'_>,
    ) -> Result<GitHubDeliverySnapshot, DeliveryStop> {
        let mut retry = preflight::OperationRetry::default();
        loop {
            ensure_active(drive.control)?;
            let request = GitHubDeliveryRead {
                target: &self.config.target,
                head_branch: &drive.review.head_branch,
                known_review: Some(&drive.review),
            };
            match self
                .authority
                .observe_delivery(request, drive.credentials.current())
                .await
            {
                Ok(snapshot) => return Ok(snapshot),
                Err(error) => {
                    self.retry_operation(error, drive.operation_context(&mut retry))
                        .await?
                }
            }
        }
    }

    async fn reconcile_updated_head(
        &self,
        drive: &mut ReviewDrive<'_>,
        observed: &GitHubReviewReceipt,
    ) -> Result<GitHubReconciliationOutcome, DeliveryStop> {
        let mut retry = preflight::OperationRetry::default();
        loop {
            ensure_active(drive.control)?;
            let request = GitHubHeadReconciliation {
                workspace: &self.config.workspace,
                published: &drive.review,
                observed,
                commit_message: "Preserve work before GitHub head reconciliation",
                authorized_update: false,
                adopting_existing: false,
            };
            let result = self
                .authority
                .reconcile_delivery_head(request, drive.credentials.current())
                .await;
            if let Some(value) = self
                .operation_result(result, drive.operation_context(&mut retry))
                .await?
            {
                return Ok(value);
            }
        }
    }

    pub(super) async fn recover_head_update(
        &self,
        drive: &mut ReviewDrive<'_>,
        failure: recovery::RepairFailure,
    ) -> Result<ReviewStep, DeliveryStop> {
        let snapshot = self.observe_updated_head(drive).await?;
        let Some(review) = snapshot.review else {
            return Err(identity_failure(
                drive.control,
                "the bound run PR disappeared during recovery",
            )
            .await);
        };
        match &review.state {
            GitHubReviewState::Merged { .. }
                if review.head_revision == drive.review.head_revision =>
            {
                return Ok(ReviewStep::Continue);
            }
            GitHubReviewState::Open { .. } if snapshot.head_revision.is_some() => {}
            _ => {
                self.refuse_delivery(
                    drive.control,
                    "the run PR closed, merged with a different head, or its branch disappeared; \
                     local work was preserved",
                )
                .await?;
            }
        }
        let observed = preflight::receipt_from_observation(&review);
        self.forget_review_base_if_head_changed(
            &observed.head_revision,
            &drive.review.head_revision,
        );
        let outcome = self.reconcile_updated_head(drive, &observed).await?;
        let outcome = recovered_outcome(
            outcome,
            observed.head_revision != drive.review.head_revision,
        );
        self.complete_head_recovery(
            drive,
            RecoveredHead {
                observed,
                failure,
                outcome,
            },
        )
        .await
    }

    async fn complete_head_recovery(
        &self,
        drive: &mut ReviewDrive<'_>,
        recovery: RecoveredHead,
    ) -> Result<ReviewStep, DeliveryStop> {
        let RecoveredHead {
            observed,
            failure,
            outcome,
        } = recovery;
        match outcome {
            GitHubReconciliationOutcome::NeedsWork(diagnostic) => {
                self.record_published(observed.clone());
                Err(
                    recovery::repair(format!("{}\n{diagnostic}", failure.diagnostic))
                        .with_review(&observed),
                )
            }
            GitHubReconciliationOutcome::Refused(diagnostic) => {
                self.refuse_delivery(drive.control, &diagnostic).await?;
                unreachable!("refused delivery always stops")
            }
            GitHubReconciliationOutcome::Unchanged if failure.retryable => {
                emit(drive.control, &failure.diagnostic).await?;
                wait_for_poll(drive.control, self.config.poll.interval).await?;
                Ok(ReviewStep::Continue)
            }
            _ => Err(DeliveryStop::Repair(failure)),
        }
    }
}

fn recovered_outcome(
    outcome: GitHubReconciliationOutcome,
    head_changed: bool,
) -> GitHubReconciliationOutcome {
    match outcome {
        GitHubReconciliationOutcome::Adopted => GitHubReconciliationOutcome::NeedsWork(
            "delivery recovered a remote transition without its mutation receipt; \
             inspect the current workspace".to_owned(),
        ),
        GitHubReconciliationOutcome::Unchanged if head_changed => GitHubReconciliationOutcome::NeedsWork(
            "delivery observed a changed remote head already present locally; inspect the current workspace".to_owned(),
        ),
        other => other,
    }
}

async fn identity_failure(control: &DriverControl, diagnostic: &str) -> DeliveryStop {
    let error = GitHubAuthorityError::identity(diagnostic);
    match emit(control, &format!("delivery: {error}")).await {
        Ok(()) => error.into(),
        Err(error) => DeliveryStop::Runner(error),
    }
}
