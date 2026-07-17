mod support;

use std::sync::Arc;

use openengine_cluster_protocol::{
    admission_fingerprint, Cursor, Generation, IdempotencyKey, Phase, StopMode, StopParams,
    UpdateParams,
};
use openengine_cluster_server::admission::{AdmissionStore, CancellationSignal, CommitProposal};
use openengine_cluster_server::lifecycle::{
    LifecycleStore, StopProposal, TurnId, UpdateProposal, VerifiedCompletion,
};
use serde_json::json;
use support::ledger::{graph_and_ir, DispatchRaceStore, ManualClock};
use zeroshot_engine::fault::{EvidenceClass, FaultContext, FaultFactory, FaultModule, ModuleEvidence};
use zeroshot_engine::ledger::fold::fold_records;
use zeroshot_engine::ledger::record::{ClosedMutationReceipt, LedgerRecord};
use zeroshot_engine::ledger::store::LedgerStore;
use zeroshot_engine::ledger::{
    AbsoluteDeadline, AdmissionRequest, ClusterLedger, DispatchRequest, IdempotencyId,
    LedgerGeneration, LedgerRunId, LedgerError, MemoryLedgerStore, MutationIdentity, OwnerId,
    Position, RecordPayload, ResourceId, SettlementRequest, TerminalOutcome,
};
use zeroshot_engine::ledger::adapters::LedgerAdapters;
use zeroshot_engine::observability::NoopObservationSink;

fn mutation(key: &str, method: &str, value: serde_json::Value) -> MutationIdentity {
    MutationIdentity::for_value(IdempotencyId::new(key).unwrap(), method, &value).unwrap()
}

#[test]
fn bounded_numeric_identities_validate_deserialized_values() {
    assert!(serde_json::from_str::<LedgerGeneration>("0").is_err());
    assert!(serde_json::from_str::<LedgerGeneration>("9223372036854775808").is_err());
    assert_eq!(
        serde_json::from_str::<Position>("0").unwrap(),
        Position::ZERO
    );
    assert!(serde_json::from_str::<Position>("9223372036854775808").is_err());
    assert!(serde_json::from_str::<AbsoluteDeadline>("0").is_err());
    assert!(serde_json::from_str::<AbsoluteDeadline>("9223372036854775808").is_err());
}

#[tokio::test]
async fn simultaneous_identical_mutations_return_one_commit_and_one_deduped_receipt() {
    let clock = Arc::new(ManualClock::at(100));
    let inner = Arc::new(MemoryLedgerStore::new(clock));
    let store = Arc::new(DispatchRaceStore::new(inner));
    let ledger = Arc::new(
        ClusterLedger::create(
            store.clone(),
            ResourceId::new("concurrent-replay").unwrap(),
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
            mutation: mutation("admit", "admit", json!({"graph":"fixture"})),
        })
        .await
        .unwrap();
    let request = DispatchRequest {
        turn_id: "same-turn".into(),
        mutation: mutation("same-dispatch", "dispatch", json!({"turn":"same-turn"})),
    };
    store.arm_stale_prefix();
    let racing = {
        let ledger = Arc::clone(&ledger);
        let request = request.clone();
        tokio::spawn(async move { ledger.dispatch(request).await })
    };
    store.wait_for_stale_prefix().await;
    let committed = ledger.dispatch(request).await.unwrap();
    store.release_stale_prefix();
    let deduped = racing.await.unwrap().unwrap();
    assert_eq!(committed.execution_id, deduped.execution_id);
    assert!(!committed.deduped);
    assert!(deduped.deduped);
}

#[tokio::test]
async fn hash_consistent_forged_admission_manifest_and_run_identity_fail_replay() {
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(MemoryLedgerStore::new(clock));
    let resource = ResourceId::new("forged-admission").unwrap();
    let ledger = ClusterLedger::create(
        store.clone(),
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
            mutation: mutation("admit", "admit", json!({"graph":"fixture"})),
        })
        .await
        .unwrap();
    let prefix = store.read_prefix(&resource).await.unwrap();
    let mut admission = prefix.records[0].decode_payload().unwrap();
    let forged_run = LedgerRunId::new("run-1-cccccccccccccccc").unwrap();
    let zeroshot_engine::ledger::RecordPayload::Admission {
        run_id, manifest, ..
    } = &mut admission
    else {
        panic!("first record must be admission");
    };
    *run_id = forged_run.clone();
    manifest.graph_digest = "c".repeat(64);
    let first = zeroshot_engine::ledger::record::LedgerRecord::new(
        resource.clone(),
        zeroshot_engine::ledger::Position::new(1).unwrap(),
        &admission,
        [0; 32],
    )
    .unwrap();
    let mut receipt = prefix.records[1].decode_payload().unwrap();
    let zeroshot_engine::ledger::RecordPayload::MutationReceipt { receipt, .. } = &mut receipt
    else {
        panic!("second record must be receipt");
    };
    let zeroshot_engine::ledger::record::ClosedMutationReceipt::Admit(receipt) = receipt else {
        panic!("receipt must be admission");
    };
    receipt.run_id = forged_run;
    let second = zeroshot_engine::ledger::record::LedgerRecord::new(
        resource.clone(),
        zeroshot_engine::ledger::Position::new(2).unwrap(),
        &zeroshot_engine::ledger::RecordPayload::MutationReceipt {
            key: IdempotencyId::new("admit").unwrap(),
            fingerprint: mutation("admit", "admit", json!({"graph":"fixture"})).fingerprint,
            receipt: zeroshot_engine::ledger::record::ClosedMutationReceipt::Admit(receipt.clone()),
        },
        first.record_hash,
    )
    .unwrap();
    assert!(fold_records(&resource, &[first, second]).is_err());
}

#[tokio::test]
async fn identical_prefix_replays_to_identical_public_bytes_and_receipts() {
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(MemoryLedgerStore::new(clock));
    let resource = ResourceId::new("replay-cluster").unwrap();
    let ledger = ClusterLedger::create(
        store.clone(),
        resource.clone(),
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
        mutation: mutation("admit", "admit", json!({"graph":"fixture"})),
    };
    let first = ledger.admit(admission.clone()).await.unwrap();
    let replayed = ledger.admit(admission).await.unwrap();
    assert_eq!(first.generation.get(), 1);
    assert!(replayed.deduped);

    let dispatch = ledger
        .dispatch(DispatchRequest {
            turn_id: "turn-1".into(),
            mutation: mutation("dispatch", "dispatch", json!({"turn":"turn-1"})),
        })
        .await
        .unwrap();
    let settle = ledger
        .settle(SettlementRequest {
            execution_id: dispatch.execution_id.clone(),
            output: json!({"ok":true}),
            mutation: mutation("settle", "settle", json!({"execution":"one"})),
        })
        .await
        .unwrap();
    assert!(settle.accepted);
    let late = ledger
        .settle(SettlementRequest {
            execution_id: dispatch.execution_id,
            output: json!({"ok":false}),
            mutation: mutation("settle-late", "settle", json!({"execution":"late"})),
        })
        .await
        .unwrap();
    assert!(!late.accepted);

    let state = ledger.replay().await.unwrap();
    let prefix = store.read_prefix(&resource).await.unwrap();
    let pure = fold_records(&resource, &prefix.records).unwrap();
    assert_eq!(
        state.canonical_bytes().unwrap(),
        pure.canonical_bytes().unwrap()
    );
    assert_eq!(state.mutation_receipts, pure.mutation_receipts);

    let reopened = ClusterLedger::open(store, resource, OwnerId::new("owner").unwrap(), 1000)
        .await
        .unwrap();
    assert_eq!(
        state.canonical_bytes().unwrap(),
        reopened.replay().await.unwrap().canonical_bytes().unwrap()
    );
}

#[tokio::test]
async fn safe_fault_and_terminal_consequence_are_one_immutable_batch() {
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(MemoryLedgerStore::new(clock));
    let ledger = ClusterLedger::create(
        store,
        ResourceId::new("fault-cluster").unwrap(),
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
            mutation: mutation("admit", "admit", json!({"graph":"fixture"})),
        })
        .await
        .unwrap();
    let fault = FaultFactory::new(&NoopObservationSink).create(ModuleEvidence::new(
        FaultModule::Storage,
        FaultContext::Execution,
        EvidenceClass::Unavailable,
    ));
    ledger
        .persist_safe_fault(
            None,
            &fault,
            TerminalOutcome::Failed,
            mutation("fault", "safe_fault", json!({"fault":"safe"})),
        )
        .await
        .unwrap();
    let state = ledger.replay().await.unwrap();
    assert_eq!(state.terminal_outcome, Some(TerminalOutcome::Failed));
    assert_eq!(state.safe_faults, vec![fault.encode_json().unwrap()]);
    assert!(
        ledger
            .dispatch(DispatchRequest {
                turn_id: "too-late".into(),
                mutation: mutation("late", "dispatch", json!({"turn":"late"})),
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn ordinary_terminalization_rejects_live_dispatches_without_erasing_them() {
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(MemoryLedgerStore::new(clock));
    let ledger = ClusterLedger::create(
        store,
        ResourceId::new("live-terminal").unwrap(),
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
            mutation: mutation("admit", "admit", json!({"graph":"fixture"})),
        })
        .await
        .unwrap();
    let dispatch = ledger
        .dispatch(DispatchRequest {
            turn_id: "live-turn".into(),
            mutation: mutation("dispatch", "dispatch", json!({"turn":"live-turn"})),
        })
        .await
        .unwrap();

    assert!(matches!(
        ledger
            .terminalize(
                TerminalOutcome::Succeeded,
                mutation("terminal", "terminalize", json!({"outcome":"succeeded"})),
            )
            .await,
        Err(LedgerError::IllegalTransition)
    ));
    let state = ledger.replay().await.unwrap();
    assert_eq!(state.phase, zeroshot_engine::ledger::LedgerPhase::Running);
    assert!(state.active_dispatches.contains_key(&dispatch.execution_id));
}

#[tokio::test]
async fn duplicate_effects_and_conflicting_cleanup_receipts_append_nothing() {
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(MemoryLedgerStore::new(clock));
    let ledger = ClusterLedger::create(
        store,
        ResourceId::new("conflicting-effects-cleanup").unwrap(),
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
            mutation: mutation("admit", "admit", json!({"graph":"fixture"})),
        })
        .await
        .unwrap();
    let dispatch = ledger
        .dispatch(DispatchRequest {
            turn_id: "effect-turn".into(),
            mutation: mutation("dispatch", "dispatch", json!({"turn":"effect-turn"})),
        })
        .await
        .unwrap();
    ledger
        .record_effect_intent(
            dispatch.execution_id.clone(),
            "effect".into(),
            "a".repeat(64),
            mutation("effect-one", "effect_intent", json!({"effect":1})),
        )
        .await
        .unwrap();
    let before_duplicate = ledger.replay().await.unwrap().at_position;
    assert!(matches!(
        ledger
            .record_effect_intent(
                dispatch.execution_id.clone(),
                "effect".into(),
                "b".repeat(64),
                mutation("effect-two", "effect_intent", json!({"effect":2})),
            )
            .await,
        Err(LedgerError::IllegalTransition)
    ));
    assert_eq!(ledger.replay().await.unwrap().at_position, before_duplicate);

    ledger
        .settle(SettlementRequest {
            execution_id: dispatch.execution_id,
            output: json!({"ok":true}),
            mutation: mutation("settle", "settle", json!({"result":"ok"})),
        })
        .await
        .unwrap();
    ledger
        .terminalize(
            TerminalOutcome::Succeeded,
            mutation("terminal", "terminalize", json!({"outcome":"succeeded"})),
        )
        .await
        .unwrap();
    ledger
        .record_cleanup(
            "workspace".into(),
            "c".repeat(64),
            mutation("cleanup-one", "cleanup", json!({"cleanup":1})),
        )
        .await
        .unwrap();
    let before_conflict = ledger.replay().await.unwrap().at_position;
    assert!(matches!(
        ledger
            .record_cleanup(
                "workspace".into(),
                "d".repeat(64),
                mutation("cleanup-two", "cleanup", json!({"cleanup":2})),
            )
            .await,
        Err(LedgerError::IllegalTransition)
    ));
    let state = ledger.replay().await.unwrap();
    assert_eq!(state.at_position, before_conflict);
    assert_eq!(
        state.cleanup_receipts.get("workspace"),
        Some(&"c".repeat(64))
    );
}

#[tokio::test]
async fn dispatch_idempotency_is_scoped_to_the_durable_run() {
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(MemoryLedgerStore::new(clock));
    let ledger = Arc::new(
        ClusterLedger::create(
            store,
            ResourceId::new("run-scoped-dispatch").unwrap(),
            OwnerId::new("owner").unwrap(),
            1_000,
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
            mutation: mutation("admit-1", "admit", json!({"generation":1})),
        })
        .await
        .unwrap();
    let adapters = LedgerAdapters::new(ledger.clone());
    let first = adapters
        .acquire_dispatch(TurnId::new("reused-turn"))
        .await
        .unwrap();
    adapters
        .complete_dispatch(VerifiedCompletion {
            lease_id: first.lease_id.clone(),
            output: json!({"generation":1}),
        })
        .await
        .unwrap();

    let (graph, compiled_ir) = graph_and_ir();
    ledger
        .admit(AdmissionRequest {
            graph,
            compiled_ir,
            input: json!({}),
            deadline: AbsoluteDeadline::from_unix_millis(20_000).unwrap(),
            mutation: mutation("admit-2", "admit", json!({"generation":2})),
        })
        .await
        .unwrap();
    let second = adapters
        .acquire_dispatch(TurnId::new("reused-turn"))
        .await
        .unwrap();
    assert_ne!(second.lease_id, first.lease_id);
    assert_eq!(
        ledger.replay().await.unwrap().generation().unwrap().get(),
        2
    );
}

#[tokio::test]
async fn unresolved_effects_survive_readmission_and_can_reconcile_after_terminalization() {
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(MemoryLedgerStore::new(clock));
    let ledger = ClusterLedger::create(
        store,
        ResourceId::new("recoverable-effects").unwrap(),
        OwnerId::new("owner").unwrap(),
        1_000,
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
            mutation: mutation("admit-1", "admit", json!({"generation":1})),
        })
        .await
        .unwrap();
    let dispatch = ledger
        .dispatch(DispatchRequest {
            turn_id: "effect-turn".into(),
            mutation: mutation("dispatch-1", "dispatch", json!({"generation":1})),
        })
        .await
        .unwrap();
    ledger
        .record_effect_intent(
            dispatch.execution_id.clone(),
            "external-effect".into(),
            "a".repeat(64),
            mutation("intent", "effect_intent", json!({"effect":"external"})),
        )
        .await
        .unwrap();
    ledger
        .settle(SettlementRequest {
            execution_id: dispatch.execution_id,
            output: json!({"ok":true}),
            mutation: mutation("settle-1", "settle", json!({"generation":1})),
        })
        .await
        .unwrap();

    let (graph, compiled_ir) = graph_and_ir();
    ledger
        .admit(AdmissionRequest {
            graph,
            compiled_ir,
            input: json!({}),
            deadline: AbsoluteDeadline::from_unix_millis(20_000).unwrap(),
            mutation: mutation("admit-2", "admit", json!({"generation":2})),
        })
        .await
        .unwrap();
    assert!(
        ledger
            .replay()
            .await
            .unwrap()
            .effects
            .contains_key("external-effect")
    );
    ledger
        .terminalize(
            TerminalOutcome::Succeeded,
            mutation("terminal", "terminalize", json!({"outcome":"succeeded"})),
        )
        .await
        .unwrap();
    assert!(matches!(
        ledger.remove_terminal().await,
        Err(LedgerError::IllegalTransition)
    ));
    ledger
        .reconcile_effect(
            "external-effect".into(),
            "b".repeat(64),
            mutation(
                "effect-receipt",
                "effect_receipt",
                json!({"effect":"external"}),
            ),
        )
        .await
        .unwrap();
    let state = ledger.replay().await.unwrap();
    assert_eq!(state.phase, zeroshot_engine::ledger::LedgerPhase::Terminal);
    assert_eq!(state.terminal_outcome, Some(TerminalOutcome::Succeeded));
    assert_eq!(
        state.effects["external-effect"].reconciliation_digest,
        Some("b".repeat(64))
    );
}

#[tokio::test]
async fn hash_consistent_apply_update_and_settlement_receipt_forgery_fails_replay() {
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(MemoryLedgerStore::new(clock));
    let resource = ResourceId::new("typed-receipts").unwrap();
    let ledger = Arc::new(
        ClusterLedger::create(
            store.clone(),
            resource.clone(),
            OwnerId::new("owner").unwrap(),
            1000,
        )
        .await
        .unwrap(),
    );
    let adapters = LedgerAdapters::new(ledger.clone());
    let (graph, compiled_ir) = graph_and_ir();
    adapters
        .commit(
            CommitProposal {
                graph,
                compiled_ir,
                input: Some(json!({})),
                if_generation: None,
                idempotency_key: IdempotencyKey::new("apply").unwrap(),
                fingerprint: serde_json::from_value(json!("a".repeat(64))).unwrap(),
            },
            &CancellationSignal::default(),
        )
        .await
        .unwrap();
    let prefix = store.read_prefix(&resource).await.unwrap();
    let forged = mutate_and_rehash(prefix.records, 1, |payload| {
        let RecordPayload::MutationReceipt { receipt, .. } = payload else {
            panic!("apply receipt missing");
        };
        let ClosedMutationReceipt::Apply(receipt) = receipt else {
            panic!("apply receipt type mismatch");
        };
        receipt.result.phase = Phase::Finished;
        receipt.result.diff = None;
    });
    assert!(fold_records(&resource, &forged).is_err());

    let params = UpdateParams {
        labels: None,
        log_level: None,
        suspended: Some(true),
        if_generation: Generation::new(1).unwrap(),
        idempotency_key: IdempotencyKey::new("update").unwrap(),
    };
    let fingerprint =
        admission_fingerprint("update", &serde_json::to_value(&params).unwrap()).unwrap();
    adapters
        .update_lifecycle(UpdateProposal {
            params,
            fingerprint,
        })
        .await
        .unwrap();
    let prefix = store.read_prefix(&resource).await.unwrap();
    let last = prefix.records.len() - 1;
    let forged = mutate_and_rehash(prefix.records, last, |payload| {
        let RecordPayload::MutationReceipt { receipt, .. } = payload else {
            panic!("update receipt missing");
        };
        let ClosedMutationReceipt::Update(receipt) = receipt else {
            panic!("update receipt type mismatch");
        };
        receipt.result.at_cursor = Cursor::new("forged-cursor");
        receipt.result.operational.in_flight = 99;
    });
    assert!(fold_records(&resource, &forged).is_err());

    let resume = UpdateParams {
        labels: None,
        log_level: None,
        suspended: Some(false),
        if_generation: Generation::new(1).unwrap(),
        idempotency_key: IdempotencyKey::new("resume").unwrap(),
    };
    let fingerprint =
        admission_fingerprint("update", &serde_json::to_value(&resume).unwrap()).unwrap();
    adapters
        .update_lifecycle(UpdateProposal {
            params: resume,
            fingerprint,
        })
        .await
        .unwrap();
    let dispatch = ledger
        .dispatch(DispatchRequest {
            turn_id: "settle-turn".into(),
            mutation: mutation("dispatch", "dispatch", json!({"turn":"settle-turn"})),
        })
        .await
        .unwrap();
    ledger
        .settle(SettlementRequest {
            execution_id: dispatch.execution_id,
            output: json!({"ok":true}),
            mutation: mutation("settle", "settle", json!({"result":"ok"})),
        })
        .await
        .unwrap();
    let prefix = store.read_prefix(&resource).await.unwrap();
    let last = prefix.records.len() - 1;
    let forged = mutate_and_rehash(prefix.records, last, |payload| {
        let RecordPayload::MutationReceipt { receipt, .. } = payload else {
            panic!("settlement receipt missing");
        };
        let ClosedMutationReceipt::Settle(receipt) = receipt else {
            panic!("settlement receipt type mismatch");
        };
        receipt.terminalized = true;
    });
    assert!(fold_records(&resource, &forged).is_err());
}

#[tokio::test]
async fn force_stop_replay_requires_every_void_and_exact_receipt() {
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(MemoryLedgerStore::new(clock));
    let resource = ResourceId::new("force-receipt").unwrap();
    let ledger = Arc::new(
        ClusterLedger::create(
            store.clone(),
            resource.clone(),
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
            mutation: mutation("admit", "admit", json!({"graph":"fixture"})),
        })
        .await
        .unwrap();
    let adapters = LedgerAdapters::new(ledger);
    adapters
        .acquire_dispatch(TurnId::new("turn-a"))
        .await
        .unwrap();
    adapters
        .acquire_dispatch(TurnId::new("turn-b"))
        .await
        .unwrap();
    let params = StopParams {
        mode: StopMode::Force,
        if_generation: Generation::new(1).unwrap(),
        idempotency_key: IdempotencyKey::new("stop").unwrap(),
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
    let prefix = store.read_prefix(&resource).await.unwrap();
    let void_indexes = prefix
        .records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            matches!(record.decode_payload().unwrap(), RecordPayload::Void { .. }).then_some(index)
        })
        .collect::<Vec<_>>();
    assert_eq!(void_indexes.len(), 2);
    let first_execution = match prefix.records[void_indexes[0]].decode_payload().unwrap() {
        RecordPayload::Void { execution_id } => execution_id,
        _ => unreachable!(),
    };
    let forged_voids = mutate_and_rehash(prefix.records.clone(), void_indexes[1], |payload| {
        let RecordPayload::Void { execution_id } = payload else {
            panic!("void missing");
        };
        *execution_id = first_execution;
    });
    assert!(fold_records(&resource, &forged_voids).is_err());

    let last = prefix.records.len() - 1;
    let forged_receipt = mutate_and_rehash(prefix.records, last, |payload| {
        let RecordPayload::MutationReceipt { receipt, .. } = payload else {
            panic!("stop receipt missing");
        };
        let ClosedMutationReceipt::Stop(receipt) = receipt else {
            panic!("stop receipt type mismatch");
        };
        receipt.result.effective_mode = StopMode::Drain;
    });
    assert!(fold_records(&resource, &forged_receipt).is_err());
}

fn mutate_and_rehash(
    mut records: Vec<LedgerRecord>,
    index: usize,
    mutate: impl FnOnce(&mut RecordPayload),
) -> Vec<LedgerRecord> {
    let mut payloads = records
        .iter()
        .map(|record| record.decode_payload().unwrap())
        .collect::<Vec<_>>();
    mutate(&mut payloads[index]);
    let mut previous_hash = index
        .checked_sub(1)
        .map_or([0; 32], |previous| records[previous].record_hash);
    for offset in index..records.len() {
        records[offset] = LedgerRecord::new(
            records[offset].resource_id.clone(),
            records[offset].sequence,
            &payloads[offset],
            previous_hash,
        )
        .unwrap();
        previous_hash = records[offset].record_hash;
    }
    records
}

#[test]
fn unpaired_receipt_unknown_method_version_gap_and_hash_fail_closed() {
    let resource = ResourceId::new("corrupt-cluster").unwrap();
    let payload = zeroshot_engine::ledger::RecordPayload::MutationReceipt {
        key: IdempotencyId::new("key").unwrap(),
        fingerprint: [1; 32],
        receipt: zeroshot_engine::ledger::record::ClosedMutationReceipt::Terminalize(
            zeroshot_engine::ledger::MutationReceipt {
                at_position: zeroshot_engine::ledger::Position::new(1).unwrap(),
                deduped: false,
            },
        ),
    };
    let base = zeroshot_engine::ledger::record::LedgerRecord::new(
        resource.clone(),
        zeroshot_engine::ledger::Position::new(1).unwrap(),
        &payload,
        [0; 32],
    )
    .unwrap();
    assert!(fold_records(&resource, std::slice::from_ref(&base)).is_err());
    let mut unknown_method = serde_json::to_value(&payload).unwrap();
    unknown_method["receipt"]["method"] = json!("unknown");
    assert!(
        serde_json::from_value::<zeroshot_engine::ledger::RecordPayload>(unknown_method).is_err()
    );
    let mut unknown = base.clone();
    unknown.version = 2;
    assert!(fold_records(&resource, &[unknown]).is_err());
    let mut hash = base.clone();
    hash.record_hash[0] ^= 1;
    assert!(fold_records(&resource, &[hash]).is_err());
    let mut gap = base;
    gap.sequence = zeroshot_engine::ledger::Position::new(2).unwrap();
    assert!(fold_records(&resource, &[gap]).is_err());
}
