use std::sync::{
    Mutex as StdMutex,
    atomic::{AtomicBool, Ordering},
};
use async_trait::async_trait;
use tokio::sync::Notify;
use openengine_cluster_protocol::RunVector;
use openengine_cluster_testkit::assertions::AssertValue;
use crate::full_v1_reducer::{DurableExecution, ExecutionBoundary};
use crate::native_v2_contract::DeliveryCheckpoint;
use crate::native_v2_supervisor::{RunRuntimeCleanup, RunRuntimeExit, RuntimeCleanupUnavailable};
use crate::native_v2_supervisor::checkpoints::{CheckpointError, RunCheckpointStore};

#[derive(Default)]
pub(crate) struct PublicationStore {
    pub(crate) vectors: StdMutex<Vec<RunVector>>,
    pub(crate) history: StdMutex<Vec<DurableExecution>>,
    pub(crate) published: Notify,
    pub(crate) publish_release: Notify,
    pub(crate) disposing: Notify,
    pub(crate) disposal_release: Notify,
    pub(crate) reject: AtomicBool,
    pub(crate) exits: StdMutex<Vec<RunRuntimeExit>>,
}

#[async_trait]
impl RunCheckpointStore for PublicationStore {
    async fn suspend(
        &self,
        history: &[DurableExecution],
        vectors: &[RunVector],
    ) -> Result<(), CheckpointError> {
        *self.history.lock().assert_value() = history.to_vec();
        *self.vectors.lock().assert_value() = vectors.to_vec();
        self.published.notify_one();
        self.publish_release.notified().await;
        if self.reject.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("rejected suspension publication").into());
        }
        Ok(())
    }

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
}

#[async_trait]
impl RunRuntimeCleanup for PublicationStore {
    async fn cleanup(&self, exit: RunRuntimeExit) -> Result<(), RuntimeCleanupUnavailable> {
        self.exits.lock().assert_value().push(exit);
        if exit == RunRuntimeExit::Suspended {
            self.disposing.notify_one();
            self.disposal_release.notified().await;
        }
        Ok(())
    }
}

#[async_trait]
impl crate::native_v2_platform::CapsuleCleanup for PublicationStore {
    async fn destroy_or_confirm_absent(
        &self,
        exit: RunRuntimeExit,
    ) -> Result<
        crate::native_v2_platform::CapsuleDestroyed,
        crate::native_v2_platform::CapsuleCleanupUnavailable,
    > {
        self.cleanup(exit)
            .await
            .map_err(|_| crate::native_v2_platform::CapsuleCleanupUnavailable)?;
        Ok(crate::native_v2_platform::CapsuleDestroyed::confirmed())
    }
}
