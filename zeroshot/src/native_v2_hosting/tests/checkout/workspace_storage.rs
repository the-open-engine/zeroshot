use super::*;
use crate::native_v2_supervisor::checkpoints::{
    CheckpointError, ResticCheckpointStore, ResticCheckpointStoreTestConfig,
};

struct ExternalStorage {
    reject: bool,
}

#[async_trait::async_trait]
impl HostedWorkspaceStorage for ExternalStorage {
    async fn prepare(
        &self,
        request: HostedWorkspaceRequest<'_>,
    ) -> Result<HostedWorkspace, CheckpointError> {
        assert!(
            request.workspace.join(".git/HEAD").is_file(),
            "source must already be installed"
        );
        if self.reject {
            return Err(std::io::Error::other("restore failed").into());
        }
        fs::write(
            request.workspace.join("restored.txt"),
            b"durable cloud bytes",
        )?;
        Ok(HostedWorkspace {
            checkpoints: Arc::new(ResticCheckpointStore::with_program(
                ResticCheckpointStoreTestConfig {
                    directory: request.checkpoint_directory.to_owned(),
                    repository: request.checkpoint_directory.join("repository"),
                    workspace: request.workspace.to_owned(),
                    writers: request.writers,
                    program: crate::native_v2_supervisor::checkpoints::restic::ResticProgram::fake(
                        request
                            .workspace
                            .parent()
                            .ok_or_else(|| std::io::Error::other("workspace has no parent"))?,
                    )?,
                },
            )),
            execution_seed: Vec::new(),
            delivery_run_id: Some(RunId::new("original-cloud-delivery")),
        })
    }
}

#[tokio::test]
async fn external_workspace_restore_precedes_dispatch_and_preserves_delivery_lineage() {
    let fixture = CheckoutFixture::with_workspace_storage(
        "",
        Some(Arc::new(ExternalStorage { reject: false })),
    )
    .await;
    let capsule = fixture.allocate().await.assert_value();
    let workspace = fixture
        .allocator
        .run_path(&fixture.run_id)
        .join("workspace");
    assert_eq!(
        fs::read(workspace.join("restored.txt")).assert_value(),
        b"durable cloud bytes"
    );
    assert!(capsule.checkpoints.is_some());
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();
    assert_eq!(
        fixture.allocator.recovery_delivery_run_id(&fixture.run_id),
        Some(RunId::new("original-cloud-delivery"))
    );
}

#[tokio::test]
async fn external_restore_failure_never_releases_an_allocated_capsule() {
    let fixture = CheckoutFixture::with_workspace_storage(
        "",
        Some(Arc::new(ExternalStorage { reject: true })),
    )
    .await;
    assert!(matches!(
        fixture.allocate().await,
        Err(CapsuleAllocationUnavailable::Runtime)
    ));
    assert!(!fixture.allocator.run_path(&fixture.run_id).exists());
}
