use super::*;

use crate::full_v1_reducer::{DurableExecution, ExecutionBoundary};
use crate::native_v2_contract::DeliveryCheckpoint;
use crate::native_v2_supervisor::checkpoints::{CheckpointError, RunCheckpointStore};

struct HeldDiscard {
    files: PathBuf,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait]
impl RunCheckpointStore for HeldDiscard {
    async fn retain_delivery_state(&self, _: &DeliveryCheckpoint) -> Result<(), CheckpointError> {
        Ok(())
    }

    async fn delivery_state(&self) -> Result<Option<DeliveryCheckpoint>, CheckpointError> {
        Ok(None)
    }

    async fn enter(
        &self,
        _: &ExecutionBoundary,
        _: &[DurableExecution],
    ) -> Result<(), CheckpointError> {
        Ok(())
    }

    async fn finish(&self, _: &[DurableExecution]) -> Result<(), CheckpointError> {
        Ok(())
    }

    async fn discard(&self) -> Result<(), CheckpointError> {
        self.entered.notify_one();
        self.release.notified().await;
        std::fs::remove_dir_all(&self.files)?;
        Ok(())
    }
}

#[tokio::test]
async fn successful_controller_waits_for_private_checkpoint_disposal_before_exit() {
    let root = tempfile::tempdir().assert_value();
    let (paths, ledger) = observer_storage(root.path());
    let run_id = RunId::new("successful-checkpoint-disposal");
    let lease = Arc::new(ControllerLease::acquire(paths.lease()).assert_value());
    let files = private_directory(root.path(), "conversations");
    std::fs::write(files.join("transcript"), "private provider bytes").assert_value();
    let store = Arc::new(HeldDiscard {
        files: files.clone(),
        entered: Default::default(),
        release: Default::default(),
    });
    let allocator = Arc::new(SingleRunAllocator::new(
        run_id.clone(),
        Some(PortableRuntime {
            checkpoints: Some(store.clone()),
            ..PortableRuntime::new(Arc::new(super::super::super::tests::NeverDispatched))
        }),
        lease.clone(),
        paths.storage().join("checkpoints"),
    ));
    let inner = Arc::new(
        NativeV2PlatformController::new_with_delivery_policy(
            ledger,
            allocator,
            crate::native_v2_admission::DeliveryPolicy::Optional,
        )
        .await
        .assert_value()
        .with_run_sizes(crate::native_v2_hosting::host_run_sizes()),
    );
    inner
        .submit(RunSubmitParams {
            run_id: run_id.clone(),
            submission: super::super::super::tests::submission("successful-checkpoint-disposal"),
        })
        .await
        .expect("submit terminal-only checkpoint-disposal run");
    let controller = PortableRunController {
        run_id,
        inner,
        paths,
        _lease: lease,
        _workspace_lease: None,
    };
    tokio::time::timeout(Duration::from_secs(2), store.entered.notified())
        .await
        .assert_value();
    let waiting = controller.wait_terminal();
    tokio::pin!(waiting);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut waiting)
            .await
            .is_err(),
        "controller exited while private checkpoint files remained"
    );
    assert!(files.join("transcript").exists());
    store.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .assert_value()
        .assert_value();
    assert!(!files.exists());
}
