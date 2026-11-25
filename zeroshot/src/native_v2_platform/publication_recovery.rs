//! Reconcile committed breakpoint receipts before generic interrupted-runtime settlement.
use super::*;
use crate::native_v2_files::publication::{FilePublication, RunFilePublisher};
use crate::native_v2_supervisor::{NativeV2Supervisor, RunRuntimeCleanup};

impl NativeV2PlatformController {
    pub(super) async fn recover_file_publication(
        &self,
        stored: &StoredRun,
    ) -> Result<bool, NativeV2PlatformError> {
        let Some(request) = crate::native_v2_supervisor::pending_file_publication(stored)? else {
            return Ok(false);
        };
        let Some(recovered) = self
            .allocator
            .recover_file_publication(stored, &request)
            .await?
        else {
            return Ok(false);
        };
        let supervisor = NativeV2Supervisor::new(
            stored.snapshot.run_id.clone(),
            self.ledger.clone(),
            Arc::new(ReceiptReplayOnly),
            Arc::new(RunEnvironment::without_credentials()),
        )
        .with_file_publisher(Some(Arc::new(CommittedReceipt {
            request,
            files: recovered.files,
        })))
        .with_checkpoints(Some(recovered.checkpoints))
        .with_delivery_policy(self.delivery_policy)
        .with_runtime_cleanup(Arc::new(RecoveredRuntimeCleanup {
            allocator: self.allocator.clone(),
            run_id: stored.snapshot.run_id.clone(),
        }));
        match supervisor.drive().await {
            Ok(_) | Err(NativeV2SupervisorError::Suspended) => Ok(true),
            Err(error) => Err(error.into()),
        }
    }
}

struct CommittedReceipt {
    request: FilePublication,
    files: openengine_cluster_protocol::PublishedFiles,
}

#[async_trait]
impl RunFilePublisher for CommittedReceipt {
    async fn publish(
        &self,
        request: FilePublication,
    ) -> std::io::Result<openengine_cluster_protocol::PublishedFiles> {
        if request.owner != self.request.owner || request.paths != self.request.paths {
            return Err(std::io::Error::other(
                "committed publication does not match breakpoint",
            ));
        }
        (request.preflight)(&self.files)?;
        Ok(self.files.clone())
    }
}

struct ReceiptReplayOnly;

#[async_trait]
impl NodeRunner for ReceiptReplayOnly {
    async fn start(
        &self,
        _: crate::native_v2_runner::NodeRunRequest,
    ) -> Result<crate::native_v2_runner::NodeHandle, crate::native_v2_runner::NodeRunnerError> {
        Err(crate::native_v2_runner::NodeRunnerError::InvalidRole)
    }
    async fn close_run(&self, _: &RunId) {}
}

struct RecoveredRuntimeCleanup {
    allocator: Arc<dyn CapsuleAllocator>,
    run_id: RunId,
}

#[async_trait]
impl RunRuntimeCleanup for RecoveredRuntimeCleanup {
    async fn cleanup(&self, exit: RunRuntimeExit) -> Result<(), RuntimeCleanupUnavailable> {
        self.allocator
            .destroy_or_confirm_absent(&self.run_id, exit)
            .await
            .map(|_| ())
            .map_err(|_| RuntimeCleanupUnavailable)
    }
}
