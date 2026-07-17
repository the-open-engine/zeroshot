use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{CompiledGraphIr, GraphSpec};
use serde_json::json;
use zeroshot_engine::ledger::record::{
    LedgerRecord, RecordPayload, MAX_APPEND_RECORDS, MAX_DISCOVERY_RESOURCES, MAX_RANGE_RECORDS,
};
use zeroshot_engine::ledger::store::{
    AppendOutcome, AppendRequest, Clock, CoherentPrefix, LedgerStore, OpaqueMutationReceipt,
    ResourceMetadata, ResourcePage, StoreError, MAX_RECEIPT_BYTES,
};
use zeroshot_engine::ledger::{IdempotencyId, OwnerFence, OwnerId, Position, ResourceId};

#[derive(Debug, Default)]
pub struct ManualClock {
    now: AtomicU64,
    reads: AtomicU64,
    read_signal: (std::sync::Mutex<()>, std::sync::Condvar),
}

impl ManualClock {
    pub fn at(value: u64) -> Self {
        Self {
            now: AtomicU64::new(value),
            reads: AtomicU64::new(0),
            read_signal: (std::sync::Mutex::new(()), std::sync::Condvar::new()),
        }
    }

    pub fn advance(&self, millis: u64) {
        self.now.fetch_add(millis, Ordering::SeqCst);
    }

    pub fn read_count(&self) -> u64 {
        self.reads.load(Ordering::SeqCst)
    }

    pub fn wait_for_read_after(&self, count: u64, timeout: std::time::Duration) -> bool {
        let guard = self.read_signal.0.lock().unwrap();
        let (_guard, _) = self
            .read_signal
            .1
            .wait_timeout_while(guard, timeout, |_| self.read_count() <= count)
            .unwrap();
        self.read_count() > count
    }
}

impl Clock for ManualClock {
    fn now_unix_millis(&self) -> u64 {
        let now = self.now.load(Ordering::SeqCst);
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.read_signal.1.notify_all();
        now
    }
}

#[derive(Default)]
struct OneShotGate {
    armed: AtomicBool,
    reached: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl OneShotGate {
    fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    async fn block_if_armed(&self) {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.reached.notify_one();
            self.release.notified().await;
        }
    }

    async fn wait_until_reached(&self) {
        self.reached.notified().await;
    }

    fn release(&self) {
        self.release.notify_one();
    }
}

#[derive(Clone)]
pub struct DispatchRaceStore {
    inner: Arc<dyn LedgerStore>,
    prefix_gate: Arc<OneShotGate>,
    stale_prefix_gate: Arc<OneShotGate>,
    dispatch_append_gate: Arc<OneShotGate>,
}

impl DispatchRaceStore {
    pub fn new(inner: Arc<dyn LedgerStore>) -> Self {
        Self {
            inner,
            prefix_gate: Arc::new(OneShotGate::default()),
            stale_prefix_gate: Arc::new(OneShotGate::default()),
            dispatch_append_gate: Arc::new(OneShotGate::default()),
        }
    }

    pub fn arm_prefix(&self) {
        self.prefix_gate.arm();
    }

    pub async fn wait_for_prefix(&self) {
        self.prefix_gate.wait_until_reached().await;
    }

    pub fn release_prefix(&self) {
        self.prefix_gate.release();
    }

    pub fn arm_stale_prefix(&self) {
        self.stale_prefix_gate.arm();
    }

    pub async fn wait_for_stale_prefix(&self) {
        self.stale_prefix_gate.wait_until_reached().await;
    }

    pub fn release_stale_prefix(&self) {
        self.stale_prefix_gate.release();
    }

    pub fn arm_dispatch_append(&self) {
        self.dispatch_append_gate.arm();
    }

    pub async fn wait_for_dispatch_append(&self) {
        self.dispatch_append_gate.wait_until_reached().await;
    }

    pub fn release_dispatch_append(&self) {
        self.dispatch_append_gate.release();
    }
}

#[async_trait]
impl LedgerStore for DispatchRaceStore {
    async fn list_resources(
        &self,
        after: Option<&ResourceId>,
        limit: usize,
    ) -> Result<ResourcePage, StoreError> {
        self.inner.list_resources(after, limit).await
    }

    async fn create_resource(
        &self,
        resource_id: &ResourceId,
    ) -> Result<ResourceMetadata, StoreError> {
        self.inner.create_resource(resource_id).await
    }

    async fn open_resource(
        &self,
        resource_id: &ResourceId,
    ) -> Result<ResourceMetadata, StoreError> {
        self.inner.open_resource(resource_id).await
    }

    async fn acquire_fence(
        &self,
        resource_id: &ResourceId,
        owner: &OwnerId,
        ttl_millis: u64,
    ) -> Result<OwnerFence, StoreError> {
        self.inner
            .acquire_fence(resource_id, owner, ttl_millis)
            .await
    }

    async fn renew_fence(
        &self,
        resource_id: &ResourceId,
        fence: &OwnerFence,
        ttl_millis: u64,
    ) -> Result<OwnerFence, StoreError> {
        self.inner.renew_fence(resource_id, fence, ttl_millis).await
    }

    async fn validate_fence(
        &self,
        resource_id: &ResourceId,
        fence: &OwnerFence,
    ) -> Result<(), StoreError> {
        self.inner.validate_fence(resource_id, fence).await
    }

    async fn read_prefix(&self, resource_id: &ResourceId) -> Result<CoherentPrefix, StoreError> {
        self.prefix_gate.block_if_armed().await;
        let prefix = self.inner.read_prefix(resource_id).await?;
        self.stale_prefix_gate.block_if_armed().await;
        Ok(prefix)
    }

    async fn read_range(
        &self,
        resource_id: &ResourceId,
        after: Position,
        limit: usize,
    ) -> Result<CoherentPrefix, StoreError> {
        self.inner.read_range(resource_id, after, limit).await
    }

    async fn read_receipt(
        &self,
        resource_id: &ResourceId,
        key: &IdempotencyId,
    ) -> Result<Option<OpaqueMutationReceipt>, StoreError> {
        self.inner.read_receipt(resource_id, key).await
    }

    async fn compare_and_append(
        &self,
        resource_id: &ResourceId,
        request: AppendRequest,
    ) -> Result<AppendOutcome, StoreError> {
        if request
            .records
            .iter()
            .any(|record| record.kind == zeroshot_engine::ledger::record::RecordKind::Dispatch)
        {
            self.dispatch_append_gate.block_if_armed().await;
        }
        self.inner.compare_and_append(resource_id, request).await
    }

    async fn wait_for_advancement(
        &self,
        resource_id: &ResourceId,
        after: Position,
    ) -> Result<Position, StoreError> {
        self.inner.wait_for_advancement(resource_id, after).await
    }

    async fn remove_resource(
        &self,
        resource_id: &ResourceId,
        fence: &OwnerFence,
        expected_position: Position,
    ) -> Result<(), StoreError> {
        self.inner
            .remove_resource(resource_id, fence, expected_position)
            .await
    }
}

pub async fn run_store_contract(store: Arc<dyn LedgerStore>, clock: Arc<ManualClock>) {
    let resource = ResourceId::new("contract-resource").unwrap();
    assert_eq!(
        store.create_resource(&resource).await.unwrap().position,
        Position::ZERO
    );
    assert_eq!(
        store.create_resource(&resource).await.unwrap_err(),
        StoreError::AlreadyExists
    );
    assert_eq!(
        store.open_resource(&resource).await.unwrap().position,
        Position::ZERO
    );
    assert_eq!(
        store.list_resources(None, 1).await.unwrap().resources.len(),
        1
    );
    assert!(store.list_resources(None, 0).await.is_err());

    let owner = OwnerId::new("owner-a").unwrap();
    let mut fence = store.acquire_fence(&resource, &owner, 10).await.unwrap();
    let other = OwnerId::new("owner-b").unwrap();
    assert_eq!(
        store
            .acquire_fence(&resource, &other, 10)
            .await
            .unwrap_err(),
        StoreError::FenceRejected
    );
    clock.advance(1);
    let renewed = store.renew_fence(&resource, &fence, 10).await.unwrap();
    assert_eq!(renewed.epoch, fence.epoch);
    assert!(renewed.expires_at_unix_millis > fence.expires_at_unix_millis);
    assert_eq!(
        store.validate_fence(&resource, &fence).await.unwrap_err(),
        StoreError::FenceRejected
    );
    store.validate_fence(&resource, &renewed).await.unwrap();
    fence = renewed;
    assert!(
        store
            .list_resources(None, MAX_DISCOVERY_RESOURCES)
            .await
            .is_ok()
    );
    let payload = RecordPayload::LifecycleUpdate {
        labels: None,
        log_level: None,
        suspended: Some(false),
    };
    let record = LedgerRecord::new(
        resource.clone(),
        Position::new(1).unwrap(),
        &payload,
        [0; 32],
    )
    .unwrap();
    let receipt = OpaqueMutationReceipt {
        key: IdempotencyId::new("mutation-1").unwrap(),
        method: "contract".into(),
        fingerprint: [7; 32],
        value: b"receipt".to_vec(),
        at_position: Position::new(1).unwrap(),
    };
    let request = AppendRequest {
        expected_position: Position::ZERO,
        fence: fence.clone(),
        records: vec![record],
        receipt: Some(receipt.clone()),
    };
    let appended = store
        .compare_and_append(&resource, request.clone())
        .await
        .unwrap();
    assert_eq!(appended.position, Position::new(1).unwrap());
    assert!(!appended.replayed);
    let replayed = store.compare_and_append(&resource, request).await.unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.receipt, Some(receipt.clone()));
    assert_eq!(
        store.read_receipt(&resource, &receipt.key).await.unwrap(),
        Some(receipt)
    );
    let prefix = store.read_prefix(&resource).await.unwrap();
    assert_eq!(prefix.end, Position::new(1).unwrap());
    assert_eq!(prefix.records.len(), 1);
    assert_eq!(
        store
            .read_range(&resource, Position::ZERO, 1)
            .await
            .unwrap(),
        prefix
    );
    assert_eq!(
        store
            .wait_for_advancement(&resource, Position::ZERO)
            .await
            .unwrap(),
        Position::new(1).unwrap()
    );

    let second_payload = RecordPayload::LifecycleUpdate {
        labels: None,
        log_level: None,
        suspended: Some(true),
    };
    let second_record = LedgerRecord::new(
        resource.clone(),
        Position::new(2).unwrap(),
        &second_payload,
        prefix.records[0].record_hash,
    )
    .unwrap();
    assert_eq!(
        store
            .compare_and_append(
                &resource,
                AppendRequest {
                    expected_position: Position::new(1).unwrap(),
                    fence: fence.clone(),
                    records: vec![second_record.clone()],
                    receipt: Some(OpaqueMutationReceipt {
                        key: IdempotencyId::new("mutation-1").unwrap(),
                        method: "contract".into(),
                        fingerprint: [9; 32],
                        value: b"conflict".to_vec(),
                        at_position: Position::new(2).unwrap(),
                    }),
                },
            )
            .await
            .unwrap_err(),
        StoreError::ReceiptConflict
    );
    let second_receipt = OpaqueMutationReceipt {
        key: IdempotencyId::new("mutation-2").unwrap(),
        method: "contract".into(),
        fingerprint: [8; 32],
        value: b"receipt-2".to_vec(),
        at_position: Position::new(2).unwrap(),
    };
    assert_eq!(
        store
            .compare_and_append(
                &resource,
                AppendRequest {
                    expected_position: Position::ZERO,
                    fence: fence.clone(),
                    records: vec![second_record.clone()],
                    receipt: Some(second_receipt.clone()),
                },
            )
            .await
            .unwrap_err(),
        StoreError::PositionConflict {
            current: Position::new(1).unwrap()
        }
    );
    let waiter = {
        let store = store.clone();
        let resource = resource.clone();
        tokio::spawn(async move {
            store
                .wait_for_advancement(&resource, Position::new(1).unwrap())
                .await
        })
    };
    tokio::task::yield_now().await;
    store
        .compare_and_append(
            &resource,
            AppendRequest {
                expected_position: Position::new(1).unwrap(),
                fence: fence.clone(),
                records: vec![second_record],
                receipt: Some(second_receipt),
            },
        )
        .await
        .unwrap();
    assert_eq!(waiter.await.unwrap().unwrap(), Position::new(2).unwrap());

    exercise_exact_upper_bounds(store.clone()).await;

    clock.advance(11);
    assert_eq!(
        store.validate_fence(&resource, &fence).await.unwrap_err(),
        StoreError::FenceRejected
    );
    let takeover = store.acquire_fence(&resource, &other, 10).await.unwrap();
    assert!(takeover.epoch > fence.epoch);
    assert_eq!(
        store
            .remove_resource(&resource, &fence, Position::new(2).unwrap())
            .await
            .unwrap_err(),
        StoreError::FenceRejected
    );
    store
        .remove_resource(&resource, &takeover, Position::new(2).unwrap())
        .await
        .unwrap();
    assert_eq!(
        store.open_resource(&resource).await.unwrap_err(),
        StoreError::NotFound
    );
}

async fn exercise_exact_upper_bounds(store: Arc<dyn LedgerStore>) {
    let resource = ResourceId::new("contract-upper-bounds").unwrap();
    store.create_resource(&resource).await.unwrap();
    let fence = store
        .acquire_fence(&resource, &OwnerId::new("bounds-owner").unwrap(), 100)
        .await
        .unwrap();
    let mut records = Vec::with_capacity(MAX_APPEND_RECORDS);
    let mut previous_hash = [0; 32];
    for sequence in 1..=MAX_APPEND_RECORDS {
        let record = LedgerRecord::new(
            resource.clone(),
            Position::new(u64::try_from(sequence).unwrap()).unwrap(),
            &RecordPayload::LifecycleUpdate {
                labels: None,
                log_level: None,
                suspended: Some(sequence % 2 == 0),
            },
            previous_hash,
        )
        .unwrap();
        previous_hash = record.record_hash;
        records.push(record);
    }
    let outcome = store
        .compare_and_append(
            &resource,
            AppendRequest {
                expected_position: Position::ZERO,
                fence: fence.clone(),
                records,
                receipt: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(outcome.position.get(), MAX_APPEND_RECORDS as u64);
    let prefix = store.read_prefix(&resource).await.unwrap();
    let mut too_many = prefix.records.clone();
    too_many.push(prefix.records.last().unwrap().clone());
    assert!(matches!(
        store
            .compare_and_append(
                &resource,
                AppendRequest {
                    expected_position: outcome.position,
                    fence: fence.clone(),
                    records: too_many,
                    receipt: None,
                },
            )
            .await,
        Err(StoreError::BoundExceeded(_))
    ));
    assert_eq!(
        store.open_resource(&resource).await.unwrap().position,
        outcome.position
    );
    assert_eq!(
        store
            .read_range(&resource, Position::ZERO, MAX_RANGE_RECORDS)
            .await
            .unwrap()
            .records
            .len(),
        MAX_APPEND_RECORDS
    );
    assert!(matches!(
        store
            .read_range(&resource, Position::ZERO, MAX_RANGE_RECORDS + 1)
            .await,
        Err(StoreError::BoundExceeded(_))
    ));

    let receipt_resource = ResourceId::new("contract-max-receipt").unwrap();
    store.create_resource(&receipt_resource).await.unwrap();
    let receipt_fence = store
        .acquire_fence(
            &receipt_resource,
            &OwnerId::new("receipt-owner").unwrap(),
            100,
        )
        .await
        .unwrap();
    let record = LedgerRecord::new(
        receipt_resource.clone(),
        Position::new(1).unwrap(),
        &RecordPayload::LifecycleUpdate {
            labels: None,
            log_level: None,
            suspended: Some(false),
        },
        [0; 32],
    )
    .unwrap();
    let receipt = OpaqueMutationReceipt {
        key: IdempotencyId::new("max-receipt").unwrap(),
        method: "contract".into(),
        fingerprint: [9; 32],
        value: vec![0; MAX_RECEIPT_BYTES],
        at_position: Position::new(1).unwrap(),
    };
    let outcome = store
        .compare_and_append(
            &receipt_resource,
            AppendRequest {
                expected_position: Position::ZERO,
                fence: receipt_fence,
                records: vec![record],
                receipt: Some(receipt.clone()),
            },
        )
        .await
        .unwrap();
    assert_eq!(outcome.receipt, Some(receipt));
    let oversized = OpaqueMutationReceipt {
        key: IdempotencyId::new("oversized-receipt").unwrap(),
        method: "contract".into(),
        fingerprint: [10; 32],
        value: vec![0; MAX_RECEIPT_BYTES + 1],
        at_position: Position::new(2).unwrap(),
    };
    assert!(matches!(
        store
            .compare_and_append(
                &receipt_resource,
                AppendRequest {
                    expected_position: outcome.position,
                    fence: store
                        .acquire_fence(
                            &receipt_resource,
                            &OwnerId::new("receipt-owner").unwrap(),
                            100,
                        )
                        .await
                        .unwrap(),
                    records: vec![
                        LedgerRecord::new(
                            receipt_resource.clone(),
                            Position::new(2).unwrap(),
                            &RecordPayload::LifecycleUpdate {
                                labels: None,
                                log_level: None,
                                suspended: Some(true),
                            },
                            store.read_prefix(&receipt_resource).await.unwrap().records[0]
                                .record_hash,
                        )
                        .unwrap()
                    ],
                    receipt: Some(oversized),
                },
            )
            .await,
        Err(StoreError::BoundExceeded(_))
    ));
    assert_eq!(
        store
            .open_resource(&receipt_resource)
            .await
            .unwrap()
            .position,
        outcome.position
    );
}

pub fn graph_and_ir() -> (GraphSpec, CompiledGraphIr) {
    let graph = json!({
        "profile":"openengine.graph.single-worker/v1",
        "initialInput":{"kind":"record","fields":{}},
        "policy":{"policy":"policy.default@1","default":"deny"},
        "root":{
            "kind":"step","name":"work","worker":"legacy.zeroshot.ship@1",
            "input":{"kind":"record","fields":{}},"output":{"kind":"null"},
            "inputBindings":[],"writeBindings":[],"timeoutMs":1000,"attempts":1
        }
    });
    let mut compiled = graph.clone();
    compiled["bounds"] = json!({
        "termination":{"kind":"acyclic","order":["work"]},
        "maxNodeExecutions":1,"peakConcurrency":1,"attemptsPerNode":{"work":1}
    });
    (
        serde_json::from_value(graph).unwrap(),
        serde_json::from_value(compiled).unwrap(),
    )
}
