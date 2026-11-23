//! Prepare host-owned or local checkpoint storage before provider dispatch.

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
