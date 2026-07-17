mod support;

use std::sync::Arc;

use support::ledger::{key, owner, resource, temp_root};
use zeroshot_engine::cluster_ledger::mutations::{AdmissionRequest, SafeFaultConsequence};
use zeroshot_engine::cluster_ledger::record::CanonicalDigest;
use zeroshot_engine::cluster_ledger::store::fake::{FakeLedgerStore, ManualLedgerClock};
use zeroshot_engine::cluster_ledger::store::sqlite::SqliteLedgerStore;
use zeroshot_engine::cluster_ledger::store::{FailPoint, LedgerStore};
use zeroshot_engine::cluster_ledger::ClusterLedger;
use zeroshot_engine::fault::{EvidenceClass, FaultContext, FaultFactory, FaultModule, ModuleEvidence};
use zeroshot_engine::observability::NoopObservationSink;

fn admission() -> AdmissionRequest {
    let graph = b"canonical-graph".to_vec();
    let input = br#"{"verified":true}"#.to_vec();
    AdmissionRequest {
        graph_digest: CanonicalDigest::of(&graph),
        input_digest: CanonicalDigest::of(&input),
        policy_digest: CanonicalDigest::of(b"policy"),
        catalog_digest: CanonicalDigest::of(b"catalog"),
        profile_digest: CanonicalDigest::of(b"profile"),
        absolute_deadline_ms: 10_000,
        verified_input: input,
        canonical_graph: graph,
        canonical_compiled_ir: b"compiled".to_vec(),
    }
}

fn safe_fault() -> zeroshot_engine::fault::EngineFault {
    FaultFactory::new(&NoopObservationSink).create(ModuleEvidence::new(
        FaultModule::Worker,
        FaultContext::Execution,
        EvidenceClass::ProcessExited,
    ))
}

async fn before_commit_matrix(store: Arc<dyn LedgerStore>, fail: impl Fn(FailPoint), label: &str) {
    let ledger = ClusterLedger::create(
        Arc::clone(&store),
        resource(label),
        owner("before-owner"),
        10_000,
    )
    .await
    .unwrap();

    fail(FailPoint::BeforeCommit);
    assert!(
        ledger
            .admit(key("admit-before"), [1; 32], admission())
            .await
            .is_err()
    );
    assert_eq!(ledger.state().await.unwrap().position.get(), 0);
    ledger
        .admit(key("admit"), [2; 32], admission())
        .await
        .unwrap();

    let before = ledger.state().await.unwrap().position;
    fail(FailPoint::BeforeCommit);
    assert!(
        ledger
            .dispatch(key("dispatch-before"), [3; 32])
            .await
            .is_err()
    );
    assert_eq!(ledger.state().await.unwrap().position, before);
    let dispatch = ledger.dispatch(key("dispatch"), [4; 32]).await.unwrap();

    let before = ledger.state().await.unwrap().position;
    fail(FailPoint::BeforeCommit);
    assert!(
        ledger
            .settle(
                key("settle-before"),
                [5; 32],
                dispatch.value.execution,
                CanonicalDigest::of(b"settled"),
                None,
            )
            .await
            .is_err()
    );
    assert_eq!(ledger.state().await.unwrap().position, before);
    ledger
        .settle(
            key("settle"),
            [6; 32],
            dispatch.value.execution,
            CanonicalDigest::of(b"settled"),
            None,
        )
        .await
        .unwrap();

    let before = ledger.state().await.unwrap().position;
    fail(FailPoint::BeforeCommit);
    assert!(
        ledger
            .record_effect_intent(
                key("effect-intent-before"),
                [7; 32],
                CanonicalDigest::of(b"request"),
            )
            .await
            .is_err()
    );
    assert_eq!(ledger.state().await.unwrap().position, before);
    let effect = ledger
        .record_effect_intent(
            key("effect-intent"),
            [8; 32],
            CanonicalDigest::of(b"request"),
        )
        .await
        .unwrap();

    let before = ledger.state().await.unwrap().position;
    fail(FailPoint::BeforeCommit);
    assert!(
        ledger
            .reconcile_effect(
                key("effect-receipt-before"),
                [9; 32],
                effect.value.effect,
                CanonicalDigest::of(b"receipt"),
            )
            .await
            .is_err()
    );
    assert_eq!(ledger.state().await.unwrap().position, before);
    ledger
        .reconcile_effect(
            key("effect-receipt"),
            [10; 32],
            effect.value.effect,
            CanonicalDigest::of(b"receipt"),
        )
        .await
        .unwrap();

    let dispatch = ledger
        .dispatch(key("fault-dispatch"), [11; 32])
        .await
        .unwrap();
    let before = ledger.state().await.unwrap().position;
    fail(FailPoint::BeforeCommit);
    assert!(
        ledger
            .record_safe_fault(
                key("fault-before"),
                [12; 32],
                &safe_fault(),
                SafeFaultConsequence::Settle {
                    execution: dispatch.value.execution,
                    outcome_digest: CanonicalDigest::of(b"faulted"),
                },
            )
            .await
            .is_err()
    );
    assert_eq!(ledger.state().await.unwrap().position, before);
    ledger
        .record_safe_fault(
            key("fault"),
            [13; 32],
            &safe_fault(),
            SafeFaultConsequence::Settle {
                execution: dispatch.value.execution,
                outcome_digest: CanonicalDigest::of(b"faulted"),
            },
        )
        .await
        .unwrap();

    let before = ledger.state().await.unwrap().position;
    fail(FailPoint::BeforeCommit);
    assert!(
        ledger
            .terminalize(
                key("terminal-before"),
                [14; 32],
                CanonicalDigest::of(b"done"),
            )
            .await
            .is_err()
    );
    assert_eq!(ledger.state().await.unwrap().position, before);
    ledger
        .terminalize(key("terminal"), [15; 32], CanonicalDigest::of(b"done"))
        .await
        .unwrap();

    fail(FailPoint::BeforeCommit);
    assert!(ledger.remove_terminal().await.is_err());
    assert!(store.open(ledger.resource()).await.is_ok());
}

async fn after_commit_matrix(store: Arc<dyn LedgerStore>, fail: impl Fn(FailPoint), label: &str) {
    let ledger = ClusterLedger::create(
        Arc::clone(&store),
        resource(label),
        owner("after-owner"),
        10_000,
    )
    .await
    .unwrap();

    fail(FailPoint::AfterCommitBeforeResponse);
    assert!(
        ledger
            .admit(key("admit"), [1; 32], admission())
            .await
            .unwrap()
            .replayed
    );
    fail(FailPoint::AfterCommitBeforeResponse);
    let dispatch = ledger.dispatch(key("dispatch"), [2; 32]).await.unwrap();
    assert!(dispatch.replayed);
    fail(FailPoint::AfterCommitBeforeResponse);
    assert!(
        ledger
            .settle(
                key("settle"),
                [3; 32],
                dispatch.value.execution,
                CanonicalDigest::of(b"settled"),
                None,
            )
            .await
            .unwrap()
            .replayed
    );

    fail(FailPoint::AfterCommitBeforeResponse);
    let effect = ledger
        .record_effect_intent(
            key("effect-intent"),
            [4; 32],
            CanonicalDigest::of(b"request"),
        )
        .await
        .unwrap();
    assert!(effect.replayed);
    fail(FailPoint::AfterCommitBeforeResponse);
    assert!(
        ledger
            .reconcile_effect(
                key("effect-receipt"),
                [5; 32],
                effect.value.effect,
                CanonicalDigest::of(b"receipt"),
            )
            .await
            .unwrap()
            .replayed
    );

    let dispatch = ledger
        .dispatch(key("fault-dispatch"), [6; 32])
        .await
        .unwrap();
    fail(FailPoint::AfterCommitBeforeResponse);
    assert!(
        ledger
            .record_safe_fault(
                key("fault"),
                [7; 32],
                &safe_fault(),
                SafeFaultConsequence::Settle {
                    execution: dispatch.value.execution,
                    outcome_digest: CanonicalDigest::of(b"faulted"),
                },
            )
            .await
            .unwrap()
            .replayed
    );

    fail(FailPoint::AfterCommitBeforeResponse);
    assert!(
        ledger
            .terminalize(key("terminal"), [8; 32], CanonicalDigest::of(b"done"))
            .await
            .unwrap()
            .replayed
    );
    fail(FailPoint::AfterCommitBeforeResponse);
    assert!(
        ledger
            .record_cleanup_receipt(key("cleanup"), [9; 32], CanonicalDigest::of(b"clean"),)
            .await
            .unwrap()
            .replayed
    );

    fail(FailPoint::AfterCommitBeforeResponse);
    assert!(ledger.remove_terminal().await.is_err());
    assert!(store.open(ledger.resource()).await.is_err());
}

#[tokio::test]
async fn fake_store_crash_matrix_preserves_atomic_authority() {
    let store = Arc::new(FakeLedgerStore::new(ManualLedgerClock::new(100)));
    before_commit_matrix(store.clone(), |point| store.fail_next(point), "fake-before").await;
    after_commit_matrix(store.clone(), |point| store.fail_next(point), "fake-after").await;
}

#[tokio::test]
async fn sqlite_store_crash_matrix_preserves_atomic_authority() {
    let root = temp_root("crash-sqlite");
    let store =
        Arc::new(SqliteLedgerStore::with_clock(&root, ManualLedgerClock::new(100)).unwrap());
    before_commit_matrix(
        store.clone(),
        |point| store.fail_next(point),
        "sqlite-before",
    )
    .await;
    after_commit_matrix(
        store.clone(),
        |point| store.fail_next(point),
        "sqlite-after",
    )
    .await;
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
