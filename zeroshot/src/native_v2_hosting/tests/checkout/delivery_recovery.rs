use super::*;
use crate::native_v2_runner::test_support::delivery_checkpoint;
use crate::native_v2_supervisor::checkpoints::{DeliveryCheckpointStore, RunCheckpointStore};
use crate::native_v2_supervisor::checkpoints::CheckpointError;

#[tokio::test]
async fn production_recovery_retains_delivery_facts_before_workspace_transfer() {
    let fixture = CheckoutFixture::new("").await;
    fixture.retain_untracked_workspace().await;
    let latest = delivery_checkpoint(json!({"owned":"verified revision"}), 2);
    fixture
        .allocator
        .retain_delivery_state(&fixture.run_id, &latest)
        .await
        .assert_value();
    fixture
        .allocator
        .retain_delivery_state(&fixture.run_id, &delivery_checkpoint(json!({}), 1))
        .await
        .assert_value();
    assert!(
        fixture
            .allocator
            .retain_delivery_state(&fixture.run_id, &delivery_checkpoint(json!({}), 2))
            .await
            .is_err()
    );
    let successor = RunId::new("receipt-recovery-successor");
    let capsule = fixture
        .allocator
        .allocate_from_retained(fixture.retained_request(
            &fixture.run_id,
            &successor,
            CheckpointRestoreSelection::Latest,
        ))
        .await
        .assert_value();
    assert_eq!(
        capsule
            .checkpoints
            .as_ref()
            .assert_value()
            .delivery_state()
            .await
            .assert_value(),
        Some(latest)
    );
    assert!(capsule.execution_seed.is_empty());
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();
}

struct ReceiptStorage {
    store: DeliveryCheckpointStore,
    expected_run: RunId,
}

#[async_trait::async_trait]
impl HostedWorkspaceStorage for ReceiptStorage {
    async fn retain_delivery_state(
        &self,
        run_id: &RunId,
        state: &crate::native_v2_contract::DeliveryCheckpoint,
    ) -> Result<(), CheckpointError> {
        assert_eq!(run_id, &self.expected_run);
        self.store.retain_delivery_state(state).await
    }

    async fn prepare(
        &self,
        _request: HostedWorkspaceRequest<'_>,
    ) -> Result<HostedWorkspace, CheckpointError> {
        panic!("receipt reconciliation precedes workspace allocation");
    }
}

#[tokio::test]
async fn external_recovery_reconciles_receipts_through_its_owned_storage() {
    let root = tempfile::tempdir().assert_value();
    let repository = root.path().join("external-lineage");
    let store = Arc::new(ReceiptStorage {
        store: DeliveryCheckpointStore::new(repository.clone()),
        expected_run: RunId::new("checkout-recovery"),
    });
    let fixture = CheckoutFixture::with_workspace_storage("", Some(store.clone())).await;
    let latest = delivery_checkpoint(json!({"owned":"verified revision"}), 2);
    fixture
        .allocator
        .retain_delivery_state(&fixture.run_id, &latest)
        .await
        .assert_value();
    assert_eq!(
        store.store.delivery_state().await.assert_value(),
        Some(latest)
    );
}
