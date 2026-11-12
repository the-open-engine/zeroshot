use super::*;
use crate::native_v2_supervisor::checkpoints::{
    CheckpointRestore, CheckpointRestoreSelection, remove_restore_stage,
};
use openengine_cluster_protocol::RunResumeFrom;

/// Chooses the snapshot a local successor restores, if any. Plain resume keeps the user-owned
/// checkout as it is. Replacing it needs `--discard-workspace-changes`: to roll back to a selected
/// checkpoint, or to complete an interrupted restore of the latest snapshot, which earlier plain
/// resumes recorded. While an interrupted restore remains, a resume that would not complete a
/// restore is refused, so no agent runs on a partially restored checkout.
fn local_restore_selection(
    run_id: &RunId,
    from: Option<&RunResumeFrom>,
    recovery: &LocalRecoveryDocument,
    discard_workspace_changes: bool,
) -> Result<Option<CheckpointRestoreSelection>, NativeV2CliError> {
    if let Some(RunResumeFrom::Checkpoint { checkpoint_id }) = from {
        if !recovery.submission.runtime.workspace_checkpoints {
            return Err(local_message(
                "workspace checkpoint restore is unavailable because workspaceCheckpoints is false",
            ));
        }
        let selection = CheckpointRestoreSelection::Checkpoint {
            checkpoint_id: checkpoint_id.clone(),
        };
        if !discard_workspace_changes {
            return Err(NativeV2CliError::Usage(format!(
                "restoring checkpoint {} discards this workspace's tracked, staged, untracked, \
                 and ignored files and its index, replacing them with the checkpoint's; attached \
                 HEAD, branches and commits are kept. To roll \
                 back, run `{}`",
                checkpoint_id.as_str(),
                restore_command(run_id, &selection),
            )));
        }
        return Ok(Some(selection));
    }
    match &recovery.pending_restore {
        Some(pending)
            if discard_workspace_changes
                && pending.selection == CheckpointRestoreSelection::Latest =>
        {
            Ok(Some(CheckpointRestoreSelection::Latest))
        }
        Some(pending) => Err(local_message(format!(
            "an interrupted checkpoint restore may have left this workspace partially restored; \
             complete it with `{}`",
            restore_command(run_id, &pending.selection)
        ))),
        None if discard_workspace_changes => Err(NativeV2CliError::Usage(
            "--discard-workspace-changes requires --from-checkpoint; a plain resume keeps the \
             current checkout"
                .to_owned(),
        )),
        None => Ok(None),
    }
}

fn restore_command(run_id: &RunId, selection: &CheckpointRestoreSelection) -> String {
    let selection = match selection {
        CheckpointRestoreSelection::Latest => String::new(),
        CheckpointRestoreSelection::Checkpoint { checkpoint_id } => {
            format!(" --from-checkpoint {}", checkpoint_id.as_str())
        }
    };
    format!(
        "zeroshot resume {}{selection} --discard-workspace-changes",
        run_id.as_str()
    )
}

impl LocalCliBackend {
    async fn resume_local(
        &self,
        params: openengine_cluster_protocol::RunResumeParams,
        discard_workspace_changes: bool,
    ) -> Result<openengine_cluster_protocol::RunResumeResult, NativeV2CliError> {
        if params.commit_identity.is_some() || params.commit_co_author.is_some() {
            return Err(local_message(
                "local runs keep the default delivery commit identity and credit no co-author",
            ));
        }
        // The shared lock serializes the claim. The claim alone then excludes other resumes of
        // this run while its successor is prepared, so a forge request never holds that lock.
        let (recovery, _claim) = {
            let _submission_lock = self.acquire_submission_lock().await?;
            self.reconcile_local_resume_claim(&params.run_id, self.ready_timeout)
                .await?;
            self.confirm_loss_for_resume(&params.run_id).await?;
            let status = self
                .status_local(RunStatusParams {
                    run_id: params.run_id.clone(),
                })
                .await?;
            let recovery = self.claimable_recovery_document(
                &params.run_id,
                status.workspace_recovery.recoverable,
            )?;
            let claim = self.claim_recovery_workspace(&params.run_id)?;
            self.retain_predecessor_delivery(&params.run_id, &recovery)
                .await?;
            (recovery, claim)
        };
        self.start_local_successor(params, recovery, discard_workspace_changes)
            .await
    }

    async fn retain_predecessor_delivery(
        &self,
        run_id: &RunId,
        recovery: &LocalRecoveryDocument,
    ) -> Result<(), NativeV2CliError> {
        use crate::native_v2_supervisor::checkpoints::{DeliveryCheckpointStore, RunCheckpointStore};
        let ledger = SqliteRunLedger::open(self.paths(run_id)?.ledger()).map_err(local_error)?;
        let predecessor = ledger
            .get(run_id)
            .await
            .map_err(local_error)?
            .ok_or_else(|| local_message("predecessor ledger is unavailable"))?;
        if let Some(state) = predecessor
            .snapshot
            .delivery_checkpoint()
            .map_err(local_error)?
        {
            let repository =
                self.checkpoint_repository(&self.delivery_run_id(run_id, recovery)?)?;
            DeliveryCheckpointStore::new(repository)
                .retain_delivery_state(&state)
                .await
                .map_err(local_error)?;
        }
        Ok(())
    }

    fn claimable_recovery_document(
        &self,
        run_id: &RunId,
        recoverable: bool,
    ) -> Result<LocalRecoveryDocument, NativeV2CliError> {
        if !recoverable {
            return Err(local_message("run does not have a recoverable workspace"));
        }
        let recovery = self.read_recovery_document(run_id)?;
        if recovery.successor_run_id.is_some() {
            return Err(local_message("retained workspace was already claimed"));
        }
        Ok(recovery)
    }

    /// A successor is published only from the record that its preparation read under the claim.
    fn require_unchanged_recovery(
        &self,
        run_id: &RunId,
        prepared_from: &LocalRecoveryDocument,
    ) -> Result<(), NativeV2CliError> {
        let current = self.read_recovery_document(run_id)?;
        let unchanged = serde_json::to_value(&current).map_err(local_error)?
            == serde_json::to_value(prepared_from).map_err(local_error)?;
        if unchanged {
            Ok(())
        } else {
            Err(local_message(
                "retained workspace record changed while the resume was prepared; run resume again",
            ))
        }
    }

    /// Selects the restore for a successor and pins the snapshot it installs. After an
    /// interrupted restore, the same selection must still resolve to its pinned snapshot.
    fn checkpoint_selection(
        &self,
        params: &openengine_cluster_protocol::RunResumeParams,
        recovery: &LocalRecoveryDocument,
        discard_workspace_changes: bool,
    ) -> Result<Option<(CheckpointRestore, PendingRestore)>, NativeV2CliError> {
        let Some(selection) = local_restore_selection(
            &params.run_id,
            params.from.as_ref(),
            recovery,
            discard_workspace_changes,
        )?
        else {
            return Ok(None);
        };
        let restore = CheckpointRestore {
            directory: self.paths(&params.run_id)?.storage().join("checkpoints"),
            selection,
        };
        let snapshot =
            crate::native_v2_supervisor::checkpoints::pin(&restore).map_err(local_error)?;
        if recovery.pending_restore.as_ref().is_some_and(|pending| {
            pending.selection == restore.selection && pending.snapshot != snapshot
        }) {
            return Err(local_message(
                "the checkpoint of an interrupted restore no longer resolves to its pinned snapshot",
            ));
        }
        let pending = PendingRestore {
            selection: restore.selection.clone(),
            snapshot,
            attempt: params.successor_run_id.clone(),
        };
        Ok(Some((restore, pending)))
    }

    pub(super) async fn start_local_successor(
        &self,
        params: openengine_cluster_protocol::RunResumeParams,
        mut recovery: LocalRecoveryDocument,
        discard_workspace_changes: bool,
    ) -> Result<openengine_cluster_protocol::RunResumeResult, NativeV2CliError> {
        if recovery.local_branch.is_none()
            && !crate::native_v2_admission::delivery_capabilities(&recovery.submission.graph.root)
                .is_empty()
        {
            return Err(NativeV2CliError::Usage(
                "cannot resume this legacy local Git-delivery run: its recovery metadata does not \
                 record the working branch. Preserve the retained workspace, return to the intended \
                 branch with `git switch BRANCH`, and start a new run from a feature branch or with \
                 --worktree; no checkpoint was restored and no agent was started"
                    .to_owned(),
            ));
        }
        let (checkpoint, restore) = self
            .checkpoint_selection(&params, &recovery, discard_workspace_changes)?
            .unzip();
        let prepared = self.prepare_successor(&params, &recovery).await?;
        if checkpoint.is_none() {
            // Remote I/O runs under the recovery claim only, outside the shared submission lock.
            crate::native_v2_local::check_forge_access(&prepared, &self.state_root)
                .await
                .map_err(|error| NativeV2CliError::Usage(error.to_string()))?;
        }
        // Publication is serialized with other submissions again; recheck what the claim covers.
        let _submission_lock = self.acquire_submission_lock().await?;
        self.require_unchanged_recovery(&params.run_id, &recovery)?;
        if let Some(interrupted) = &recovery.pending_restore {
            // Reconciliation confirmed that the interrupted attempt's controller released its
            // claim, so only its own stage is removed; other names beside the workspace stay.
            remove_restore_stage(&recovery.workspace, &interrupted.attempt).map_err(|error| {
                local_message(format!(
                    "an interrupted checkpoint restore's staging directory could not be removed: {error}"
                ))
            })?;
        }
        recovery.successor_run_id = Some(params.successor_run_id.clone());
        recovery.pending_restore = restore;
        self.write_recovery_document(&params.run_id, &recovery)?;
        if let Err(failure) = self
            .start_prepared_controller_with_lineage(
                prepared,
                Some(params.run_id.clone()),
                checkpoint,
            )
            .await
        {
            let handoff = if failure.controller_stopped {
                Duration::ZERO
            } else {
                self.ready_timeout
            };
            self.reconcile_local_resume_claim(&params.run_id, handoff)
                .await?;
            if failure.controller_stopped && recovery.pending_restore.is_some() {
                // Best effort: the recorded pending restore keeps this stage owned until removed.
                let _ = remove_restore_stage(&recovery.workspace, &params.successor_run_id);
            }
            return Err(failure.error);
        }
        if recovery.pending_restore.take().is_some() {
            // Readiness follows the complete restore and durable admission. If this write fails,
            // the admitted successor still owns the claim and the record is never replayed.
            let _ = self.write_recovery_document(&params.run_id, &recovery);
        }
        let successor = self.read_recovery_document(&params.successor_run_id)?;
        debug_assert_eq!(successor.resumed_from.as_ref(), Some(&params.run_id));
        Ok(openengine_cluster_protocol::RunResumeResult {
            run_id: params.successor_run_id,
            resumed_from: params.run_id,
        })
    }
    async fn prepare_successor(
        &self,
        params: &openengine_cluster_protocol::RunResumeParams,
        recovery: &LocalRecoveryDocument,
    ) -> Result<PreparedLocalRun, NativeV2CliError> {
        crate::native_v2_local::validate_workspace_state(&recovery.workspace, &self.state_root)
            .map_err(|error| NativeV2CliError::Usage(error.to_string()))?;
        let mut native_environment =
            native_forge::capture_native_environment(&self.current_directory)?;
        let mut submission = recovery.submission.clone();
        native_forge::materialize_local_access(
            &mut submission,
            native_forge::NativeContext {
                environment: &native_environment,
                directory: &recovery.workspace,
            },
            params.github_token.is_some(),
        )?;
        let store = LocalConnectionStore::new(self.state_root.clone());
        let mut connections = params.connections.clone();
        native_forge::fill_missing(
            &submission,
            &mut connections,
            &store,
            native_forge::NativeContext {
                environment: &native_environment,
                directory: &recovery.workspace,
            },
        )
        .await?;
        let environment = store.resolve_submission(&submission, &connections)?;
        let git_credentials = environment.resolve_git_source().await?;
        let (github_token, github_token_file) = crate::native_v2_local::take_github_authority(
            &mut native_environment,
            params.github_token.clone(),
            &self.current_directory,
            &recovery.workspace,
        )
        .map_err(local_error)?;
        let delivery_validator = crate::native_v2_local::take_delivery_validator(
            &mut native_environment,
            &recovery.workspace,
        )
        .map_err(local_error)?;
        Ok(PreparedLocalRun {
            local_branch: recovery.local_branch.clone(),
            invoking_workspace: recovery.invoking_workspace.clone(),
            request_digest: recovery.request_digest.clone(),
            workspace_revision: recovery.workspace_revision.clone(),
            run_id: params.successor_run_id.clone(),
            delivery_run_id: self.delivery_run_id(&params.run_id, recovery)?,
            submission,
            environment,
            github_token,
            github_token_file,
            delivery_validator,
            git_credentials,
            workspace: recovery.workspace.clone(),
            native_environment,
        })
    }
}

#[async_trait]
impl NativeV2CliBackend for LocalCliBackend {
    type Watch = ChannelSubscription<CliRunWatchEventNotification>;
    type Logs = ChannelSubscription<RunLogEventNotification>;
    type Attach = ChannelSubscription<RunAttachEventNotification>;

    async fn resolve_review(
        &self,
        target: Option<&str>,
        request: crate::native_v2_review::ReviewResolutionRequest,
        _access: crate::native_v2_review::ReviewResolutionAccess<'_>,
    ) -> Result<crate::native_v2_review::PreparedReview, NativeV2CliError> {
        require_local(target)?;
        let token = super::review_token(&self.current_directory).await?;
        crate::native_v2_review::resolve_review(request, &token)
            .await
            .map_err(|error| NativeV2CliError::Usage(error.to_string()))
    }

    async fn target_add(&self, _request: TargetAdd) -> Result<(), NativeV2CliError> {
        Err(local_message(
            "target commands are not local run operations",
        ))
    }

    async fn target_login(&self, _name: &str) -> Result<(), NativeV2CliError> {
        Err(local_message(
            "target commands are not local run operations",
        ))
    }

    async fn connection_list(
        &self,
        target: Option<&str>,
        request: ConnectionListRequest,
    ) -> Result<ConnectionListResult, NativeV2CliError> {
        require_local(target)?;
        LocalConnectionStore::new(self.state_root.clone()).list(request)
    }

    async fn connection_set(
        &self,
        target: Option<&str>,
        request: ConnectionSetRequest,
    ) -> Result<ConnectionMutationResult, NativeV2CliError> {
        require_local(target)?;
        LocalConnectionStore::new(self.state_root.clone()).set(request)
    }

    async fn connection_delete(
        &self,
        target: Option<&str>,
        request: ConnectionDeleteRequest,
    ) -> Result<ConnectionDeleteResult, NativeV2CliError> {
        require_local(target)?;
        LocalConnectionStore::new(self.state_root.clone()).delete(request)
    }

    async fn profile_list(
        &self,
        target: Option<&str>,
        request: RunProfileListRequest,
    ) -> Result<RunProfileListResult, NativeV2CliError> {
        self.profile_list_with_problems(target, request)
            .await?
            .into_protocol()
    }

    async fn profile_list_with_problems(
        &self,
        target: Option<&str>,
        request: RunProfileListRequest,
    ) -> Result<CliProfileListResult, NativeV2CliError> {
        require_local(target)?;
        LocalRunProfileStore::production()?.list(request)
    }

    async fn profile_show(
        &self,
        target: Option<&str>,
        selector: RunProfileSelector,
    ) -> Result<RunProfile, NativeV2CliError> {
        require_local(target)?;
        LocalRunProfileStore::production()?.show(selector)
    }

    async fn profile_set(
        &self,
        target: Option<&str>,
        request: RunProfileSetRequest,
    ) -> Result<RunProfileMutationResult, NativeV2CliError> {
        require_local(target)?;
        LocalRunProfileStore::production()?.set(request)
    }

    async fn profile_delete(
        &self,
        target: Option<&str>,
        selector: RunProfileSelector,
    ) -> Result<RunProfileDeleteResult, NativeV2CliError> {
        require_local(target)?;
        LocalRunProfileStore::production()?.delete(selector)
    }

    async fn profile_default(
        &self,
        target: Option<&str>,
        request: RunProfileDefaultRequest,
    ) -> Result<RunProfileDefaultResult, NativeV2CliError> {
        require_local(target)?;
        LocalRunProfileStore::production()?.set_default(request)
    }

    async fn run_submit(
        &self,
        target: Option<&str>,
        request: PreparedRunRequest,
    ) -> Result<RunSubmitResult, NativeV2CliError> {
        require_local(target)?;
        let run_id = self.start_controller(request).await?;
        Ok(RunSubmitResult { run_id })
    }

    async fn run_list(
        &self,
        target: Option<&str>,
        _params: RunListParams,
    ) -> Result<CliRunListResult, NativeV2CliError> {
        require_local(target)?;
        self.list_local().await.map(Into::into)
    }

    async fn run_status(
        &self,
        target: Option<&str>,
        params: RunStatusParams,
    ) -> Result<CliRunStatusResult, NativeV2CliError> {
        require_local(target)?;
        self.status_local(params).await.map(Into::into)
    }

    async fn run_resume_status(
        &self,
        target: Option<&str>,
        params: RunStatusParams,
    ) -> Result<CliRunStatusResult, NativeV2CliError> {
        require_local(target)?;
        self.confirm_loss_for_resume(&params.run_id).await?;
        self.status_local(params).await.map(Into::into)
    }

    async fn run_watch(
        &self,
        target: Option<&str>,
        params: RunWatchParams,
    ) -> Result<Self::Watch, NativeV2CliError> {
        require_local(target)?;
        self.inspect_watch(params).await
    }

    async fn run_logs(
        &self,
        target: Option<&str>,
        params: RunLogsParams,
    ) -> Result<Self::Logs, NativeV2CliError> {
        require_local(target)?;
        self.inspect_logs(params).await
    }

    async fn run_attach(
        &self,
        target: Option<&str>,
        params: RunAttachParams,
    ) -> Result<Self::Attach, NativeV2CliError> {
        require_local(target)?;
        self.inspect_attach(params).await
    }

    async fn run_force(
        &self,
        target: Option<&str>,
        params: RunForceParams,
    ) -> Result<CliRunForceResult, NativeV2CliError> {
        require_local(target)?;
        self.force_local(params).await.map(Into::into)
    }

    async fn validate_resume_selection(
        &self,
        command: &crate::native_v2_cli::RunResumeCommand,
    ) -> Result<(), NativeV2CliError> {
        require_local(command.run.target.as_deref())?;
        let recovery = self.read_recovery_document(&command.run.run_id)?;
        local_restore_selection(
            &command.run.run_id,
            command.from.as_ref(),
            &recovery,
            command.discard_workspace_changes,
        )
        .map(|_| ())
    }

    async fn run_checkpoints(
        &self,
        target: Option<&str>,
        params: openengine_cluster_protocol::RunCheckpointsParams,
    ) -> Result<openengine_cluster_protocol::RunCheckpointsResult, NativeV2CliError> {
        require_local(target)?;
        let status = self
            .status_local(RunStatusParams {
                run_id: params.run_id.clone(),
            })
            .await?;
        let mut result = crate::native_v2_supervisor::checkpoints::list(
            &self.paths(&params.run_id)?.storage().join("checkpoints"),
            params,
        )
        .map_err(local_error)?;
        result.controller = status.controller;
        Ok(result)
    }

    async fn run_resume(
        &self,
        target: Option<&str>,
        params: openengine_cluster_protocol::RunResumeParams,
        discard_workspace_changes: bool,
    ) -> Result<openengine_cluster_protocol::RunResumeResult, NativeV2CliError> {
        require_local(target)?;
        self.resume_local(params, discard_workspace_changes).await
    }

    async fn run_discard_workspace(
        &self,
        target: Option<&str>,
        _params: openengine_cluster_protocol::RunDiscardWorkspaceParams,
    ) -> Result<openengine_cluster_protocol::RunDiscardWorkspaceResult, NativeV2CliError> {
        require_local(target)?;
        Err(local_message(
            "local workspaces are user-owned and cannot be discarded by Zeroshot",
        ))
    }
}

#[cfg(test)]
#[path = "backend/checkpoint_compatibility_tests.rs"]
mod checkpoint_compatibility_tests;

#[cfg(test)]
#[path = "backend/tests.rs"]
mod tests;
