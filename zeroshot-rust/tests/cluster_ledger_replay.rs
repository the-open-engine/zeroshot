mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use support::ledger::{key, owner, resource, temp_root};
use tokio::sync::Barrier;
use openengine_cluster_server::admission::{AdmissionStore, ControlJournal, VerifiedIoLedger};
use openengine_cluster_server::admission::{CancellationSignal, CommitProposal};
use openengine_cluster_server::lifecycle::LifecycleStore;
use zeroshot_engine::cluster_ledger::adapters::ClusterLedgerAdapters;
use zeroshot_engine::cluster_ledger::mutations::AdmissionRequest;
use zeroshot_engine::cluster_ledger::mutations::SafeFaultConsequence;
use zeroshot_engine::cluster_ledger::record::{
    CanonicalDigest, RecordKind, RecordPayload, StoredRecord, MAX_APPEND_RECORDS,
    MAX_RECORD_PAYLOAD_BYTES,
};
use zeroshot_engine::cluster_ledger::replay::{replay, ReplayError};
use zeroshot_engine::cluster_ledger::store::fake::{FakeLedgerStore, ManualLedgerClock};
use zeroshot_engine::cluster_ledger::store::sqlite::SqliteLedgerStore;
use zeroshot_engine::cluster_ledger::store::{
    AppendBatch, AppendGuard, AppendOutcome, DiscoveryPage, Fence, IdempotencyId, LedgerStore,
    MutationReceipt, OwnerId, Position, PrefixSnapshot, ResourceId, ResourceInfo, StoreError,
    MAX_DISCOVERY_PAGE, MAX_IDENTIFIER_BYTES,
};
use zeroshot_engine::cluster_ledger::{ClusterLedger, LedgerErrorKind};
use zeroshot_engine::fault::{
    EvidenceClass, FaultContext, FaultFactory, FaultModule, ModuleEvidence, RawDiagnostic,
    RedactionMarker,
};
use zeroshot_engine::observability::NoopObservationSink;

fn admission(label: &[u8]) -> AdmissionRequest {
    let input = br#"{"task":"verified"}"#.to_vec();
    AdmissionRequest {
        graph_digest: CanonicalDigest::of(label),
        input_digest: CanonicalDigest::of(&input),
        policy_digest: CanonicalDigest::of(b"policy"),
        catalog_digest: CanonicalDigest::of(b"catalog"),
        profile_digest: CanonicalDigest::of(b"profile"),
        absolute_deadline_ms: 100_000,
        verified_input: input,
        canonical_graph: label.to_vec(),
        canonical_compiled_ir: br#"{"ir":"verified"}"#.to_vec(),
    }
}

fn replace_payload(snapshot: &mut PrefixSnapshot, index: usize, payload: RecordPayload) {
    let previous_hash = index
        .checked_sub(1)
        .map_or([0; 32], |previous| snapshot.records[previous].record_hash);
    let sequence = snapshot.records[index].sequence;
    snapshot.records[index] = StoredRecord::build(
        snapshot.records[index].resource.clone(),
        sequence,
        &payload,
        previous_hash,
    )
    .unwrap();
    for next in index + 1..snapshot.records.len() {
        let existing = RecordPayload::decode(
            snapshot.records[next].kind,
            snapshot.records[next].version,
            &snapshot.records[next].payload,
        )
        .unwrap();
        snapshot.records[next] = StoredRecord::build(
            snapshot.records[next].resource.clone(),
            snapshot.records[next].sequence,
            &existing,
            snapshot.records[next - 1].record_hash,
        )
        .unwrap();
    }
}

async fn ledger(label: &str) -> (Arc<dyn LedgerStore>, ClusterLedger) {
    let store: Arc<dyn LedgerStore> = Arc::new(FakeLedgerStore::new(ManualLedgerClock::new(1_000)));
    let ledger = ClusterLedger::create(
        Arc::clone(&store),
        resource(label),
        owner("replay-owner"),
        10_000,
    )
    .await
    .unwrap();
    (store, ledger)
}

#[tokio::test]
async fn failed_creation_never_leaves_a_durable_resource() {
    let store = Arc::new(FakeLedgerStore::new(ManualLedgerClock::new(1_000)));
    for (label, ttl) in [("zero-ttl", 0), ("overflow-ttl", u64::MAX)] {
        let resource = resource(label);
        assert!(
            ClusterLedger::create(store.clone(), resource.clone(), owner("owner"), ttl)
                .await
                .is_err()
        );
        assert!(matches!(
            store.open(&resource).await,
            Err(StoreError::ResourceNotFound)
        ));
    }
}

async fn assert_concurrent_identical_mutation_marks_one_replayed(
    inner: Arc<dyn LedgerStore>,
    resource_id: &str,
) {
    let racing_store = Arc::new(SnapshotRaceStore::new(inner));
    let store: Arc<dyn LedgerStore> = racing_store.clone();
    let ledger = ClusterLedger::create(store, resource(resource_id), owner("owner"), 10_000)
        .await
        .unwrap();
    racing_store.arm();

    let first_ledger = ledger.clone();
    let first = tokio::spawn(async move {
        first_ledger
            .admit(key("same-key"), [1; 32], admission(b"same-admission"))
            .await
            .unwrap()
    });
    let second_ledger = ledger.clone();
    let second = tokio::spawn(async move {
        second_ledger
            .admit(key("same-key"), [1; 32], admission(b"same-admission"))
            .await
            .unwrap()
    });
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();

    assert_ne!(first.replayed, second.replayed);
    assert_eq!(first.value, second.value);
    assert_eq!(first.position, second.position);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_identical_fake_mutation_marks_exactly_one_result_replayed() {
    let store: Arc<dyn LedgerStore> = Arc::new(FakeLedgerStore::new(ManualLedgerClock::new(1_000)));
    assert_concurrent_identical_mutation_marks_one_replayed(store, "concurrent-fake-idempotency")
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_identical_sqlite_mutation_marks_exactly_one_result_replayed() {
    let root = temp_root("concurrent-sqlite-idempotency");
    let store: Arc<dyn LedgerStore> =
        Arc::new(SqliteLedgerStore::with_clock(&root, ManualLedgerClock::new(1_000)).unwrap());
    assert_concurrent_identical_mutation_marks_one_replayed(store, "concurrent-sqlite-idempotency")
        .await;
    std::fs::remove_dir_all(root).unwrap();
}

#[derive(Clone)]
struct SnapshotRaceStore {
    inner: Arc<dyn LedgerStore>,
    gated_reads: Arc<AtomicUsize>,
    barrier: Arc<Barrier>,
    cancel_before_append: Arc<Mutex<Option<CancellationSignal>>>,
}

impl SnapshotRaceStore {
    fn new(inner: Arc<dyn LedgerStore>) -> Self {
        Self {
            inner,
            gated_reads: Arc::new(AtomicUsize::new(0)),
            barrier: Arc::new(Barrier::new(2)),
            cancel_before_append: Arc::new(Mutex::new(None)),
        }
    }

    fn arm(&self) {
        self.gated_reads.store(2, Ordering::Release);
    }

    fn cancel_before_next_append(&self, cancellation: CancellationSignal) {
        *self
            .cancel_before_append
            .lock()
            .expect("cancellation mutex must not be poisoned") = Some(cancellation);
    }
}

#[async_trait]
impl LedgerStore for SnapshotRaceStore {
    async fn discover(
        &self,
        after: Option<&ResourceId>,
        limit: usize,
    ) -> Result<DiscoveryPage, StoreError> {
        self.inner.discover(after, limit).await
    }

    async fn create(&self, resource: &ResourceId) -> Result<ResourceInfo, StoreError> {
        self.inner.create(resource).await
    }

    async fn create_fenced(
        &self,
        resource: &ResourceId,
        owner: &OwnerId,
        ttl_ms: u64,
    ) -> Result<(ResourceInfo, Fence), StoreError> {
        self.inner.create_fenced(resource, owner, ttl_ms).await
    }

    async fn open(&self, resource: &ResourceId) -> Result<ResourceInfo, StoreError> {
        self.inner.open(resource).await
    }

    async fn acquire_fence(
        &self,
        resource: &ResourceId,
        owner: &OwnerId,
        ttl_ms: u64,
    ) -> Result<Fence, StoreError> {
        self.inner.acquire_fence(resource, owner, ttl_ms).await
    }

    async fn renew_fence(&self, fence: &Fence, ttl_ms: u64) -> Result<Fence, StoreError> {
        self.inner.renew_fence(fence, ttl_ms).await
    }

    async fn check_fence(&self, fence: &Fence) -> Result<(), StoreError> {
        self.inner.check_fence(fence).await
    }

    async fn read_prefix(
        &self,
        resource: &ResourceId,
        through: Option<Position>,
    ) -> Result<PrefixSnapshot, StoreError> {
        let snapshot = self.inner.read_prefix(resource, through).await?;
        if self
            .gated_reads
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            self.barrier.wait().await;
        }
        Ok(snapshot)
    }

    async fn read_range(
        &self,
        resource: &ResourceId,
        after: Position,
        limit: usize,
    ) -> Result<Vec<StoredRecord>, StoreError> {
        self.inner.read_range(resource, after, limit).await
    }

    async fn compare_and_append(
        &self,
        resource: &ResourceId,
        fence: &Fence,
        expected: Position,
        batch: AppendBatch,
    ) -> Result<AppendOutcome, StoreError> {
        self.inner
            .compare_and_append(resource, fence, expected, batch)
            .await
    }

    async fn compare_and_append_guarded(
        &self,
        resource: &ResourceId,
        fence: &Fence,
        expected: Position,
        batch: AppendBatch,
        guard: AppendGuard,
    ) -> Result<AppendOutcome, StoreError> {
        if let Some(cancellation) = self
            .cancel_before_append
            .lock()
            .expect("cancellation mutex must not be poisoned")
            .take()
        {
            cancellation.cancel();
        }
        self.inner
            .compare_and_append_guarded(resource, fence, expected, batch, guard)
            .await
    }

    async fn lookup_receipt(
        &self,
        resource: &ResourceId,
        key: &IdempotencyId,
    ) -> Result<Option<MutationReceipt>, StoreError> {
        self.inner.lookup_receipt(resource, key).await
    }

    async fn wait_for_advance(
        &self,
        resource: &ResourceId,
        after: Position,
        deadline_ms: u64,
    ) -> Result<Position, StoreError> {
        self.inner
            .wait_for_advance(resource, after, deadline_ms)
            .await
    }

    async fn remove(
        &self,
        resource: &ResourceId,
        fence: &Fence,
        expected: Position,
    ) -> Result<(), StoreError> {
        self.inner.remove(resource, fence, expected).await
    }
}

async fn race_ledger(label: &str) -> (SnapshotRaceStore, ClusterLedger) {
    let inner: Arc<dyn LedgerStore> = Arc::new(FakeLedgerStore::new(ManualLedgerClock::new(1_000)));
    let race_store = SnapshotRaceStore::new(inner);
    let store: Arc<dyn LedgerStore> = Arc::new(race_store.clone());
    let ledger = ClusterLedger::create(store, resource(label), owner("race-owner"), 10_000)
        .await
        .unwrap();
    (race_store, ledger)
}

fn assert_one_cas_winner<T, E>(first: &Result<T, E>, second: &Result<T, E>) {
    assert_eq!(
        usize::from(first.is_ok()) + usize::from(second.is_ok()),
        1,
        "exactly one mutation may commit from one validated prefix"
    );
}

fn assert_protocol_store_adapters<T>()
where
    T: AdmissionStore + ControlJournal + VerifiedIoLedger + LifecycleStore,
{
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_admissions_cas_against_the_validated_prefix() {
    let (store, ledger) = race_ledger("race-admission").await;
    store.arm();
    let first_ledger = ledger.clone();
    let first = tokio::spawn(async move {
        first_ledger
            .admit(key("admit-a"), [1; 32], admission(b"graph-a"))
            .await
    });
    let second_ledger = ledger.clone();
    let second = tokio::spawn(async move {
        second_ledger
            .admit(key("admit-b"), [2; 32], admission(b"graph-b"))
            .await
    });
    let first = first.await.unwrap();
    let second = second.await.unwrap();

    assert_one_cas_winner(&first, &second);
    let state = ledger.state().await.unwrap();
    assert_eq!(state.identities.next_generation, 2);
    assert_eq!(state.identities.next_run, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_dispatches_cas_against_the_validated_prefix() {
    let (store, ledger) = race_ledger("race-dispatch").await;
    ledger
        .admit(key("admit"), [1; 32], admission(b"graph"))
        .await
        .unwrap();
    store.arm();
    let first_ledger = ledger.clone();
    let first =
        tokio::spawn(async move { first_ledger.dispatch(key("dispatch-a"), [2; 32]).await });
    let second_ledger = ledger.clone();
    let second =
        tokio::spawn(async move { second_ledger.dispatch(key("dispatch-b"), [3; 32]).await });
    let first = first.await.unwrap();
    let second = second.await.unwrap();

    assert_one_cas_winner(&first, &second);
    let state = ledger.state().await.unwrap();
    assert_eq!(state.active_dispatches.len(), 1);
    assert_eq!(state.identities.next_node_instance, 2);
    assert_eq!(state.identities.next_execution, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_settlements_cas_against_the_validated_prefix() {
    let (store, ledger) = race_ledger("race-settlement").await;
    ledger
        .admit(key("admit"), [1; 32], admission(b"graph"))
        .await
        .unwrap();
    let dispatch = ledger.dispatch(key("dispatch"), [2; 32]).await.unwrap();
    store.arm();
    let execution = dispatch.value.execution;
    let first_ledger = ledger.clone();
    let first = tokio::spawn(async move {
        first_ledger
            .settle(
                key("settle-a"),
                [3; 32],
                execution,
                CanonicalDigest::of(b"outcome-a"),
                None,
            )
            .await
    });
    let second_ledger = ledger.clone();
    let second = tokio::spawn(async move {
        second_ledger
            .settle(
                key("settle-b"),
                [4; 32],
                execution,
                CanonicalDigest::of(b"outcome-b"),
                None,
            )
            .await
    });
    let first = first.await.unwrap();
    let second = second.await.unwrap();

    assert_one_cas_winner(&first, &second);
    let state = ledger.state().await.unwrap();
    assert_eq!(state.settlements.len(), 1);
    assert!(state.active_dispatches.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_terminalizations_cas_against_the_validated_prefix() {
    let (store, ledger) = race_ledger("race-terminal").await;
    ledger
        .admit(key("admit"), [1; 32], admission(b"graph"))
        .await
        .unwrap();
    store.arm();
    let first_ledger = ledger.clone();
    let first = tokio::spawn(async move {
        first_ledger
            .terminalize(
                key("terminal-a"),
                [2; 32],
                CanonicalDigest::of(b"outcome-a"),
            )
            .await
    });
    let second_ledger = ledger.clone();
    let second = tokio::spawn(async move {
        second_ledger
            .terminalize(
                key("terminal-b"),
                [3; 32],
                CanonicalDigest::of(b"outcome-b"),
            )
            .await
    });
    let first = first.await.unwrap();
    let second = second.await.unwrap();

    assert_one_cas_winner(&first, &second);
    assert!(ledger.state().await.unwrap().terminal_outcome.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_safe_faults_cas_against_the_validated_prefix() {
    let (store, ledger) = race_ledger("race-safe-fault").await;
    ledger
        .admit(key("admit"), [1; 32], admission(b"graph"))
        .await
        .unwrap();
    let dispatch = ledger.dispatch(key("dispatch"), [2; 32]).await.unwrap();
    let fault = FaultFactory::new(&NoopObservationSink).create(ModuleEvidence::new(
        FaultModule::Worker,
        FaultContext::Execution,
        EvidenceClass::ProcessExited,
    ));
    store.arm();
    let execution = dispatch.value.execution;
    let first_ledger = ledger.clone();
    let first_fault = fault.clone();
    let first = tokio::spawn(async move {
        first_ledger
            .record_safe_fault(
                key("safe-fault-a"),
                [3; 32],
                &first_fault,
                SafeFaultConsequence::Settle {
                    execution,
                    outcome_digest: CanonicalDigest::of(b"outcome-a"),
                },
            )
            .await
    });
    let second_ledger = ledger.clone();
    let second = tokio::spawn(async move {
        second_ledger
            .record_safe_fault(
                key("safe-fault-b"),
                [4; 32],
                &fault,
                SafeFaultConsequence::Settle {
                    execution,
                    outcome_digest: CanonicalDigest::of(b"outcome-b"),
                },
            )
            .await
    });
    let first = first.await.unwrap();
    let second = second.await.unwrap();

    assert_one_cas_winner(&first, &second);
    let state = ledger.state().await.unwrap();
    assert_eq!(state.safe_faults.len(), 1);
    assert_eq!(state.settlements.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_effect_intents_cas_against_the_validated_prefix() {
    let (store, ledger) = race_ledger("race-effect-intent").await;
    ledger
        .admit(key("admit"), [1; 32], admission(b"graph"))
        .await
        .unwrap();
    store.arm();
    let first_ledger = ledger.clone();
    let first = tokio::spawn(async move {
        first_ledger
            .record_effect_intent(
                key("effect-intent-a"),
                [2; 32],
                CanonicalDigest::of(b"request-a"),
            )
            .await
    });
    let second_ledger = ledger.clone();
    let second = tokio::spawn(async move {
        second_ledger
            .record_effect_intent(
                key("effect-intent-b"),
                [3; 32],
                CanonicalDigest::of(b"request-b"),
            )
            .await
    });
    let first = first.await.unwrap();
    let second = second.await.unwrap();

    assert_one_cas_winner(&first, &second);
    let state = ledger.state().await.unwrap();
    assert_eq!(state.effects.len(), 1);
    assert_eq!(state.identities.next_effect, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_effect_receipts_cas_against_the_validated_prefix() {
    let (store, ledger) = race_ledger("race-effect-receipt").await;
    ledger
        .admit(key("admit"), [1; 32], admission(b"graph"))
        .await
        .unwrap();
    let effect = ledger
        .record_effect_intent(
            key("effect-intent"),
            [2; 32],
            CanonicalDigest::of(b"request"),
        )
        .await
        .unwrap()
        .value
        .effect;
    store.arm();
    let first_ledger = ledger.clone();
    let first = tokio::spawn(async move {
        first_ledger
            .reconcile_effect(
                key("effect-receipt-a"),
                [3; 32],
                effect,
                CanonicalDigest::of(b"receipt-a"),
            )
            .await
    });
    let second_ledger = ledger.clone();
    let second = tokio::spawn(async move {
        second_ledger
            .reconcile_effect(
                key("effect-receipt-b"),
                [4; 32],
                effect,
                CanonicalDigest::of(b"receipt-b"),
            )
            .await
    });
    let first = first.await.unwrap();
    let second = second.await.unwrap();

    assert_one_cas_winner(&first, &second);
    assert!(
        ledger
            .state()
            .await
            .unwrap()
            .effects
            .get(&effect)
            .unwrap()
            .receipt_digest
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_cleanup_receipts_cas_against_the_validated_prefix() {
    let (store, ledger) = race_ledger("race-cleanup").await;
    ledger
        .admit(key("admit"), [1; 32], admission(b"graph"))
        .await
        .unwrap();
    ledger
        .terminalize(key("terminal"), [2; 32], CanonicalDigest::of(b"outcome"))
        .await
        .unwrap();
    store.arm();
    let first_ledger = ledger.clone();
    let first = tokio::spawn(async move {
        first_ledger
            .record_cleanup_receipt(key("cleanup-a"), [3; 32], CanonicalDigest::of(b"cleanup-a"))
            .await
    });
    let second_ledger = ledger.clone();
    let second = tokio::spawn(async move {
        second_ledger
            .record_cleanup_receipt(key("cleanup-b"), [4; 32], CanonicalDigest::of(b"cleanup-b"))
            .await
    });
    let first = first.await.unwrap();
    let second = second.await.unwrap();

    assert_one_cas_winner(&first, &second);
    assert_eq!(ledger.state().await.unwrap().cleanup_receipts.len(), 1);
}

#[tokio::test]
async fn protocol_adapters_fold_control_and_lifecycle_from_one_empty_prefix() {
    assert_protocol_store_adapters::<ClusterLedgerAdapters>();
    let (_, ledger) = ledger("adapter-prefix").await;
    let adapters = ClusterLedgerAdapters::new(ledger);
    let (admission, lifecycle) = adapters.read_aggregate().await.unwrap();
    assert_eq!(
        admission.control.phase,
        openengine_cluster_protocol::Phase::Empty
    );
    assert_eq!(admission.control.cursor, None);
    assert_eq!(lifecycle.latest_cursor, None);
}

#[tokio::test]
async fn protocol_admission_adapter_commits_control_seed_and_receipt_atomically() {
    let (store, ledger) = race_ledger("adapter-admission").await;
    let adapters = ClusterLedgerAdapters::new(ledger);
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let graph = serde_json::from_slice(
        &std::fs::read(
            repository
                .join("protocol/openengine-cluster/v1/fixtures/graph/positive/full-all-nodes.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let compiled_ir = serde_json::from_slice(
        &std::fs::read(
            repository
                .join("protocol/openengine-cluster/v1/fixtures/graph/positive/compiled-ir.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let fingerprint = openengine_cluster_protocol::admission_fingerprint(
        "apply",
        &serde_json::json!({"fixture": "adapter"}),
    )
    .unwrap();
    let proposal = CommitProposal {
        graph,
        compiled_ir,
        input: Some(serde_json::json!({})),
        if_generation: Some(openengine_cluster_protocol::Generation::new(0).unwrap()),
        idempotency_key: openengine_cluster_protocol::IdempotencyKey::new("adapter-apply").unwrap(),
        fingerprint,
    };
    let mut conflict = proposal.clone();
    let mut changed = proposal.clone();
    let mut unchanged = proposal.clone();
    unchanged.input = None;
    unchanged.idempotency_key =
        openengine_cluster_protocol::IdempotencyKey::new("adapter-unchanged").unwrap();
    unchanged.fingerprint = openengine_cluster_protocol::admission_fingerprint(
        "apply",
        &serde_json::json!({"fixture": "unchanged"}),
    )
    .unwrap();
    let first = adapters
        .commit(proposal.clone(), &CancellationSignal::default())
        .await
        .unwrap();
    assert!(!first.deduped);
    unchanged.if_generation = first.generation;
    let unchanged_result = adapters
        .commit(unchanged, &CancellationSignal::default())
        .await
        .unwrap();
    assert!(!unchanged_result.deduped);
    assert_eq!(unchanged_result.generation, first.generation);
    assert_eq!(unchanged_result.run_id, first.run_id);
    let cancelled = CancellationSignal::default();
    cancelled.cancel();
    let replayed = adapters.commit(proposal, &cancelled).await.unwrap();
    assert!(replayed.deduped);
    assert_eq!(first.generation, replayed.generation);
    assert_eq!(first.run_id, replayed.run_id);
    conflict.fingerprint = openengine_cluster_protocol::admission_fingerprint(
        "apply",
        &serde_json::json!({"fixture": "conflict"}),
    )
    .unwrap();
    assert!(matches!(
        adapters
            .commit(conflict, &CancellationSignal::default())
            .await,
        Err(openengine_cluster_server::admission::StoreError::IdempotencyReuse)
    ));

    let (admission, lifecycle) = adapters.read_aggregate().await.unwrap();
    assert_eq!(
        admission.control.phase,
        openengine_cluster_protocol::Phase::Running
    );
    assert!(admission.control.spec.is_some());
    assert!(admission.control.compiled_ir.is_some());
    assert_eq!(admission.seed.as_ref().unwrap().cursor.as_str(), "ledger:2");
    assert_eq!(
        admission.control.cursor.as_ref().unwrap().as_str(),
        "ledger:4"
    );
    assert_eq!(
        lifecycle.latest_cursor.as_ref().unwrap().as_str(),
        "ledger:4"
    );

    let verifier_vector: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            repository.join("protocol/openengine-cluster/v1/fixtures/verifier/positive/basic.json"),
        )
        .unwrap(),
    )
    .unwrap();
    changed.compiled_ir =
        serde_json::from_value(verifier_vector["expected"]["compiledIr"].clone()).unwrap();
    changed.if_generation = first.generation;
    changed.idempotency_key =
        openengine_cluster_protocol::IdempotencyKey::new("adapter-changed").unwrap();
    changed.fingerprint = openengine_cluster_protocol::admission_fingerprint(
        "apply",
        &serde_json::json!({"fixture": "changed"}),
    )
    .unwrap();
    let cancellation_race = CancellationSignal::default();
    store.cancel_before_next_append(cancellation_race.clone());
    let position_before_cancel = adapters.ledger().state().await.unwrap().position;
    assert_eq!(
        adapters
            .commit(changed.clone(), &cancellation_race)
            .await
            .unwrap_err(),
        openengine_cluster_server::admission::StoreError::Cancelled
    );
    assert_eq!(
        adapters.ledger().state().await.unwrap().position,
        position_before_cancel
    );
    let changed_result = adapters
        .commit(changed, &CancellationSignal::default())
        .await
        .unwrap();
    assert_eq!(changed_result.generation.unwrap().get(), 2);
    assert_eq!(changed_result.run_id.as_ref().unwrap().as_str(), "run:2");
    let (changed_admission, changed_lifecycle) = adapters.read_aggregate().await.unwrap();
    assert_eq!(
        changed_admission.control.cursor.as_ref().unwrap().as_str(),
        "ledger:7"
    );
    assert_eq!(
        changed_lifecycle.latest_cursor.as_ref().unwrap().as_str(),
        "ledger:7"
    );
}

#[tokio::test]
async fn generation_cas_allocates_exact_sequential_run_identities() {
    let (_, ledger) = ledger("generation-cas").await;
    let first = ledger
        .admit(key("first"), [1; 32], admission(b"graph-one"))
        .await
        .unwrap();
    let first_dispatch = ledger
        .dispatch(key("first-dispatch"), [2; 32])
        .await
        .unwrap();
    let first_outcome = CanonicalDigest::of(b"first-outcome");
    ledger
        .settle(
            key("first-settlement"),
            [3; 32],
            first_dispatch.value.execution,
            first_outcome,
            None,
        )
        .await
        .unwrap();
    let before = ledger.state().await.unwrap().position;
    assert!(
        ledger
            .admit_next(
                key("wrong-cas"),
                [4; 32],
                zeroshot_engine::cluster_ledger::GenerationId::new(99).unwrap(),
                admission(b"graph-two"),
            )
            .await
            .is_err()
    );
    assert_eq!(ledger.state().await.unwrap().position, before);

    let second = ledger
        .admit_next(
            key("second"),
            [5; 32],
            first.value.generation,
            admission(b"graph-two"),
        )
        .await
        .unwrap();
    assert_eq!(second.value.generation.get(), 2);
    assert_eq!(second.value.run.get(), 2);
    let state = ledger.state().await.unwrap();
    assert_eq!(state.admission.unwrap().generation, second.value.generation);
    assert_eq!(state.verified_inputs.len(), 2);
    assert_eq!(state.position.get(), 10);
    let late = ledger
        .settle(
            key("late-first-settlement"),
            [6; 32],
            first_dispatch.value.execution,
            CanonicalDigest::of(b"late-outcome"),
            None,
        )
        .await
        .unwrap();
    assert!(!late.value.accepted);
    assert_eq!(late.value.authoritative_digest, first_outcome);
    assert_eq!(ledger.state().await.unwrap().position.get(), 12);
}

#[tokio::test]
async fn identical_prefix_replays_to_byte_identical_public_state() {
    let (store, ledger) = ledger("exact-replay").await;
    let admitted = ledger
        .admit(key("admit"), [1; 32], admission(b"graph"))
        .await
        .unwrap();
    assert!(!admitted.replayed);
    let duplicate = ledger
        .admit(key("admit"), [1; 32], admission(b"graph"))
        .await
        .unwrap();
    assert!(duplicate.replayed);
    assert_eq!(admitted.value, duplicate.value);

    let dispatched = ledger.dispatch(key("dispatch"), [2; 32]).await.unwrap();
    let output = br#"{"result":"ok"}"#.to_vec();
    let output_digest = CanonicalDigest::of(&output);
    let settled = ledger
        .settle(
            key("settle"),
            [3; 32],
            dispatched.value.execution,
            output_digest,
            Some(output),
        )
        .await
        .unwrap();
    assert!(settled.value.accepted);
    let late = ledger
        .settle(
            key("late"),
            [4; 32],
            dispatched.value.execution,
            CanonicalDigest::of(b"late"),
            None,
        )
        .await
        .unwrap();
    assert!(!late.value.accepted);
    assert_eq!(late.value.authoritative_digest, output_digest);

    let terminal = ledger
        .terminalize(key("terminal"), [5; 32], output_digest)
        .await
        .unwrap();
    ledger
        .record_cleanup_receipt(key("cleanup"), [6; 32], CanonicalDigest::of(b"clean"))
        .await
        .unwrap();
    assert!(
        ledger
            .dispatch(key("post-terminal"), [7; 32])
            .await
            .is_err()
    );

    let prefix = store
        .read_prefix(ledger.resource(), Some(terminal.position))
        .await
        .unwrap();
    let first = replay(&prefix, ledger.resource())
        .unwrap()
        .public_bytes()
        .unwrap();
    let second = replay(&prefix, ledger.resource())
        .unwrap()
        .public_bytes()
        .unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn corruption_and_impossible_order_fail_closed() {
    let (store, ledger) = ledger("corruption").await;
    ledger
        .admit(key("admit"), [1; 32], admission(b"graph"))
        .await
        .unwrap();
    let snapshot = store.read_prefix(ledger.resource(), None).await.unwrap();

    let mut unknown_version = snapshot.clone();
    unknown_version.records[0].version = 999;
    assert!(matches!(
        replay(&unknown_version, ledger.resource()),
        Err(ReplayError::Record(_))
    ));

    let mut gap = snapshot.clone();
    gap.records[1].sequence = Position::new(3).unwrap();
    assert!(matches!(
        replay(&gap, ledger.resource()),
        Err(ReplayError::Record(_))
    ));

    let mut hash = snapshot.clone();
    hash.records[0].record_hash[0] ^= 1;
    assert!(matches!(
        replay(&hash, ledger.resource()),
        Err(ReplayError::Record(_))
    ));

    let mut receipt = snapshot.clone();
    receipt.receipts[0].committed_position = Position::MAX;
    assert_eq!(
        replay(&receipt, ledger.resource()).unwrap_err(),
        ReplayError::ReceiptCorrupt
    );

    let mut lower_position = snapshot.clone();
    lower_position.receipts[0].committed_position =
        Position::new(lower_position.receipts[0].committed_position.get() - 1).unwrap();
    assert_eq!(
        replay(&lower_position, ledger.resource()).unwrap_err(),
        ReplayError::ReceiptCorrupt
    );

    let mut missing = snapshot.clone();
    missing.receipts.clear();
    assert_eq!(
        replay(&missing, ledger.resource()).unwrap_err(),
        ReplayError::ReceiptCorrupt
    );

    let mut missing_receipt_record = snapshot.clone();
    missing_receipt_record.records.pop();
    missing_receipt_record.position = Position::new(2).unwrap();
    missing_receipt_record.receipts.clear();
    assert_eq!(
        replay(&missing_receipt_record, ledger.resource()).unwrap_err(),
        ReplayError::ReceiptCorrupt
    );

    let mut forged = snapshot;
    forged.receipts[0].method = "dispatch".to_owned();
    forged.receipts[0].fingerprint = [9; 32];
    forged.receipts[0].response =
        serde_json::to_vec(&zeroshot_engine::cluster_ledger::DispatchAllocation {
            run: zeroshot_engine::cluster_ledger::RunSequence::new(1).unwrap(),
            node_instance: zeroshot_engine::cluster_ledger::NodeInstanceId::new(1).unwrap(),
            execution: zeroshot_engine::cluster_ledger::ExecutionId::new(1).unwrap(),
        })
        .unwrap();
    assert_eq!(
        replay(&forged, ledger.resource()).unwrap_err(),
        ReplayError::ReceiptCorrupt
    );
}

#[tokio::test]
async fn replay_rejects_receipts_whose_response_was_not_committed() {
    let (store, ledger) = ledger("forged-response").await;
    ledger
        .admit(key("admit"), [1; 32], admission(b"graph"))
        .await
        .unwrap();
    let mut snapshot = store.read_prefix(ledger.resource(), None).await.unwrap();
    let forged = zeroshot_engine::cluster_ledger::mutations::AdmissionAllocation {
        generation: zeroshot_engine::cluster_ledger::GenerationId::new(2).unwrap(),
        run: zeroshot_engine::cluster_ledger::RunSequence::new(1).unwrap(),
    };
    snapshot.receipts[0].response = serde_json::to_vec(&forged).unwrap();
    let receipt_index = snapshot
        .records
        .iter()
        .position(|record| record.kind == RecordKind::MutationReceipt)
        .unwrap();
    let forged_receipt = snapshot.receipts[0].clone();
    replace_payload(
        &mut snapshot,
        receipt_index,
        RecordPayload::MutationReceipt {
            receipt: forged_receipt,
        },
    );

    assert_eq!(
        replay(&snapshot, ledger.resource()).unwrap_err(),
        ReplayError::ReceiptCorrupt
    );
}

#[tokio::test]
async fn replay_rejects_verified_io_that_contradicts_authoritative_records() {
    let (store, ledger) = ledger("contradictory-io").await;
    ledger
        .admit(key("admit"), [1; 32], admission(b"graph"))
        .await
        .unwrap();
    let dispatch = ledger.dispatch(key("dispatch"), [2; 32]).await.unwrap();
    let output = b"accepted-output".to_vec();
    ledger
        .settle(
            key("settle"),
            [3; 32],
            dispatch.value.execution,
            CanonicalDigest::of(&output),
            Some(output),
        )
        .await
        .unwrap();
    let snapshot = store.read_prefix(ledger.resource(), None).await.unwrap();

    let mut forged_input = snapshot.clone();
    let input_index = forged_input
        .records
        .iter()
        .position(|record| record.kind == RecordKind::VerifiedInput)
        .unwrap();
    let different_input = b"different-verified-input".to_vec();
    replace_payload(
        &mut forged_input,
        input_index,
        RecordPayload::VerifiedInput {
            run: zeroshot_engine::cluster_ledger::RunSequence::new(1).unwrap(),
            digest: CanonicalDigest::of(&different_input),
            canonical_bytes: different_input,
        },
    );
    assert_eq!(
        replay(&forged_input, ledger.resource()).unwrap_err(),
        ReplayError::InvalidOrder
    );

    let mut forged_output = snapshot;
    let output_index = forged_output
        .records
        .iter()
        .position(|record| record.kind == RecordKind::VerifiedOutput)
        .unwrap();
    let different_output = b"different-verified-output".to_vec();
    replace_payload(
        &mut forged_output,
        output_index,
        RecordPayload::VerifiedOutput {
            run: zeroshot_engine::cluster_ledger::RunSequence::new(1).unwrap(),
            execution: dispatch.value.execution,
            digest: CanonicalDigest::of(&different_output),
            canonical_bytes: different_output,
        },
    );
    assert_eq!(
        replay(&forged_output, ledger.resource()).unwrap_err(),
        ReplayError::InvalidOrder
    );
}

#[tokio::test]
async fn pending_effects_block_every_terminal_transition() {
    let (store, ledger) = ledger("pending-effect-terminal").await;
    ledger
        .admit(key("admit"), [1; 32], admission(b"graph"))
        .await
        .unwrap();
    let effect = ledger
        .record_effect_intent(key("effect"), [2; 32], CanonicalDigest::of(b"request"))
        .await
        .unwrap()
        .value
        .effect;
    let pending_position = ledger.state().await.unwrap().position;

    let terminal_error = ledger
        .terminalize(key("terminal"), [3; 32], CanonicalDigest::of(b"done"))
        .await
        .unwrap_err();
    assert!(matches!(
        terminal_error.kind(),
        LedgerErrorKind::InvalidLifecycle
    ));

    let fault = FaultFactory::new(&NoopObservationSink).create(ModuleEvidence::new(
        FaultModule::Worker,
        FaultContext::Execution,
        EvidenceClass::MalformedExternalData,
    ));
    let fault_error = ledger
        .record_safe_fault(
            key("fault-terminal"),
            [4; 32],
            &fault,
            SafeFaultConsequence::Terminal {
                outcome_digest: CanonicalDigest::of(b"faulted"),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        fault_error.kind(),
        LedgerErrorKind::InvalidLifecycle
    ));
    assert_eq!(ledger.state().await.unwrap().position, pending_position);

    let mut forged = store.read_prefix(ledger.resource(), None).await.unwrap();
    let terminal = RecordPayload::Terminal {
        run: zeroshot_engine::cluster_ledger::RunSequence::new(1).unwrap(),
        outcome_digest: CanonicalDigest::of(b"forged-terminal"),
    };
    let position = forged.position.checked_add(1).unwrap();
    forged.records.push(
        StoredRecord::build(
            ledger.resource().clone(),
            position,
            &terminal,
            forged.records.last().unwrap().record_hash,
        )
        .unwrap(),
    );
    forged.position = position;
    assert_eq!(
        replay(&forged, ledger.resource()).unwrap_err(),
        ReplayError::InvalidOrder
    );

    ledger
        .reconcile_effect(
            key("effect-receipt"),
            [5; 32],
            effect,
            CanonicalDigest::of(b"receipt"),
        )
        .await
        .unwrap();
    ledger
        .terminalize(key("terminal"), [3; 32], CanonicalDigest::of(b"done"))
        .await
        .unwrap();
}

#[tokio::test]
async fn persisted_safe_fault_never_contains_ephemeral_diagnostic_bytes() {
    let secret = "Authorization: Bearer ledger-secret";
    let (store, ledger) = ledger("redacted-fault").await;
    ledger
        .admit(key("admit"), [1; 32], admission(b"graph"))
        .await
        .unwrap();
    let dispatch = ledger.dispatch(key("dispatch"), [2; 32]).await.unwrap();
    let fault = FaultFactory::new(&NoopObservationSink).create(
        ModuleEvidence::new(
            FaultModule::Worker,
            FaultContext::Execution,
            EvidenceClass::MalformedExternalData,
        )
        .with_diagnostic(RawDiagnostic::new(RedactionMarker::Header, secret).unwrap()),
    );
    ledger
        .record_safe_fault(
            key("fault"),
            [3; 32],
            &fault,
            SafeFaultConsequence::Settle {
                execution: dispatch.value.execution,
                outcome_digest: CanonicalDigest::of(b"faulted"),
            },
        )
        .await
        .unwrap();
    let records = store.read_prefix(ledger.resource(), None).await.unwrap();
    let encoded = String::from_utf8(serde_json::to_vec(&records.records).unwrap()).unwrap();
    assert!(!encoded.contains(secret));
    assert!(!encoded.contains("ledger-secret"));
}

#[tokio::test]
async fn fixed_bounds_fail_before_durable_write() {
    assert!(ResourceId::new("x".repeat(MAX_IDENTIFIER_BYTES + 1)).is_err());
    assert!(IdempotencyId::new("\u{7}").is_err());
    assert!(
        serde_json::from_value::<ResourceId>(serde_json::Value::String(
            "x".repeat(MAX_IDENTIFIER_BYTES + 1)
        ))
        .is_err()
    );
    assert!(
        serde_json::from_value::<zeroshot_engine::cluster_ledger::ExecutionId>(serde_json::json!(
            0
        ))
        .is_err()
    );
    assert!(serde_json::from_value::<Position>(serde_json::json!(u64::MAX)).is_err());
    assert_eq!(MAX_DISCOVERY_PAGE, 1_024);

    let (_, ledger) = ledger("bounds").await;
    let oversized = vec![b'x'; MAX_RECORD_PAYLOAD_BYTES + 1];
    let mut request = admission(&oversized);
    request.graph_digest = CanonicalDigest::of(&oversized);
    assert!(
        ledger
            .admit(key("oversized"), [8; 32], request)
            .await
            .is_err()
    );
    assert_eq!(ledger.state().await.unwrap().position, Position::ZERO);

    let resource = resource("batch-bound");
    let payload = RecordPayload::CleanupReceipt {
        cleanup_digest: CanonicalDigest::of(b"x"),
    };
    let records = (1..=MAX_APPEND_RECORDS + 1)
        .map(|sequence| {
            StoredRecord::build(
                resource.clone(),
                Position::new(sequence as u64).unwrap(),
                &payload,
                [0; 32],
            )
            .unwrap()
        })
        .collect();
    assert!(AppendBatch::new(records, None).is_err());
}

#[test]
fn post_terminal_record_is_rejected_by_pure_replay() {
    let resource = resource("terminal-order");
    let terminal = RecordPayload::Terminal {
        run: zeroshot_engine::cluster_ledger::RunSequence::new(1).unwrap(),
        outcome_digest: CanonicalDigest::of(b"done"),
    };
    let dispatch = RecordPayload::Dispatch {
        run: zeroshot_engine::cluster_ledger::RunSequence::new(1).unwrap(),
        node_instance: zeroshot_engine::cluster_ledger::NodeInstanceId::new(1).unwrap(),
        execution: zeroshot_engine::cluster_ledger::ExecutionId::new(1).unwrap(),
    };
    let admission = RecordPayload::Admission {
        generation: zeroshot_engine::cluster_ledger::GenerationId::new(1).unwrap(),
        run: zeroshot_engine::cluster_ledger::RunSequence::new(1).unwrap(),
        graph_digest: CanonicalDigest::of(b"g"),
        input_digest: CanonicalDigest::of(b"null"),
        policy_digest: CanonicalDigest::of(b"p"),
        catalog_digest: CanonicalDigest::of(b"c"),
        profile_digest: CanonicalDigest::of(b"f"),
        absolute_deadline_ms: 1,
        canonical_graph: b"g".to_vec(),
        canonical_compiled_ir: Vec::new(),
    };
    let first = StoredRecord::build(
        resource.clone(),
        Position::new(1).unwrap(),
        &admission,
        [0; 32],
    )
    .unwrap();
    let second = StoredRecord::build(
        resource.clone(),
        Position::new(2).unwrap(),
        &terminal,
        first.record_hash,
    )
    .unwrap();
    let third = StoredRecord::build(
        resource.clone(),
        Position::new(3).unwrap(),
        &dispatch,
        second.record_hash,
    )
    .unwrap();
    let snapshot = PrefixSnapshot {
        position: Position::new(3).unwrap(),
        records: vec![first, second, third],
        receipts: Vec::<MutationReceipt>::new(),
    };
    assert_eq!(
        replay(&snapshot, &resource).unwrap_err(),
        ReplayError::PostTerminalRecord
    );
}
