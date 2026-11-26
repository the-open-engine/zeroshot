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
        if let Some(storage) = &self.config.workspace_storage {
            let mut prepared = storage
                .prepare(HostedWorkspaceRequest {
                    run_id: request.run_id,
                    workspace: request.workspace,
                    checkpoint_directory: &directory,
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
        let checkpoints = if let Some(storage) = &self.config.workspace_storage {
            storage
                .prepare(HostedWorkspaceRequest {
                    run_id,
                    workspace: &workspace,
                    checkpoint_directory: &directory,
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
