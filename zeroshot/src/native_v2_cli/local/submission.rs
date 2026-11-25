//! Local submission deduplication and remote admission before controller launch.
use super::*;

impl LocalCliBackend {
    pub(super) async fn start_prepared_controller(
        &self,
        prepared: PreparedLocalRun,
    ) -> Result<RunId, NativeV2CliError> {
        let managed = prepared
            .submission
            .workspace
            .as_ref()
            .filter(|workspace| !workspace.is_in_place())
            .map(|_| (prepared.run_id.clone(), prepared.workspace.clone()));
        let result = self
            .start_prepared_controller_with_lineage(prepared, None, None)
            .await
            .map_err(|failure| failure.error);
        if result.is_err()
            && let Some((run_id, workspace)) = managed
        {
            self.discard_unadmitted_workspace(&run_id, Some(&workspace))
                .await?;
        }
        result
    }

    async fn discard_unadmitted_workspace(
        &self,
        run_id: &RunId,
        workspace: Option<&Path>,
    ) -> Result<(), NativeV2CliError> {
        let paths = self.paths(run_id)?;
        if self
            .successor_claim_is_live_or_admitted(run_id, paths.storage(), Duration::ZERO)
            .await
            .is_ok_and(|owned| !owned)
        {
            if let Some(workspace) = workspace {
                let _ =
                    crate::native_v2_supervisor::checkpoints::filesystem::remove_disposable_tree(
                        workspace,
                    );
            }
            crate::native_v2_cli::files::workspace_files(&self.state_root)?
                .release(run_id)
                .map_err(local_error)?;
        }
        Ok(())
    }

    /// The request precedes remote default/base resolution. Replays use its frozen recovery
    /// fingerprint so a moved remote or dirty retained worktree cannot start another run.
    pub(super) async fn existing_local_request(
        &self,
        key: &IdempotencyKey,
        digest: &Sha256Digest,
        workspace: Option<&Path>,
    ) -> Result<Option<RunId>, NativeV2CliError> {
        for run_id in self.local_run_ids()? {
            if !self.recovery_path(&run_id)?.is_file() {
                continue;
            }
            let recovery = self.read_recovery_document(&run_id)?;
            if &recovery.submission.submission_key != key {
                continue;
            }
            let Some(saved) = &recovery.request_digest else {
                continue;
            };
            if saved != digest
                || workspace.is_some_and(|workspace| {
                    recovery
                        .invoking_workspace
                        .as_deref()
                        .unwrap_or(&recovery.workspace)
                        != workspace
                })
            {
                return Err(NativeV2CliError::SubmissionConflict {
                    existing_run_id: run_id.as_str().to_owned(),
                });
            }
            let final_digest = submission_digest(&recovery.submission).map_err(local_error)?;
            if let Some(run_id) = self
                .matching_submission(run_id, &recovery.submission.submission_key, &final_digest)
                .await?
            {
                return Ok(Some(run_id));
            }
        }
        Ok(None)
    }

    pub(super) async fn submit_prepared(
        &self,
        mut prepared: PreparedLocalRun,
        worktree: Option<crate::native_v2_cli::LocalWorktreeBase>,
    ) -> Result<RunId, NativeV2CliError> {
        let request_digest = local_request_digest(&prepared, worktree)?;
        prepared.request_digest = Some(request_digest.clone());
        {
            let _submission_lock = self.acquire_submission_lock().await?;
            if let Some(run_id) = self
                .existing_local_request(
                    &prepared.submission.submission_key,
                    &request_digest,
                    prepared
                        .submission
                        .workspace
                        .is_none()
                        .then_some(prepared.workspace.as_path()),
                )
                .await?
            {
                return Ok(run_id);
            }
            if worktree.is_none() {
                let digest = submission_digest(&prepared.submission).map_err(local_error)?;
                if let Some(run_id) = self
                    .existing_submission(
                        &prepared.submission.submission_key,
                        &digest,
                        &prepared.workspace,
                    )
                    .await?
                {
                    return Ok(run_id);
                }
            }
        }
        // Forge admission must precede Git's remote-default lookup as well as agent startup.
        // Remote I/O must not hold the shared submission lock.
        crate::native_v2_local::check_forge_access(&prepared, &self.state_root)
            .await
            .map_err(|error| NativeV2CliError::Usage(error.to_string()))?;
        crate::native_v2_local::prepare_branch_source(
            &mut prepared,
            worktree,
            &self.git_program,
            &self.state_root,
        )
        .await
        .map_err(|error| self.local_branch_error(error, &prepared.workspace))?;
        let digest = submission_digest(&prepared.submission).map_err(local_error)?;

        let existing = || {
            self.existing_submission(
                &prepared.submission.submission_key,
                &digest,
                &prepared.workspace,
            )
        };
        if crate::native_v2_admission::delivery_capabilities(&prepared.submission.graph.root)
            .contains(&crate::source_code_provider::SourceCapability::PullRequest)
        {
            {
                let _submission_lock = self.acquire_submission_lock().await?;
                if let Some(run_id) = existing().await? {
                    return Ok(run_id);
                }
            }
            // Remote I/O must not hold the shared submission lock.
            crate::native_v2_local::validate_remote_base_branch(
                &prepared,
                &self.git_program,
                &self.state_root,
            )
            .await
            .map_err(|error| NativeV2CliError::Usage(error.to_string()))?;
        }

        // A concurrent submitter may have accepted this request during remote validation.
        let _submission_lock = self.acquire_submission_lock().await?;
        if let Some(run_id) = self
            .existing_local_request(
                &prepared.submission.submission_key,
                &request_digest,
                prepared
                    .submission
                    .workspace
                    .is_none()
                    .then_some(prepared.workspace.as_path()),
            )
            .await?
        {
            return Ok(run_id);
        }
        if let Some(run_id) = existing().await? {
            return Ok(run_id);
        }
        crate::native_v2_local::validate_clean_checkout(&prepared, &self.git_program)
            .await
            .map_err(|error| NativeV2CliError::Usage(error.to_string()))?;
        let harnesses = crate::harnesses::HarnessRegistry::local();
        let sizes = crate::native_v2_hosting::host_run_sizes();
        let admitted = NativeV2Admission
            .admit_with_context(
                prepared.submission.clone(),
                crate::native_v2_admission::NativeV2AdmissionContext::new(
                    &harnesses,
                    DeliveryPolicy::Optional,
                )
                .with_run_sizes(&sizes),
            )
            .await
            .map_err(NativeV2CliError::InvalidRun)?;
        crate::v2_run_ledger::validate_admitted_run(&admitted)?;
        if worktree.is_some() {
            crate::native_v2_local::create_run_worktree(
                &mut prepared,
                &self.git_program,
                &self.state_root,
            )
            .await
            .map_err(|error| NativeV2CliError::Usage(error.to_string()))?;
        }
        if prepared
            .submission
            .workspace
            .as_ref()
            .is_some_and(|workspace| !workspace.is_in_place())
        {
            self.prepare_managed_workspace(&mut prepared).await?;
        }
        self.start_prepared_controller(prepared).await
    }
}

impl LocalCliBackend {
    async fn prepare_managed_workspace(
        &self,
        prepared: &mut PreparedLocalRun,
    ) -> Result<(), NativeV2CliError> {
        let source = prepared.workspace.clone();
        let name = self
            .state_root
            .file_name()
            .ok_or_else(|| local_message("state root has no name"))?
            .to_string_lossy();
        let root = self.state_root.with_file_name(format!("{name}-workspaces"));
        crate::native_v2_local::validate_workspace_state(&source, &root).map_err(local_error)?;
        let files = super::super::files::workspace_files(&self.state_root)?;
        let workspace = prepared
            .submission
            .workspace
            .clone()
            .ok_or_else(|| local_message("workspace mode is missing"))?;
        let run_id = prepared.run_id.clone();
        let key = prepared.submission.submission_key.clone();
        let frozen = {
            let files = files.clone();
            let source = source.clone();
            tokio::task::spawn_blocking(move || {
                files.freeze(crate::native_v2_files::FreezeWorkspace {
                    run_id: &run_id,
                    submission_key: &key,
                    local_root: workspace.copy_root.then_some(source.as_path()),
                    workspace: &workspace,
                })
            })
            .await
            .map_err(local_error)?
            .map_err(local_error)?
        };
        prepared.run_id = frozen.clone();
        prepared.delivery_run_id = frozen.clone();
        let destination = root.join(frozen.as_str());
        let valid_destination = destination.parent() == Some(root.as_path())
            && Path::new(frozen.as_str()).components().count() == 1;
        let result = async {
            if !valid_destination {
                return Err(local_message("invalid workspace run identity"));
            }
            crate::execution::platform::private_directory(&root).map_err(local_error)?;
            if destination.exists() {
                let paths = self.paths(&frozen)?;
                if self
                    .successor_claim_is_live_or_admitted(&frozen, paths.storage(), Duration::ZERO)
                    .await?
                {
                    return Err(local_message(
                        "managed workspace still belongs to an admitted or running controller",
                    ));
                }
                crate::native_v2_supervisor::checkpoints::filesystem::remove_disposable_tree(
                    &destination,
                )
                .map_err(local_error)?;
            }
            std::fs::create_dir(&destination).map_err(local_error)?;
            // Complete publication in this poll, so cancellation cannot leave a detached writer
            // racing a retry or controller cleanup.
            files
                .materialize(&frozen, &destination)
                .map_err(local_error)?;
            crate::native_v2_local::validate_workspace_state(&destination, &self.state_root)
                .map_err(local_error)?;
            prepared.invoking_workspace = Some(source);
            prepared.workspace = destination.clone();
            if let Some(workspace) = &mut prepared.submission.workspace {
                workspace.local_directory = Some(
                    prepared
                        .workspace
                        .to_str()
                        .ok_or_else(|| local_message("workspace path is not representable"))?
                        .into(),
                );
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            self.discard_unadmitted_workspace(
                &frozen,
                valid_destination.then_some(destination.as_path()),
            )
            .await?;
        }
        result
    }
}

fn local_request_digest(
    prepared: &PreparedLocalRun,
    worktree: Option<crate::native_v2_cli::LocalWorktreeBase>,
) -> Result<Sha256Digest, NativeV2CliError> {
    if let Some(digest) = &prepared.request_digest {
        return Ok(digest.clone());
    }
    let bytes = serde_json::to_vec(&(&prepared.submission, worktree, &prepared.local_branch))
        .map_err(local_error)?;
    Sha256Digest::new(format!("{:x}", Sha256::digest(bytes))).map_err(local_error)
}

/// Called only after runtime cleanup, when success or explicit stop ends workspace recovery.
pub(crate) fn release_workspace_inputs(storage: &Path) -> Result<(), NativeV2CliError> {
    let recovery = match LocalRecoveryDocument::read(&storage.join(RECOVERY_FILE)) {
        Ok(recovery) => recovery,
        Err(_) if !storage.join(RECOVERY_FILE).exists() => return Ok(()),
        Err(error) => return Err(error),
    };
    if !recovery
        .submission
        .workspace
        .as_ref()
        .is_some_and(|spec| !spec.is_in_place())
    {
        return Ok(());
    }
    let origin = recovery
        .delivery_run_id
        .ok_or_else(|| local_message("workspace input identity is missing"))?;
    let state = storage
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| local_message("run storage has no state root"))?;
    crate::native_v2_cli::files::workspace_files(state)?
        .release(&origin)
        .map_err(local_error)
}

#[cfg(test)]
#[path = "submission/tests.rs"]
mod tests;
