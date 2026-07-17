mod support;

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use openengine_cluster_protocol::{
    admission_fingerprint, Generation, IdempotencyKey, RequestFingerprint, StopMode, StopParams,
};
use openengine_cluster_server::admission::{
    AdmissionStore, CancellationSignal, CommitProposal, StoreError as AdmissionStoreError,
};
use openengine_cluster_server::lifecycle::{LifecycleStore, StopProposal, TurnId, VerifiedCompletion};
use serde_json::json;
use support::ledger::{graph_and_ir, ManualClock};
use zeroshot_engine::fault::{EvidenceClass, FaultContext, FaultFactory, FaultModule, ModuleEvidence};
use zeroshot_engine::ledger::store::{
    AppendOutcome, AppendRequest, CoherentPrefix, LedgerStore, OpaqueMutationReceipt,
    ResourceMetadata, ResourcePage, StoreError,
};
use zeroshot_engine::ledger::{
    AbsoluteDeadline, AdmissionRequest, ClusterLedger, DispatchRequest, IdempotencyId,
    MemoryLedgerStore, MutationIdentity, OwnerFence, OwnerId, Position, ResourceId,
    SettlementRequest, TerminalOutcome,
};
use zeroshot_engine::ledger::adapters::LedgerAdapters;
use zeroshot_engine::observability::NoopObservationSink;

const NONE: u8 = 0;
const BEFORE_APPEND: u8 = 1;
const AFTER_APPEND: u8 = 2;
const BEFORE_REMOVE: u8 = 3;
const AFTER_REMOVE: u8 = 4;

struct FailpointStore {
    inner: Arc<dyn LedgerStore>,
    failpoint: AtomicU8,
    cancel_after_prefix_reads: AtomicU8,
    cancellation: Mutex<Option<CancellationSignal>>,
}

impl FailpointStore {
    fn new(inner: Arc<dyn LedgerStore>) -> Self {
        Self {
            inner,
            failpoint: AtomicU8::new(NONE),
            cancel_after_prefix_reads: AtomicU8::new(0),
            cancellation: Mutex::new(None),
        }
    }

    fn fail_once(&self, failpoint: u8) {
        self.failpoint.store(failpoint, Ordering::SeqCst);
    }

    fn cancel_after_prefix_reads(&self, reads: u8, cancellation: CancellationSignal) {
        *self.cancellation.lock().unwrap() = Some(cancellation);
        self.cancel_after_prefix_reads
            .store(reads, Ordering::SeqCst);
    }
}

#[async_trait]
impl LedgerStore for FailpointStore {
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
        let prefix = self.inner.read_prefix(resource_id).await?;
        let remaining = self.cancel_after_prefix_reads.load(Ordering::SeqCst);
        if remaining > 0
            && self
                .cancel_after_prefix_reads
                .fetch_sub(1, Ordering::SeqCst)
                == 1
        {
            if let Some(cancellation) = self.cancellation.lock().unwrap().take() {
                cancellation.cancel();
            }
        }
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
        match self.failpoint.swap(NONE, Ordering::SeqCst) {
            BEFORE_APPEND => Err(StoreError::StorageUnavailable),
            AFTER_APPEND => {
                self.inner.compare_and_append(resource_id, request).await?;
                Err(StoreError::StorageUnavailable)
            }
            _ => self.inner.compare_and_append(resource_id, request).await,
        }
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
        match self.failpoint.swap(NONE, Ordering::SeqCst) {
            BEFORE_REMOVE => Err(StoreError::StorageUnavailable),
            AFTER_REMOVE => {
                self.inner
                    .remove_resource(resource_id, fence, expected_position)
                    .await?;
                Err(StoreError::StorageUnavailable)
            }
            _ => {
                self.inner
                    .remove_resource(resource_id, fence, expected_position)
                    .await
            }
        }
    }
}

#[tokio::test]
async fn admission_cancellation_is_rechecked_at_the_atomic_append_boundary() {
    let clock = Arc::new(ManualClock::at(100));
    let inner = Arc::new(MemoryLedgerStore::new(clock));
    let store = Arc::new(FailpointStore::new(inner));
    let ledger = Arc::new(
        ClusterLedger::create(
            store.clone(),
            ResourceId::new("cancelled-admission").unwrap(),
            OwnerId::new("owner").unwrap(),
            1000,
        )
        .await
        .unwrap(),
    );
    let adapters = LedgerAdapters::new(ledger.clone());
    let cancellation = CancellationSignal::default();
    store.cancel_after_prefix_reads(2, cancellation.clone());
    let (graph, compiled_ir) = graph_and_ir();
    let fingerprint: RequestFingerprint = serde_json::from_value(json!("a".repeat(64))).unwrap();
    let result = adapters
        .commit(
            CommitProposal {
                graph,
                compiled_ir,
                input: Some(json!({})),
                if_generation: None,
                idempotency_key: IdempotencyKey::new("cancelled-apply").unwrap(),
                fingerprint,
            },
            &cancellation,
        )
        .await;
    assert_eq!(result.unwrap_err(), AdmissionStoreError::Cancelled);
    assert_eq!(ledger.replay().await.unwrap().at_position, Position::ZERO);
}

#[tokio::test]
async fn terminal_drain_settlement_recovers_after_commit_before_response() {
    let clock = Arc::new(ManualClock::at(100));
    let inner = Arc::new(MemoryLedgerStore::new(clock));
    let failpoints = Arc::new(FailpointStore::new(inner));
    let ledger = Arc::new(
        ClusterLedger::create(
            failpoints.clone(),
            ResourceId::new("terminal-settlement-recovery").unwrap(),
            OwnerId::new("owner").unwrap(),
            1000,
        )
        .await
        .unwrap(),
    );
    let (graph, compiled_ir) = graph_and_ir();
    ledger
        .admit(AdmissionRequest {
            graph,
            compiled_ir,
            input: json!({}),
            deadline: AbsoluteDeadline::from_unix_millis(10_000).unwrap(),
            mutation: mutation("admit-terminal", "admit", json!({"graph":"fixture"})),
        })
        .await
        .unwrap();
    let adapters = LedgerAdapters::new(ledger.clone());
    let permit = adapters
        .acquire_dispatch(TurnId::new("draining-turn"))
        .await
        .unwrap();
    let params = StopParams {
        mode: StopMode::Drain,
        if_generation: Generation::new(1).unwrap(),
        idempotency_key: IdempotencyKey::new("drain").unwrap(),
    };
    let fingerprint =
        admission_fingerprint("stop", &serde_json::to_value(&params).unwrap()).unwrap();
    adapters
        .stop_lifecycle(StopProposal {
            params,
            fingerprint,
        })
        .await
        .unwrap();

    failpoints.fail_once(AFTER_APPEND);
    let first = adapters
        .complete_dispatch(VerifiedCompletion {
            lease_id: permit.lease_id.clone(),
            output: json!({"ok":true}),
        })
        .await;
    assert_eq!(first.unwrap_err(), AdmissionStoreError::CompletionRejected);
    let committed_position = ledger.replay().await.unwrap().at_position;
    let recovered = adapters
        .complete_dispatch(VerifiedCompletion {
            lease_id: permit.lease_id,
            output: json!({"ok":true}),
        })
        .await
        .unwrap();
    assert!(recovered.terminalized);
    assert_eq!(recovered.turn_id.as_str(), "draining-turn");
    assert_eq!(
        recovered.at_cursor.as_str(),
        format!("ledger-{}", committed_position.get())
    );
    assert_eq!(
        ledger.replay().await.unwrap().at_position,
        committed_position
    );
}

fn mutation(key: &str, method: &str, value: serde_json::Value) -> MutationIdentity {
    MutationIdentity::for_value(IdempotencyId::new(key).unwrap(), method, &value).unwrap()
}

#[tokio::test]
async fn before_commit_writes_nothing_and_after_commit_recovers_every_mutation_receipt() {
    let clock = Arc::new(ManualClock::at(100));
    let inner = Arc::new(MemoryLedgerStore::new(clock));
    let failpoints = Arc::new(FailpointStore::new(inner));
    let resource = ResourceId::new("crash-cluster").unwrap();
    let ledger = ClusterLedger::create(
        failpoints.clone(),
        resource,
        OwnerId::new("owner").unwrap(),
        1000,
    )
    .await
    .unwrap();
    let (graph, compiled_ir) = graph_and_ir();
    let admission = AdmissionRequest {
        graph,
        compiled_ir,
        input: json!({}),
        deadline: AbsoluteDeadline::from_unix_millis(10_000).unwrap(),
        mutation: mutation("admit", "admit", json!({"kind":"admission"})),
    };
    failpoints.fail_once(BEFORE_APPEND);
    assert!(ledger.admit(admission.clone()).await.is_err());
    assert_eq!(ledger.replay().await.unwrap().at_position, Position::ZERO);
    failpoints.fail_once(AFTER_APPEND);
    assert!(ledger.admit(admission.clone()).await.is_err());
    assert!(ledger.admit(admission).await.unwrap().deduped);

    let dispatch_request = DispatchRequest {
        turn_id: "turn".into(),
        mutation: mutation("dispatch", "dispatch", json!({"kind":"dispatch"})),
    };
    let before_dispatch = ledger.replay().await.unwrap().at_position;
    failpoints.fail_once(BEFORE_APPEND);
    assert!(ledger.dispatch(dispatch_request.clone()).await.is_err());
    assert_eq!(ledger.replay().await.unwrap().at_position, before_dispatch);
    failpoints.fail_once(AFTER_APPEND);
    assert!(ledger.dispatch(dispatch_request.clone()).await.is_err());
    let dispatch = ledger.dispatch(dispatch_request).await.unwrap();
    assert!(dispatch.deduped);

    let settlement = SettlementRequest {
        execution_id: dispatch.execution_id.clone(),
        output: json!({"ok":true}),
        mutation: mutation("settle", "settle", json!({"kind":"settlement"})),
    };
    let before_settlement = ledger.replay().await.unwrap().at_position;
    failpoints.fail_once(BEFORE_APPEND);
    assert!(ledger.settle(settlement.clone()).await.is_err());
    assert_eq!(
        ledger.replay().await.unwrap().at_position,
        before_settlement
    );
    failpoints.fail_once(AFTER_APPEND);
    assert!(ledger.settle(settlement.clone()).await.is_err());
    assert!(ledger.settle(settlement).await.unwrap().deduped);

    let next_dispatch = ledger
        .dispatch(DispatchRequest {
            turn_id: "effect-turn".into(),
            mutation: mutation(
                "effect-dispatch",
                "dispatch",
                json!({"kind":"effect-dispatch"}),
            ),
        })
        .await
        .unwrap();
    let effect_intent = mutation("effect-intent", "effect_intent", json!({"kind":"intent"}));
    let before_intent = ledger.replay().await.unwrap().at_position;
    failpoints.fail_once(BEFORE_APPEND);
    assert!(
        ledger
            .record_effect_intent(
                next_dispatch.execution_id.clone(),
                "effect".into(),
                "a".repeat(64),
                effect_intent.clone(),
            )
            .await
            .is_err()
    );
    assert_eq!(ledger.replay().await.unwrap().at_position, before_intent);
    failpoints.fail_once(AFTER_APPEND);
    assert!(
        ledger
            .record_effect_intent(
                next_dispatch.execution_id.clone(),
                "effect".into(),
                "a".repeat(64),
                effect_intent.clone(),
            )
            .await
            .is_err()
    );
    assert!(
        ledger
            .record_effect_intent(
                next_dispatch.execution_id,
                "effect".into(),
                "a".repeat(64),
                effect_intent,
            )
            .await
            .unwrap()
            .deduped
    );
    let effect_receipt = mutation(
        "effect-receipt",
        "effect_receipt",
        json!({"kind":"receipt"}),
    );
    let before_effect_receipt = ledger.replay().await.unwrap().at_position;
    failpoints.fail_once(BEFORE_APPEND);
    assert!(
        ledger
            .reconcile_effect("effect".into(), "b".repeat(64), effect_receipt.clone())
            .await
            .is_err()
    );
    assert_eq!(
        ledger.replay().await.unwrap().at_position,
        before_effect_receipt
    );
    failpoints.fail_once(AFTER_APPEND);
    assert!(
        ledger
            .reconcile_effect("effect".into(), "b".repeat(64), effect_receipt.clone())
            .await
            .is_err()
    );
    assert!(
        ledger
            .reconcile_effect("effect".into(), "b".repeat(64), effect_receipt)
            .await
            .unwrap()
            .deduped
    );

    let fault = FaultFactory::new(&NoopObservationSink).create(ModuleEvidence::new(
        FaultModule::Engine,
        FaultContext::Execution,
        EvidenceClass::InvariantViolation,
    ));
    let safe_fault = mutation("safe-fault", "safe_fault", json!({"kind":"fault"}));
    let before_safe_fault = ledger.replay().await.unwrap().at_position;
    failpoints.fail_once(BEFORE_APPEND);
    assert!(
        ledger
            .persist_safe_fault(None, &fault, TerminalOutcome::Failed, safe_fault.clone())
            .await
            .is_err()
    );
    assert_eq!(
        ledger.replay().await.unwrap().at_position,
        before_safe_fault
    );
    failpoints.fail_once(AFTER_APPEND);
    assert!(
        ledger
            .persist_safe_fault(None, &fault, TerminalOutcome::Failed, safe_fault.clone())
            .await
            .is_err()
    );
    assert!(
        ledger
            .persist_safe_fault(None, &fault, TerminalOutcome::Failed, safe_fault)
            .await
            .unwrap()
            .deduped
    );
    assert_eq!(
        ledger.replay().await.unwrap().terminal_outcome,
        Some(TerminalOutcome::Failed)
    );
}

#[tokio::test]
async fn removal_failpoints_preserve_terminal_only_atomic_product_view() {
    let clock = Arc::new(ManualClock::at(100));
    let inner = Arc::new(MemoryLedgerStore::new(clock));
    let failpoints = Arc::new(FailpointStore::new(inner.clone()));
    let resource = ResourceId::new("remove-cluster").unwrap();
    let ledger = ClusterLedger::create(
        failpoints.clone(),
        resource.clone(),
        OwnerId::new("owner").unwrap(),
        1000,
    )
    .await
    .unwrap();
    let (graph, compiled_ir) = graph_and_ir();
    ledger
        .admit(AdmissionRequest {
            graph,
            compiled_ir,
            input: json!({}),
            deadline: AbsoluteDeadline::from_unix_millis(10_000).unwrap(),
            mutation: mutation("admit", "admit", json!({"kind":"admission"})),
        })
        .await
        .unwrap();
    let terminal = mutation("terminal", "terminalize", json!({"kind":"terminal"}));
    let before_terminal = ledger.replay().await.unwrap().at_position;
    failpoints.fail_once(BEFORE_APPEND);
    assert!(
        ledger
            .terminalize(TerminalOutcome::Succeeded, terminal.clone())
            .await
            .is_err()
    );
    assert_eq!(ledger.replay().await.unwrap().at_position, before_terminal);
    failpoints.fail_once(AFTER_APPEND);
    assert!(
        ledger
            .terminalize(TerminalOutcome::Succeeded, terminal.clone())
            .await
            .is_err()
    );
    assert!(
        ledger
            .terminalize(TerminalOutcome::Succeeded, terminal)
            .await
            .unwrap()
            .deduped
    );
    failpoints.fail_once(BEFORE_REMOVE);
    assert!(ledger.remove_terminal().await.is_err());
    assert!(inner.open_resource(&resource).await.is_ok());
    failpoints.fail_once(AFTER_REMOVE);
    assert!(ledger.remove_terminal().await.is_err());
    assert_eq!(
        inner.open_resource(&resource).await.unwrap_err(),
        StoreError::NotFound
    );
}
