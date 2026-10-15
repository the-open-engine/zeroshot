//! Reconcile an unpublished candidate with the captured target before attempting its first push.
use super::*;

impl NativeV2DeliveryAdapter {
    pub(super) fn record_review_base(&self, revision: Option<String>) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .review_base_revision = revision;
    }

    pub(super) fn forget_review_base_if_head_changed(&self, before: &str, after: &str) {
        if before != after {
            self.record_review_base(None);
        }
    }

    pub(super) async fn reconcile_target_before_publication(
        &self,
        preparation: &mut DeliveryPreparation<'_, '_>,
        commit_message: &str,
    ) -> Result<(), DeliveryStop> {
        let before = self
            .git
            .workspace_state(&self.config.workspace)
            .await
            .map_err(recovery::git_failure)?;
        let mut retry = preflight::OperationRetry::default();
        loop {
            ensure_active(preparation.control)?;
            let request = SourceTargetReconciliation {
                workspace: &self.config.workspace,
                target: &self.config.target,
                commit_message,
            };
            match self
                .authority
                .reconcile_delivery_target(request, preparation.credentials.current())
                .await
            {
                Ok(integration) => {
                    return self
                        .target_integration_result(integration, &before, preparation)
                        .await;
                }
                Err(error) => {
                    self.record_review_base(None);
                    self.retry_operation(
                        error,
                        preparation
                            .credentials
                            .operation_context(preparation.control, &mut retry),
                    )
                    .await?
                }
            }
        }
    }

    async fn target_integration_result(
        &self,
        integration: SourceTargetIntegration,
        before: &(String, bool),
        preparation: &DeliveryPreparation<'_, '_>,
    ) -> Result<(), DeliveryStop> {
        if !valid_revision(&integration.target_revision) {
            return Err(DeliveryStop::Outcome(WorkerOutcome::malformed()));
        }
        let outcome = self
            .reconciliation_result(integration.outcome, before)
            .await?;
        if let SourceReconciliationOutcome::Refused(diagnostic) = outcome {
            return self
                .refuse_delivery(
                    preparation.control,
                    &self.redact_feedback(preparation.credentials, &diagnostic),
                )
                .await;
        }
        let previous = self.delivery_state().review_base_revision;
        let baseline_changed = previous
            .as_deref()
            .unwrap_or(&self.config.target.base_revision)
            != integration.target_revision;
        self.record_review_base(Some(integration.target_revision));
        match outcome {
            SourceReconciliationOutcome::Unchanged if !baseline_changed => Ok(()),
            SourceReconciliationOutcome::NeedsWork(diagnostic) => Err(recovery::repair(diagnostic)),
            _ => Err(recovery::repair(
                "the target revision advanced; verify the candidate against reviewBaseRevision \
                 before publication, preserving upstream changes and the requested feature",
            )),
        }
    }
}
