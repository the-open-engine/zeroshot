//! Local submission deduplication and remote admission before controller launch.
use super::*;

impl LocalCliBackend {
    pub(super) async fn submit_prepared(
        &self,
        mut prepared: PreparedLocalRun,
        worktree: Option<crate::native_v2_cli::LocalWorktreeBase>,
    ) -> Result<RunId, NativeV2CliError> {
        let request_bytes =
            serde_json::to_vec(&(&prepared.submission, worktree, &prepared.local_branch))
                .map_err(local_error)?;
        let request_digest = Sha256Digest::new(format!("{:x}", Sha256::digest(&request_bytes)))
            .map_err(local_error)?;
        prepared.request_digest = Some(request_digest.clone());
        {
            let _submission_lock = self.acquire_submission_lock().await?;
            if let Some(run_id) = self
                .existing_local_request(&prepared, &request_digest)
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
            .existing_local_request(&prepared, &request_digest)
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
        if worktree.is_some() {
            crate::native_v2_local::create_run_worktree(
                &mut prepared,
                &self.git_program,
                &self.state_root,
            )
            .await
            .map_err(|error| NativeV2CliError::Usage(error.to_string()))?;
        }
        self.start_prepared_controller(prepared).await
    }
}
