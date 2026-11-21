//! Retained-workspace recovery eligibility and successor preparation.

use super::*;

impl NativeV2PlatformController {
    pub async fn resume(
        &self,
        params: RunResumeParams,
    ) -> Result<RunResumeResult, NativeV2PlatformError> {
        let _turn = self.submission_turn.lock().await;
        let RunResumeParams {
            from,
            run_id,
            successor_run_id,
            connections,
            connection_resolver,
            github_token,
            git_credentials,
            commit_identity,
            commit_co_author,
        } = params;
        if matches!(
            from,
            Some(
                openengine_cluster_protocol::RunResumeFrom::Answer { .. }
                    | openengine_cluster_protocol::RunResumeFrom::Expiry {}
            )
        ) && !self.allocator.supports_request_ask()
        {
            return Err(NativeV2PlatformError::InvalidRequest(
                "host does not enable openengine.request-ask/v1".to_owned(),
            ));
        }
        // Refuse a live source before taking its control turn: cancellation must not wait for
        // a resume's ledger reads. Recheck eligibility once ownership is serialized with force.
        self.recoverable_resume_source(&run_id).await?;
        let _control = self.lock_run_control(&run_id).await;
        let source = self.recoverable_resume_source(&run_id).await?;
        let request_resolution = if let Some(suspension) = &source.snapshot.suspension {
            if matches!(
                suspension,
                openengine_cluster_protocol::RunSuspension::Request { .. }
            ) || matches!(
                from,
                Some(
                    openengine_cluster_protocol::RunResumeFrom::Answer { .. }
                        | openengine_cluster_protocol::RunResumeFrom::Expiry {}
                )
            ) {
                Some(
                    crate::native_v2_request::resolve(
                        suspension,
                        from.as_ref(),
                        (self
                            .allocator
                            .suspension_policy()
                            .unwrap_or_default()
                            .wall_clock)(),
                    )
                    .map_err(|message| {
                        NativeV2PlatformError::InvalidRequest(
                            message.replace("RUN_ID", run_id.as_str()),
                        )
                    })?,
                )
            } else {
                None
            }
        } else if matches!(
            from,
            Some(
                openengine_cluster_protocol::RunResumeFrom::Answer { .. }
                    | openengine_cluster_protocol::RunResumeFrom::Expiry {}
            )
        ) {
            return Err(NativeV2PlatformError::InvalidRequest(
                "answer requires a suspended run".to_owned(),
            ));
        } else {
            None
        };
        if let Some(resolution) = &request_resolution {
            crate::native_v2_request::validate_resolution(
                &source.snapshot,
                &successor_run_id,
                resolution,
            )
            .map_err(NativeV2PlatformError::InvalidRequest)?;
        }
        let expired = matches!(
            request_resolution,
            Some(openengine_cluster_protocol::RequestResolution::Expiry {})
        );
        if let Some(suspension) = &source.snapshot.suspension {
            // A settled attempt must release its owner before the successor can claim storage.
            self.wait_runtime_settled(&run_id).await;
            self.runtimes.lock().await.remove(&run_id);
            if from.is_some() && request_resolution.is_none() {
                return Err(NativeV2PlatformError::InvalidRequest(
                    "suspended runs accept only default resume".to_owned(),
                ));
            }
            if let Some(due) = suspension.due_at()
                && (self
                    .allocator
                    .suspension_policy()
                    .unwrap_or_default()
                    .wall_clock)()
                    < due
            {
                return Err(NativeV2PlatformError::InvalidRequest(format!(
                    "wait is due at {}",
                    due.get()
                )));
            }
        }
        // Reconcile before claiming/moving the workspace or creating a successor. A hard exit
        // may leave acknowledged ledger receipts newer than the last workspace boundary.
        if let Some(state) = source.snapshot.delivery_checkpoint()? {
            self.allocator
                .retain_delivery_state(&run_id, &state)
                .await?;
        }
        let (submission_key, submission_digest) =
            retained_submission_identity(&run_id, &successor_run_id)?;
        let mut admitted = source.admitted.clone();
        if let Some(catalog) = self.run_sizes.as_deref() {
            crate::native_v2_admission::resolve_runtime_size(&mut admitted.runtime, catalog)?;
        }
        self.validate_harnesses(&admitted.runtime)?;
        let secrets = resume_secret_envelope(ResumeSecretRequest {
            admitted: &admitted,
            successor_run_id: &successor_run_id,
            connections,
            connection_resolver,
            github_token,
            git_credentials,
            commit_identity,
            commit_co_author,
        })?;
        let controller_claim = self.allocator.claim_controller(&successor_run_id).await?;
        let stored = self
            .create_resume_successor(CreateRun {
                run_id: successor_run_id.clone(),
                submission_key,
                submission_digest,
                admitted: admitted.clone(),
            })
            .await?;
        self.start_preparation(PreparationStart {
            stored,
            secrets,
            controller_claim,
            resume: Some(ResumePreparation {
                source_run_id: run_id.clone(),
                request_resolution,
                selection: if source.snapshot.phase == crate::v2_run_ledger::RunPhase::Suspended {
                    crate::native_v2_supervisor::checkpoints::CheckpointRestoreSelection::Continue
                } else {
                    crate::native_v2_supervisor::checkpoints::restore_selection(from.as_ref())
                },
            }),
        })
        .await?;
        if expired {
            return Err(NativeV2PlatformError::InvalidRequest(format!(
                "request expired; successor {}",
                successor_run_id.as_str()
            )));
        }
        Ok(RunResumeResult {
            run_id: successor_run_id,
            resumed_from: run_id,
        })
    }

    async fn recoverable_resume_source(
        &self,
        run_id: &RunId,
    ) -> Result<StoredRun, NativeV2PlatformError> {
        let source = self
            .ledger
            .get(run_id)
            .await?
            .ok_or(RunLedgerError::RunNotFound)?;
        let failed = matches!(
            source.snapshot.terminal,
            Some(TerminalResult::Failed { .. })
        );
        let recovery = self.allocator.workspace_recovery(run_id).await;
        if let Some(successor) = &recovery.successor_run_id {
            return Err(NativeV2PlatformError::InvalidRequest(format!(
                "retained workspace was already claimed by {}",
                successor.as_str()
            )));
        }
        if (failed && source.snapshot.suspension.is_some())
            || !(failed || source.snapshot.phase == crate::v2_run_ledger::RunPhase::Suspended)
            || !recovery.recoverable
        {
            return Err(CapsuleAllocationUnavailable::Runtime.into());
        }
        if self.hosted_workspace_checkpoint_opt_out.is_some()
            && !source.admitted.runtime.workspace_checkpoints
        {
            return Err(NativeV2PlatformError::InvalidRequest(
                "workspace recovery is unavailable because workspaceCheckpoints is false"
                    .to_owned(),
            ));
        }
        Ok(source)
    }

    async fn create_resume_successor(
        &self,
        request: CreateRun,
    ) -> Result<StoredRun, NativeV2PlatformError> {
        let created = self.ledger.create_or_get(request).await?;
        match created {
            CreateRunOutcome::Created(stored) => Ok(stored),
            CreateRunOutcome::Existing(_) => Err(RunLedgerError::RunIdConflict.into()),
        }
    }
}
