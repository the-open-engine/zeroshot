use super::*;
use crate::native_v2_supervisor::RunEnvironment;

impl LocalCliBackend {
    async fn resume_local(
        &self,
        params: openengine_cluster_protocol::RunResumeParams,
    ) -> Result<openengine_cluster_protocol::RunResumeResult, NativeV2CliError> {
        let _submission_lock = self.acquire_submission_lock().await?;
        self.reconcile_local_resume_claim(&params.run_id).await?;
        let status = self
            .status_local(RunStatusParams {
                run_id: params.run_id.clone(),
            })
            .await?;
        if !status.workspace_recovery.recoverable {
            return Err(local_message("run does not have a recoverable workspace"));
        }
        let recovery = self.read_recovery_document(&params.run_id)?;
        if recovery.successor_run_id.is_some() {
            return Err(local_message("retained workspace was already claimed"));
        }
        let _claim = self.claim_recovery_workspace(&params.run_id)?;
        self.start_local_successor(params, recovery).await
    }

    fn checkpoint_selection(
        &self,
        params: &openengine_cluster_protocol::RunResumeParams,
    ) -> Result<Option<crate::native_v2_supervisor::checkpoints::CheckpointRestore>, NativeV2CliError>
    {
        use crate::native_v2_supervisor::checkpoints::{
            CheckpointRestore, CheckpointRestoreSelection, latest_available, restore_selection,
        };
        let directory = self.paths(&params.run_id)?.storage().join("checkpoints");
        let selection = restore_selection(params.from.as_ref());
        if matches!(&selection, CheckpointRestoreSelection::Latest)
            && !latest_available(&directory).map_err(local_error)?
        {
            return Ok(None);
        }
        Ok(Some(CheckpointRestore {
            directory,
            selection,
        }))
    }

    async fn start_local_successor(
        &self,
        params: openengine_cluster_protocol::RunResumeParams,
        mut recovery: LocalRecoveryDocument,
    ) -> Result<openengine_cluster_protocol::RunResumeResult, NativeV2CliError> {
        let connections = LocalConnectionStore::new(self.state_root.clone())
            .resolve(&recovery.submission.runtime, &params.connections)?
            .bootstrap_values();
        let environment = RunEnvironment::exact(&recovery.submission.runtime, connections)
            .map_err(local_error)?;
        let checkpoint = self.checkpoint_selection(&params)?;
        recovery.successor_run_id = Some(params.successor_run_id.clone());
        self.write_recovery_document(&params.run_id, &recovery)?;
        let prepared = PreparedLocalRun {
            run_id: params.successor_run_id.clone(),
            delivery_run_id: self.delivery_run_id(&params.run_id, &recovery)?,
            submission: recovery.submission,
            environment,
            github_token: params.github_token,
            workspace: recovery.workspace,
            native_environment: crate::native_v2_local::capture_local_native_environment(
                &self.current_directory,
            )
            .map_err(local_error)?,
        };
        if let Err(error) = self
            .start_prepared_controller_with_lineage(
                prepared,
                Some(params.run_id.clone()),
                checkpoint,
            )
            .await
        {
            self.reconcile_local_resume_claim(&params.run_id).await?;
            return Err(error);
        }
        let successor = self.read_recovery_document(&params.successor_run_id)?;
        debug_assert_eq!(successor.resumed_from.as_ref(), Some(&params.run_id));
        Ok(openengine_cluster_protocol::RunResumeResult {
            run_id: params.successor_run_id,
            resumed_from: params.run_id,
        })
    }
}

#[async_trait]
impl NativeV2CliBackend for LocalCliBackend {
    type Watch = ChannelSubscription<CliRunWatchEventNotification>;
    type Logs = ChannelSubscription<RunLogEventNotification>;
    type Attach = ChannelSubscription<RunAttachEventNotification>;

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

    async fn run_watch(
        &self,
        target: Option<&str>,
        params: RunWatchParams,
    ) -> Result<Self::Watch, NativeV2CliError> {
        require_local(target)?;
        let transport = self.connect_run(&params.run_id).await?;
        Ok(spawn_watch(transport, params))
    }

    async fn run_logs(
        &self,
        target: Option<&str>,
        params: RunLogsParams,
    ) -> Result<Self::Logs, NativeV2CliError> {
        require_local(target)?;
        let transport = self.connect_run(&params.run_id).await?;
        Ok(spawn_logs(transport, params))
    }

    async fn run_attach(
        &self,
        target: Option<&str>,
        params: RunAttachParams,
    ) -> Result<Self::Attach, NativeV2CliError> {
        require_local(target)?;
        let transport = self.connect_run(&params.run_id).await?;
        Ok(spawn_attach(transport, params))
    }

    async fn run_force(
        &self,
        target: Option<&str>,
        params: RunForceParams,
    ) -> Result<CliRunForceResult, NativeV2CliError> {
        require_local(target)?;
        self.force_local(params).await.map(Into::into)
    }

    async fn run_checkpoints(
        &self,
        target: Option<&str>,
        params: openengine_cluster_protocol::RunCheckpointsParams,
    ) -> Result<openengine_cluster_protocol::RunCheckpointsResult, NativeV2CliError> {
        require_local(target)?;
        self.status_local(RunStatusParams {
            run_id: params.run_id.clone(),
        })
        .await?;
        crate::native_v2_supervisor::checkpoints::list(
            &self.paths(&params.run_id)?.storage().join("checkpoints"),
            params,
        )
        .map_err(local_error)
    }

    async fn run_resume(
        &self,
        target: Option<&str>,
        params: openengine_cluster_protocol::RunResumeParams,
    ) -> Result<openengine_cluster_protocol::RunResumeResult, NativeV2CliError> {
        require_local(target)?;
        self.resume_local(params).await
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
mod checkpoint_compatibility_tests {
    use super::*;
    use crate::native_v2_candidate::test_support::TestDirectory;
    use crate::native_v2_supervisor::checkpoints::CheckpointRestoreSelection;
    use openengine_cluster_testkit::assertions::AssertValue;
    use openengine_cluster_protocol::{CheckpointId, RunResumeFrom};

    fn params(from: Option<RunResumeFrom>) -> openengine_cluster_protocol::RunResumeParams {
        openengine_cluster_protocol::RunResumeParams {
            run_id: RunId::new("0199f33f-3b44-7d21-9000-000000000001"),
            successor_run_id: RunId::new("0199f33f-3b44-7d21-9000-000000000002"),
            from,
            connections: Default::default(),
            connection_resolver: None,
            github_token: None,
        }
    }

    #[test]
    fn legacy_resume_uses_the_retained_workspace_but_checkpoint_selection_stays_strict() {
        let root = TestDirectory::new("llr");
        let backend = LocalCliBackend::new(
            root.path().to_owned(),
            PathBuf::from("zeroshot"),
            root.path().to_owned(),
            PathBuf::from("git"),
        );

        for from in [None, Some(RunResumeFrom::Restart {})] {
            assert!(
                backend
                    .checkpoint_selection(&params(from))
                    .assert_value()
                    .is_none()
            );
        }
        let selected = backend
            .checkpoint_selection(&params(Some(RunResumeFrom::Checkpoint {
                checkpoint_id: CheckpointId::new("entry-1").assert_value(),
            })))
            .assert_value()
            .assert_value();
        assert!(matches!(
            selected.selection,
            CheckpointRestoreSelection::Checkpoint { .. }
        ));
    }
}
