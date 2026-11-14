use super::*;
use crate::full_v1_reducer::ExecutionBoundary;
use crate::native_v2_contract::DeliveryCheckpoint;
use crate::native_v2_supervisor::checkpoints::{CheckpointError, RunCheckpointStore};
use openengine_cluster_protocol::{RunVector, RunVectorContent, RunVectorsParams};
use tokio::sync::Notify;

#[derive(Default)]
struct PublicationStore {
    vectors: StdMutex<Vec<RunVector>>,
    history: StdMutex<Vec<DurableExecution>>,
    published: Notify,
    publish_release: Notify,
    disposing: Notify,
    disposal_release: Notify,
    reject: AtomicBool,
    exits: StdMutex<Vec<RunRuntimeExit>>,
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

async fn fixture() -> (Harness, Arc<PublicationStore>) {
    let (mut harness, _) = waiting(Some(Duration::from_secs(3600))).await;
    let store = Arc::new(PublicationStore::default());
    harness.supervisor = harness
        .supervisor
        .with_checkpoints(Some(store.clone()))
        .with_runtime_cleanup(store.clone());
    (harness, store)
}

async fn vectors(harness: &Harness) -> Vec<RunVector> {
    crate::v2_run_ledger::vectors::list_full(
        harness.ledger.as_ref(),
        RunVectorsParams {
            run_id: harness.supervisor.run_id.clone(),
            after: None,
            limit: Some(100),
        },
    )
    .await
    .assert_value()
    .vectors
}

async fn request(harness: &Harness) {
    tokio::time::timeout(Duration::from_secs(1), harness.supervisor.suspend())
        .await
        .assert_value()
        .assert_value();
}

#[tokio::test]
async fn request_during_host_save_is_republished_before_disposal() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (harness, store) = fixture().await;
        let task = harness.spawn_drive();
        store.published.notified().await;
        request(&harness).await;
        request(&harness).await;
        assert_eq!(vectors(&harness).await.len(), 2);
        assert_eq!(store.vectors.lock().assert_value().len(), 2);

        store.publish_release.notify_one();
        store.published.notified().await;
        assert_eq!(store.vectors.lock().assert_value().len(), 3);
        assert!(store.exits.lock().assert_value().is_empty());
        request(&harness).await;
        assert_eq!(vectors(&harness).await.len(), 2);
        store.publish_release.notify_one();
        store.disposing.notified().await;
        store.disposal_release.notify_one();
        assert!(matches!(
            task.await.assert_value(),
            Err(NativeV2SupervisorError::Suspended)
        ));

        let durable = vectors(&harness).await;
        assert_eq!(*store.vectors.lock().assert_value(), durable);
        assert!(matches!(
            durable.as_slice(),
            [
                RunVector {
                    content: RunVectorContent::Wait { .. },
                    ..
                },
                RunVector {
                    content: RunVectorContent::SuspendRequest {},
                    ..
                },
                RunVector {
                    content: RunVectorContent::Suspension { .. },
                    ..
                }
            ]
        ));
        let history = continuation_history(&harness.supervisor.snapshot().await.assert_value())
            .assert_value();
        assert_eq!(*store.history.lock().assert_value(), history);
        assert_eq!(harness.driver.starts("after"), 0);
    })
    .await
    .assert_value();
}

#[tokio::test]
async fn final_disposal_fences_suspend_requests_without_blocking_force_stop() {
    tokio::time::timeout(Duration::from_secs(5), async {
        for force in [false, true] {
            let (harness, store) = fixture().await;
            let task = harness.spawn_drive();
            store.published.notified().await;
            store.publish_release.notify_one();
            store.disposing.notified().await;

            let request = harness.supervisor.suspend();
            tokio::pin!(request);
            assert!(futures_util::poll!(&mut request).is_pending());
            if force {
                tokio::time::timeout(Duration::from_secs(1), harness.supervisor.force_stop())
                    .await
                    .assert_value()
                    .assert_value();
            }
            store.disposal_release.notify_one();
            let result = task.await.assert_value();
            if force {
                assert!(matches!(
                    result.assert_value(),
                    TerminalResult::Failed { reason, .. } if reason.as_str() == "force_stopped"
                ));
                assert_eq!(
                    *store.exits.lock().assert_value(),
                    [RunRuntimeExit::Suspended, RunRuntimeExit::ForceStopped]
                );
            } else {
                assert!(matches!(result, Err(NativeV2SupervisorError::Suspended)));
                let durable = vectors(&harness).await;
                assert_eq!(*store.vectors.lock().assert_value(), durable);
            }
            assert!(request.await.is_err());
            assert!(
                !harness
                    .supervisor
                    .snapshot()
                    .await
                    .assert_value()
                    .suspend_requested
            );
        }
    })
    .await
    .assert_value();
}

#[tokio::test]
async fn failed_republication_keeps_the_runtime_and_durable_record_unsettled() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (harness, store) = fixture().await;
        let task = harness.spawn_drive();
        store.published.notified().await;
        request(&harness).await;
        store.publish_release.notify_one();
        store.published.notified().await;
        store.reject.store(true, Ordering::SeqCst);
        store.publish_release.notify_one();
        assert!(matches!(
            task.await.assert_value(),
            Err(NativeV2SupervisorError::Checkpoint(_))
        ));
        assert!(store.exits.lock().assert_value().is_empty());
        assert!(
            harness
                .supervisor
                .snapshot()
                .await
                .assert_value()
                .suspension
                .is_none()
        );
        assert_eq!(vectors(&harness).await.len(), 2);
    })
    .await
    .assert_value();
}
