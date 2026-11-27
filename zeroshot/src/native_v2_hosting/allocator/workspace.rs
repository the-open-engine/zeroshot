//! Prepare and recover host-owned or local workspace checkpoint authority.

use super::*;
use crate::native_v2_admission::writer_nodes;
use crate::native_v2_hosting::HostedWorkspaceRequest;

pub(super) struct WorkspacePreparation<'a> {
    pub(super) run_id: &'a RunId,
    pub(super) checkpoint_source: Option<&'a RunId>,
    pub(super) delivery_run_id: &'a RunId,
    pub(super) workspace: &'a Path,
    pub(super) admitted: &'a AdmittedRun,
    pub(super) state: &'a ProductionCapsuleState,
    pub(super) process_pool: HostedProcessPool,
    pub(super) redactions: &'a [String],
}

impl ProductionCapsuleAllocator {
    pub(super) fn accept_request_files(
        &self,
        request: crate::native_v2_files::inputs::InputAdmission<'_>,
    ) -> Result<crate::native_v2_files::inputs::FrozenFileInputs, String> {
        let Some(files) = &self.config.workspace_files else {
            let mut value = request.value.clone();
            crate::native_v2_files::inputs::visit(request.schema, &mut value, "", &mut |_, _| {
                Err("host does not enable file inputs".to_owned())
            })?;
            return Ok(crate::native_v2_files::inputs::FrozenFileInputs {
                run_id: request.run_id.clone(),
                value,
                claim: None,
            });
        };
        let recovery = read_recovery(&recovery_path(&self.config.storage_root, request.owner));
        let owner = recovery
            .as_ref()
            .and_then(|document| document.delivery_run_id.as_ref())
            .unwrap_or(request.owner);
        files
            .freeze_answer_inputs(crate::native_v2_files::inputs::InputAdmission {
                owner,
                ..request
            })
            .map_err(|error| error.to_string())
    }

    pub(super) fn release_refused_answer(
        &self,
        claim: &crate::native_v2_files::inputs::FileInputClaim,
    ) {
        if let Some(files) = &self.config.workspace_files {
            let _ = files.release_file_input_claim(claim);
        }
    }

    pub(super) fn release_request_uploads(
        &self,
        schema: &openengine_cluster_protocol::PayloadType,
        value: &serde_json::Value,
    ) {
        if let Some(files) = &self.config.workspace_files {
            let _ = files.release_staged_inputs(schema, value);
        }
    }

    pub(super) async fn release_disposed_inputs(
        &self,
        run_id: &RunId,
        origin: &RunId,
        exit: RunRuntimeExit,
    ) -> Result<(), CapsuleCleanupUnavailable> {
        if matches!(
            exit,
            RunRuntimeExit::Completed | RunRuntimeExit::ForceStopped
        ) && let Some(files) = &self.config.workspace_files
        {
            files
                .release(origin)
                .map_err(|_| CapsuleCleanupUnavailable)?;
            files
                .release_file_inputs(origin)
                .map_err(|_| CapsuleCleanupUnavailable)?;
        }
        if !self.workspace_recovery(run_id).await.recoverable {
            if let Some(files) = &self.config.workspace_files {
                files
                    .release_file_input_part(run_id)
                    .map_err(|_| CapsuleCleanupUnavailable)?;
            }
        }
        Ok(())
    }

    pub(super) async fn discard_retained_workspace(
        &self,
        run_id: &RunId,
    ) -> Result<bool, CapsuleCleanupUnavailable> {
        let _turn = self.allocation_turns.lock(run_id).await;
        let root = run_directory(&self.config.storage_root, run_id);
        let metadata_path = recovery_path(&self.config.storage_root, run_id);
        let Some(mut document) = read_recovery(&metadata_path) else {
            return Ok(false);
        };
        if !document.recoverable {
            return Ok(false);
        }
        let delivery_run_id = document.delivery_run_id.as_ref().unwrap_or(run_id);
        checkpoints::discard_lineage(
            &checkpoint_directory(&self.config.storage_root, run_id),
            &checkpoint_repository(&self.config.storage_root, delivery_run_id),
        )
        .map_err(|_| CapsuleCleanupUnavailable)?;
        remove_run_directory(&root)?;
        if let Some(files) = &self.config.workspace_files {
            files
                .release(delivery_run_id)
                .map_err(|_| CapsuleCleanupUnavailable)?;
            files
                .release_file_inputs(delivery_run_id)
                .map_err(|_| CapsuleCleanupUnavailable)?;
        }
        document.recoverable = false;
        write_recovery(&metadata_path, &document)?;
        Ok(true)
    }

    pub(super) fn prepare_capsule_workspace(
        &self,
        pending: &PendingCapsule<'_>,
        request: &CapsuleBuildRequest<'_>,
        paths: &CapsuleBuildPaths,
    ) -> Result<CapsuleFilesystem, CapsuleAllocationUnavailable> {
        if request.install_source {
            pending.state.create_run_directory()?;
        } else {
            pending.state.capture_run_directory()?;
        }
        if request.transfer_retained_workspace {
            transfer_retained_workspace_to_writer(&paths.workspace, pending.process_pool)?;
        }
        let filesystem =
            (self.prepare_filesystem)(&paths.workspace, &paths.runtime_home, pending.process_pool)?;
        let identity = pending
            .process_pool
            .identity(HostedProcessScope::Environment)
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        #[cfg(test)]
        let owner = (!self.portable_test_processes()).then_some(identity);
        #[cfg(not(test))]
        let owner = Some(identity);
        environment::prepare_tools(&pending.state.run_root, owner)?;
        Ok(filesystem)
    }

    pub(super) fn prepare_file_inputs(
        &self,
        request: &CapsuleBuildRequest<'_>,
        state: &ProductionCapsuleState,
        process_pool: HostedProcessPool,
    ) -> Result<(), CapsuleAllocationUnavailable> {
        if let Some(files) = &self.config.workspace_files {
            let restart = request
                .retained_restore
                .as_ref()
                .is_some_and(|(source, selection)| {
                    matches!(selection, CheckpointRestoreSelection::Latest)
                        && !checkpoints::latest_available(&checkpoint_directory(
                            &self.config.storage_root,
                            source,
                        ))
                        .unwrap_or(true)
                });
            let roots = files
                .materialize_file_inputs(request.delivery_run_id, restart)
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
            state
                .file_inputs
                .set(roots.clone())
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
            if !roots.is_empty() {
                super::super::set_traversable_directory(&files.input_data_directory())
                    .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let location = files
                        .input_location(request.delivery_run_id)
                        .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
                    std::fs::set_permissions(location, std::fs::Permissions::from_mode(0o755))
                        .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
                }
                for root in roots {
                    transfer_retained_workspace_to_writer(&root, process_pool)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn file_publisher(
        &self,
        workspace: &Path,
    ) -> Option<Arc<dyn crate::native_v2_files::publication::RunFilePublisher>> {
        self.config.file_store.as_ref().map(|(store, identity)| {
            Arc::new(crate::native_v2_files::publication::ManagedFilePublisher {
                workspace: workspace.to_owned(),
                store: store.clone(),
                identity: identity.clone(),
            }) as Arc<dyn crate::native_v2_files::publication::RunFilePublisher>
        })
    }

    pub(super) async fn prepare_hosted_workspace(
        &self,
        request: WorkspacePreparation<'_>,
    ) -> Result<HostedWorkspace, CapsuleAllocationUnavailable> {
        let directory = checkpoint_directory(&self.config.storage_root, request.run_id);
        let writers = writer_nodes(request.admitted);
        let inputs = match &self.config.workspace_files {
            Some(files) => files
                .file_input_roots(request.delivery_run_id)
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?,
            None => Vec::new(),
        };
        if let Some(storage) = &self.config.workspace_storage {
            let mut prepared = storage
                .prepare(HostedWorkspaceRequest {
                    run_id: request.run_id,
                    workspace: request.workspace,
                    checkpoint_directory: &directory,
                    file_inputs: &inputs,
                    workspace_checkpoints: request.admitted.runtime.workspace_checkpoints,
                    writers,
                })
                .await
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
            if prepared.request_continuation.is_some() && !self.config.request_ask {
                return Err(CapsuleAllocationUnavailable::Runtime);
            }
            if let Some(continuation) = &prepared.request_continuation {
                let openengine_cluster_protocol::RunSuspension::Request {
                    node,
                    request: question,
                    ..
                } = &continuation.suspension
                else {
                    return Err(CapsuleAllocationUnavailable::Runtime);
                };
                let verifier = crate::native_v2_request::find(&request.admitted.graph.root, node)
                    .ok_or(CapsuleAllocationUnavailable::Runtime)?;
                if verifier.worker.as_str() != crate::native_v2_request::ASK_WORKER_REF
                    || verifier.signals != question.signals
                    || verifier.output != question.output
                {
                    return Err(CapsuleAllocationUnavailable::Runtime);
                }
            }
            prepared
                .settle_request(request.run_id)
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
            transfer_retained_workspace_to_writer(request.workspace, request.process_pool)?;
            if let Some(id) = &prepared.delivery_run_id {
                request
                    .state
                    .restored_delivery_run_id
                    .set(id.clone())
                    .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
            }
            return Ok(prepared);
        }
        Ok(HostedWorkspace {
            resumed_from: None,
            comment_publications: None,
            checkpoints: Arc::new(
                ResticCheckpointStore::new_with_fallback(
                    checkpoints::ResticCheckpointStoreRequest {
                        directory: directory.clone(),
                        repository: checkpoint_repository(
                            &self.config.storage_root,
                            request.delivery_run_id,
                        ),
                        workspace: request.workspace.to_owned(),
                        writers,
                    },
                    Some(&checkpoint_directory(
                        &self.config.storage_root,
                        request.checkpoint_source.unwrap_or(request.run_id),
                    )),
                )
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?
                .with_file_inputs(match &self.config.workspace_files {
                    Some(files) => files
                        .file_input_roots(request.delivery_run_id)
                        .map_err(|_| CapsuleAllocationUnavailable::Runtime)?,
                    None => Vec::new(),
                })
                .with_redactions(request.redactions.to_vec())
                .with_conversation_restore(request.state.conversation_restore.get().cloned()),
            ),
            execution_seed: Vec::new(),
            request_continuation: None,
            vector_seed: Vec::new(),
            delivery_run_id: None,
        })
    }
}

impl ProductionCapsuleState {
    pub(super) fn reclaim_file_inputs(&self) -> Result<(), CapsuleCleanupUnavailable> {
        // This runs after process cleanup and before releasing the numeric identity lease,
        // including when allocation failed after transferring only some of the roots.
        for root in self.file_inputs.get().into_iter().flatten() {
            transfer_workspace_ownership(root, 0, 0).map_err(|_| CapsuleCleanupUnavailable)?;
            crate::execution::platform::private_directory(root)
                .map_err(|_| CapsuleCleanupUnavailable)?;
        }
        Ok(())
    }
}

impl ProductionCapsuleAllocator {
    pub(super) async fn recover_committed_file_publication(
        &self,
        stored: &crate::v2_run_ledger::StoredRun,
        request: &crate::native_v2_files::publication::FilePublication,
    ) -> Result<
        Option<crate::native_v2_platform::RecoveredFilePublication>,
        CapsuleAllocationUnavailable,
    > {
        let Some((store, identity)) = &self.config.file_store else {
            return Ok(None);
        };
        let files = {
            let store = store.clone();
            let identity = identity.clone();
            let request = request.clone();
            tokio::task::spawn_blocking(move || {
                store.publication_receipt(
                    &crate::native_v2_files::FileAccess {
                        target: store.target(),
                        identity: &identity,
                    },
                    &request,
                )
            })
            .await
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?
        };
        let Some(files) = files else {
            return Ok(None);
        };
        let run_id = &stored.snapshot.run_id;
        // Quiesce the original allocation before reusing its checkpoint authority. Suspension
        // cleanup retains the workspace; the replayed breakpoint owns its final disposition.
        self.destroy_or_confirm_absent(run_id, RunRuntimeExit::Suspended)
            .await
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        let workspace = CapsuleBuildPaths::new(&self.config.storage_root, run_id).workspace;
        let directory = checkpoint_directory(&self.config.storage_root, run_id);
        let origin = read_recovery(&recovery_path(&self.config.storage_root, run_id))
            .and_then(|document| document.delivery_run_id)
            .unwrap_or_else(|| run_id.clone());
        let inputs = match &self.config.workspace_files {
            Some(files) => files
                .file_input_roots(&origin)
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?,
            None => Vec::new(),
        };
        let checkpoints = if let Some(storage) = &self.config.workspace_storage {
            storage
                .prepare(HostedWorkspaceRequest {
                    run_id,
                    workspace: &workspace,
                    checkpoint_directory: &directory,
                    file_inputs: &inputs,
                    workspace_checkpoints: stored.admitted.runtime.workspace_checkpoints,
                    writers: crate::native_v2_admission::writer_nodes(&stored.admitted),
                })
                .await
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?
                .checkpoints
        } else {
            let recovery = read_recovery(&recovery_path(&self.config.storage_root, run_id));
            let delivery_run_id = recovery
                .as_ref()
                .and_then(|document| document.delivery_run_id.as_ref())
                .unwrap_or(run_id);
            Arc::new(
                ResticCheckpointStore::new_with_fallback(
                    checkpoints::ResticCheckpointStoreRequest {
                        directory: directory.clone(),
                        repository: checkpoint_repository(
                            &self.config.storage_root,
                            delivery_run_id,
                        ),
                        workspace,
                        writers: crate::native_v2_admission::writer_nodes(&stored.admitted),
                    },
                    Some(&directory),
                )
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?,
            )
        };
        Ok(Some(crate::native_v2_platform::RecoveredFilePublication {
            files,
            checkpoints,
        }))
    }
}
