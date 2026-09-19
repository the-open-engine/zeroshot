use super::*;

#[derive(Default)]
pub(super) struct OperationRetry {
    pub(super) attempts: usize,
    pub(super) refreshed: bool,
}

pub(super) struct OperationContext<'a, 'environment> {
    pub(super) credentials: &'a mut DeliveryCredentials<'environment>,
    pub(super) control: &'a DriverControl,
    pub(super) retry: &'a mut OperationRetry,
}

struct ObservedPreflight<'a, 'invocation, 'environment> {
    known: &'a DeliveryState,
    observed: GitHubReviewReceipt,
    preparation: &'a mut DeliveryPreparation<'invocation, 'environment>,
    commit_message: &'a str,
}

impl<'environment> DeliveryPreparation<'_, 'environment> {
    pub(super) fn operation_context<'a>(
        &'a mut self,
        retry: &'a mut OperationRetry,
    ) -> OperationContext<'a, 'environment> {
        OperationContext {
            credentials: self.credentials,
            control: self.control,
            retry,
        }
    }
}

impl<'environment> ReviewDrive<'environment> {
    pub(super) fn operation_context<'a>(
        &'a mut self,
        retry: &'a mut OperationRetry,
    ) -> OperationContext<'a, 'environment> {
        OperationContext {
            credentials: &mut self.credentials,
            control: self.control,
            retry,
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct DeliveryState {
    pub(super) published: Option<GitHubReviewReceipt>,
    pub(super) intended_push: Option<String>,
    pub(super) pending_head: Option<head::PendingHead>,
    pub(super) review_base_revision: Option<String>,
    pub(super) feedback_versions: BTreeMap<String, String>,
}

impl NativeV2DeliveryAdapter {
    pub(super) fn delivery_state(&self) -> DeliveryState {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(super) fn record_published(&self, review: GitHubReviewReceipt) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.published = Some(review);
        state.intended_push = None;
        state.pending_head = None;
    }

    pub(super) fn checkpoint_feedback(
        &self,
        feedback: GitHubReviewFeedback,
    ) -> Vec<GitHubReviewFeedbackItem> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut unseen = Vec::new();
        for item in feedback.items {
            let changed = state
                .feedback_versions
                .get(&item.key)
                .is_none_or(|version| version != &item.version);
            state
                .feedback_versions
                .insert(item.key.clone(), item.version.clone());
            if changed {
                unseen.push(item);
            }
        }
        unseen
    }

    pub(super) async fn preflight(
        &self,
        preparation: &mut DeliveryPreparation<'_, '_>,
        commit_message: &str,
    ) -> Result<(), DeliveryStop> {
        let branch = delivery_branch(self.config.delivery_run_id.as_str());
        let known = self.delivery_state();
        let mode = DeliveryMode::from_worker(&preparation.invocation.node.worker)
            .ok_or(NodeRunnerError::InvalidRole)?;
        let request = GitHubDeliveryRead {
            target: &self.config.target,
            head_branch: &branch,
            known_review: known.published.as_ref(),
            include_review: mode.creates_review(),
        };
        let snapshot = self.observe_before_delivery(request, preparation).await?;
        self.check_terminal_review(&snapshot, preparation).await?;
        let Some(observed) = observed_receipt(snapshot, &self.config.target, &branch) else {
            return self
                .unpublished_preflight(&known, preparation, commit_message)
                .await;
        };
        self.reconcile_observed(ObservedPreflight {
            known: &known,
            observed,
            preparation,
            commit_message,
        })
        .await
    }

    async fn unpublished_preflight(
        &self,
        known: &DeliveryState,
        preparation: &mut DeliveryPreparation<'_, '_>,
        commit_message: &str,
    ) -> Result<(), DeliveryStop> {
        if known.published.is_some() {
            return self
                .refuse_delivery(
                    preparation.control,
                    "the published run branch was deleted; work was preserved",
                )
                .await;
        }
        if DeliveryMode::from_worker(&preparation.invocation.node.worker)
            == Some(DeliveryMode::Push)
        {
            return Ok(());
        }
        self.reconcile_target_before_publication(preparation, commit_message)
            .await
    }

    async fn reconcile_observed(
        &self,
        context: ObservedPreflight<'_, '_, '_>,
    ) -> Result<(), DeliveryStop> {
        let ObservedPreflight {
            known,
            observed,
            preparation,
            commit_message,
        } = context;
        let published = self
            .require_reconciliation_anchor(known, &observed, preparation.control)
            .await?;
        let authorized_update = known.pending_head.as_ref().is_some_and(|pending| {
            pending
                .updated
                .as_ref()
                .is_some_and(|updated| updated.head_revision == observed.head_revision)
        });
        let adopting_existing = self.is_adopting_existing_delivery(known);
        let request = GitHubHeadReconciliation {
            workspace: &self.config.workspace,
            published: &published,
            observed: &observed,
            commit_message,
            authorized_update,
            adopting_existing,
        };
        self.forget_review_base_if_head_changed(&published.head_revision, &observed.head_revision);
        let outcome = self.reconcile_before_delivery(request, preparation).await?;
        match outcome {
            GitHubReconciliationOutcome::Refused(diagnostic) => {
                self.refuse_delivery(preparation.control, &diagnostic).await
            }
            GitHubReconciliationOutcome::NeedsWork(diagnostic) => {
                self.record_published(observed.clone());
                Err(recovery::repair(diagnostic).with_review(&observed))
            }
            GitHubReconciliationOutcome::Unchanged | GitHubReconciliationOutcome::Adopted => {
                self.record_published(observed);
                Ok(())
            }
        }
    }

    fn is_adopting_existing_delivery(&self, known: &DeliveryState) -> bool {
        self.config.adopt_existing_delivery
            && known.published.is_none()
            && known.intended_push.is_none()
    }

    async fn require_reconciliation_anchor(
        &self,
        known: &DeliveryState,
        observed: &GitHubReviewReceipt,
        control: &DriverControl,
    ) -> Result<GitHubReviewReceipt, DeliveryStop> {
        match reconciliation_anchor(known, observed, self.config.adopt_existing_delivery) {
            Ok(published) => Ok(published),
            Err(error) => {
                emit(control, &format!("delivery: {error}")).await?;
                Err(error.into())
            }
        }
    }

    async fn observe_before_delivery(
        &self,
        request: GitHubDeliveryRead<'_>,
        preparation: &mut DeliveryPreparation<'_, '_>,
    ) -> Result<GitHubDeliverySnapshot, DeliveryStop> {
        let mut refreshed = OperationRetry::default();
        loop {
            ensure_active(preparation.control)?;
            match self
                .authority
                .observe_delivery(request, preparation.credentials.current())
                .await
            {
                Ok(snapshot) => return Ok(snapshot),
                Err(error) => {
                    self.retry_operation(error, preparation.operation_context(&mut refreshed))
                        .await?
                }
            }
        }
    }

    async fn reconcile_before_delivery(
        &self,
        request: GitHubHeadReconciliation<'_>,
        preparation: &mut DeliveryPreparation<'_, '_>,
    ) -> Result<GitHubReconciliationOutcome, DeliveryStop> {
        let before = self
            .git
            .workspace_state(&self.config.workspace)
            .await
            .map_err(|error| recovery::repair(error.to_string()))?;
        let mut refreshed = OperationRetry::default();
        loop {
            ensure_active(preparation.control)?;
            match self
                .authority
                .reconcile_delivery_head(request, preparation.credentials.current())
                .await
            {
                Ok(outcome) => return self.reconciliation_result(outcome, &before).await,
                Err(error) => {
                    self.retry_operation(error, preparation.operation_context(&mut refreshed))
                        .await?
                }
            }
        }
    }

    pub(super) async fn reconciliation_result(
        &self,
        outcome: GitHubReconciliationOutcome,
        before: &(String, bool),
    ) -> Result<GitHubReconciliationOutcome, DeliveryStop> {
        if outcome != GitHubReconciliationOutcome::Unchanged {
            return Ok(outcome);
        }
        let after = self
            .git
            .workspace_state(&self.config.workspace)
            .await
            .map_err(|error| recovery::repair(error.to_string()))?;
        if before != &after {
            return Ok(GitHubReconciliationOutcome::NeedsWork(format!(
                "the workspace changed during trusted reconciliation before a retry confirmed it: \
                 before head {} dirty {}; after head {} dirty {}; inspect the current workspace",
                before.0, before.1, after.0, after.1,
            )));
        }
        Ok(outcome)
    }

    async fn check_terminal_review(
        &self,
        snapshot: &GitHubDeliverySnapshot,
        preparation: &DeliveryPreparation<'_, '_>,
    ) -> Result<(), DeliveryStop> {
        let Some(review) = &snapshot.review else {
            return Ok(());
        };
        match &review.state {
            GitHubReviewState::Closed => {
                self.refuse_delivery(
                    preparation.control,
                    "the run PR was closed; no branch or PR mutation was attempted",
                )
                .await
            }
            GitHubReviewState::Merged { .. } => {
                self.complete_existing_merge(review, preparation).await
            }
            _ => Ok(()),
        }
    }

    async fn complete_existing_merge(
        &self,
        review: &GitHubReviewObservation,
        preparation: &DeliveryPreparation<'_, '_>,
    ) -> Result<(), DeliveryStop> {
        let GitHubReviewState::Merged { merge_revision } = &review.state else {
            return Err(DeliveryStop::Outcome(WorkerOutcome::malformed()));
        };
        let (head, dirty) = self
            .git
            .workspace_state(&self.config.workspace)
            .await
            .map_err(|error| recovery::repair(error.to_string()))?;
        let mode = DeliveryMode::from_worker(&preparation.invocation.node.worker)
            .ok_or(NodeRunnerError::InvalidRole)?;
        if dirty || head != review.head_revision || !mode.is_merge() {
            return self.refuse_delivery(preparation.control,
                "the run PR is already merged, but current local work is not confirmed delivered; work was preserved")
                .await;
        }
        if let Err(error) = self
            .git
            .deliverable_revision(&self.config.workspace, &self.config.target.base_revision)
            .await
        {
            return self
                .refuse_delivery(
                    preparation.control,
                    &format!(
                        "the merged PR does not prove delivery of the admitted candidate: {error}",
                    ),
                )
                .await;
        }
        let receipt = receipt_from_observation(review);
        let diagnostic = "GitHub confirmed the previously delivered candidate is already merged";
        emit(preparation.control, diagnostic).await?;
        let outcome = delivery_outcome(
            DeliveryResult {
                mode,
                outcome: DELIVERY_MERGED_LABEL,
                review: &receipt,
                merge_revision: Some(merge_revision),
            },
            diagnostic,
        )?;
        Err(DeliveryStop::Outcome(outcome))
    }

    pub(super) async fn refuse_delivery(
        &self,
        control: &DriverControl,
        diagnostic: &str,
    ) -> Result<(), DeliveryStop> {
        emit(control, diagnostic).await?;
        Err(DeliveryStop::Outcome(WorkerOutcome::declared_failure(
            WorkerErrorCode::Refusal,
        )))
    }

    fn retry_interval(&self, attempt: usize) -> Duration {
        self.config
            .poll
            .interval
            .saturating_mul(1 << attempt.saturating_sub(1).min(3))
            .min(Duration::from_secs(60))
    }

    async fn refresh_operation(
        &self,
        context: OperationContext<'_, '_>,
    ) -> Result<(), DeliveryStop> {
        let OperationContext {
            credentials,
            control,
            retry,
        } = context;
        match credentials.try_refresh().await {
            Ok(()) => {
                retry.refreshed = true;
                Ok(())
            }
            Err(crate::native_v2_runner::EnvironmentRefreshError::Unavailable) => {
                emit(
                    control,
                    "delivery: credential refresh is temporarily unavailable; retrying",
                )
                .await?;
                wait_for_poll(control, self.retry_interval(retry.attempts))
                    .await
                    .map_err(Into::into)
            }
            Err(error) => Err(refresh_failure(error)),
        }
    }

    pub(super) async fn operation_result<T>(
        &self,
        result: Result<T, GitHubAuthorityError>,
        context: OperationContext<'_, '_>,
    ) -> Result<Option<T>, DeliveryStop> {
        match result {
            Ok(value) => Ok(Some(value)),
            Err(error) => {
                self.retry_operation(error, context).await?;
                Ok(None)
            }
        }
    }

    pub(super) async fn retry_operation(
        &self,
        error: GitHubAuthorityError,
        context: OperationContext<'_, '_>,
    ) -> Result<(), DeliveryStop> {
        let OperationContext {
            credentials,
            control,
            retry,
        } = context;
        emit(control, &format!("delivery: {error}")).await?;
        let can_refresh =
            error.authentication_failed() && credentials.can_refresh() && !retry.refreshed;
        if !can_refresh && !error.retryable_operation() {
            return Err(error.into());
        }
        retry.attempts = retry.attempts.saturating_add(1);
        if !self.config.poll.has_next(retry.attempts) {
            return Err(DeliveryStop::Outcome(WorkerOutcome::declared_failure(
                WorkerErrorCode::Timeout,
            )));
        }
        if can_refresh {
            return self
                .refresh_operation(OperationContext {
                    credentials,
                    control,
                    retry,
                })
                .await;
        }
        if error.retryable_operation() {
            emit(control, "delivery: retrying the trusted GitHub operation").await?;
            return wait_for_poll(control, self.retry_interval(retry.attempts))
                .await
                .map_err(Into::into);
        }
        Err(error.into())
    }
}

fn reconciliation_anchor(
    known: &DeliveryState,
    observed: &GitHubReviewReceipt,
    adopt_existing_delivery: bool,
) -> Result<GitHubReviewReceipt, GitHubAuthorityError> {
    if let Some(published) = &known.published {
        return Ok(published.clone());
    }
    if let Some(intended) = &known.intended_push {
        let mut anchor = observed.clone();
        anchor.head_revision.clone_from(intended);
        return Ok(anchor);
    }
    if adopt_existing_delivery {
        return Ok(observed.clone());
    }
    Err(GitHubAuthorityError::identity(format!(
        "unexpected existing run branch at {}; no published candidate or pending push proves ownership",
        observed.head_revision,
    )))
}

fn observed_receipt(
    snapshot: GitHubDeliverySnapshot,
    target: &DeliveryTarget,
    branch: &str,
) -> Option<GitHubReviewReceipt> {
    let head_revision = snapshot.head_revision?;
    match snapshot.review {
        Some(review) => Some(receipt_from_observation(&review)),
        None => Some(GitHubReviewReceipt {
            review_id: String::new(),
            repository: target.repository.clone(),
            target_branch: target.target_branch.clone(),
            head_branch: branch.to_owned(),
            head_revision,
        }),
    }
}

pub(super) fn receipt_from_observation(review: &GitHubReviewObservation) -> GitHubReviewReceipt {
    GitHubReviewReceipt {
        review_id: review.review_id.clone(),
        repository: review.repository.clone(),
        target_branch: review.target_branch.clone(),
        head_branch: review.head_branch.clone(),
        head_revision: review.head_revision.clone(),
    }
}
