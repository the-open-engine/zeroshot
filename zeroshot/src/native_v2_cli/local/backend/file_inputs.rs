//! Local request admission binds files under the existing successor claim.
use super::*;

impl LocalCliBackend {
    pub(super) async fn release_answer_uploads(
        &self,
        run_id: &RunId,
        output: &serde_json::Value,
    ) -> Result<(), NativeV2CliError> {
        let (paths, _directory) = self.recorded_storage(run_id)?;
        let ledger = SqliteRunLedger::open_read_only(paths.ledger()).map_err(local_error)?;
        if let Some(stored) = ledger.get(run_id).await.map_err(local_error)?
            && let Some(openengine_cluster_protocol::RunSuspension::Request { request, .. }) =
                &stored.snapshot.suspension
        {
            crate::native_v2_cli::files::release_staged_uploads(
                &self.state_root,
                &request.output,
                output,
            )?;
        }
        Ok(())
    }

    pub(super) async fn resolve_local_request(
        &self,
        params: &openengine_cluster_protocol::RunResumeParams,
    ) -> Result<Option<openengine_cluster_protocol::RequestResolution>, NativeV2CliError> {
        let resolution = if matches!(
            params.from,
            Some(RunResumeFrom::Answer { .. } | RunResumeFrom::Expiry {})
        ) {
            let (paths, _directory) = self.recorded_storage(&params.run_id)?;
            let ledger = SqliteRunLedger::open_read_only(paths.ledger()).map_err(local_error)?;
            let stored = ledger
                .get(&params.run_id)
                .await
                .map_err(local_error)?
                .ok_or_else(|| invalid_request("run is not suspended"))?;
            let suspension = stored
                .snapshot
                .suspension
                .as_ref()
                .filter(|_| stored.snapshot.phase == crate::v2_run_ledger::RunPhase::Suspended)
                .ok_or_else(|| invalid_request("answer requires a suspended run"))?;
            let resolution = crate::native_v2_request::resolve(
                suspension,
                params.from.as_ref(),
                crate::native_v2_runner::current_timestamp(),
            )
            .map_err(invalid_request)?;
            crate::native_v2_request::validate_resolution(
                &stored.snapshot,
                &params.successor_run_id,
                &resolution,
            )
            .map_err(invalid_request)?;
            Some(resolution)
        } else {
            None
        };
        Ok(resolution)
    }

    pub(super) async fn accept_request_files(
        &self,
        params: &openengine_cluster_protocol::RunResumeParams,
        prepared: &PreparedLocalRun,
        resolution: &mut Option<openengine_cluster_protocol::RequestResolution>,
    ) -> Result<Option<crate::native_v2_files::inputs::FileInputClaim>, NativeV2CliError> {
        let mut input_claim = None;
        if let Some(openengine_cluster_protocol::RequestResolution::Answer { answer }) = resolution
        {
            let (paths, _directory) = self.recorded_storage(&params.run_id)?;
            let ledger = SqliteRunLedger::open_read_only(paths.ledger()).map_err(local_error)?;
            let stored = ledger
                .get(&params.run_id)
                .await
                .map_err(local_error)?
                .ok_or_else(|| invalid_request("request is unavailable"))?;
            let Some(openengine_cluster_protocol::RunSuspension::Request { request, .. }) =
                &stored.snapshot.suspension
            else {
                return Err(invalid_request("run has no request"));
            };
            if crate::native_v2_files::inputs::has_file_inputs(&request.output, &answer.output) {
                let files = crate::native_v2_cli::files::workspace_files(&self.state_root)?;
                let key = IdempotencyKey::new(format!(
                    "answer-{:x}",
                    Sha256::digest(params.successor_run_id.as_str().as_bytes())
                ))
                .map_err(local_error)?;
                let frozen = files
                    .freeze_answer_inputs(crate::native_v2_files::inputs::InputAdmission {
                        run_id: &params.successor_run_id,
                        owner: &prepared.delivery_run_id,
                        key: &key,
                        schema: &request.output,
                        value: &answer.output,
                        local_root: Some(&self.current_directory),
                    })
                    .map_err(|error| invalid_request(format!("$.output: {error}")))?;
                input_claim = frozen.claim;
                answer.output = frozen.value;
            }
            if let Err(error) = crate::native_v2_request::validate_resolution(
                &stored.snapshot,
                &params.successor_run_id,
                resolution.as_ref().expect("answer resolution"),
            ) {
                if let Some(claim) = input_claim {
                    self.release_refused_input_claim(&params.successor_run_id, &claim)
                        .await?;
                }
                return Err(invalid_request(error));
            }
        }
        Ok(input_claim)
    }

    pub(super) async fn release_refused_input_claim(
        &self,
        run_id: &RunId,
        claim: &crate::native_v2_files::inputs::FileInputClaim,
    ) -> Result<(), NativeV2CliError> {
        let Some(files) = crate::native_v2_cli::files::input_files_if_present(&self.state_root)?
        else {
            return Ok(());
        };
        let successor_paths = self.paths(run_id)?;
        if !successor_paths
            .storage()
            .try_exists()
            .map_err(local_error)?
            || self
                .successor_claim_is_live_or_admitted(
                    run_id,
                    successor_paths.storage(),
                    Duration::ZERO,
                )
                .await
                .is_ok_and(|owned| !owned)
        {
            files.release_file_input_claim(claim).map_err(local_error)?;
        }
        Ok(())
    }
}
