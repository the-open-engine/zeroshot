mod support;

use std::sync::Arc;
use std::time::Duration;

use openengine_cluster_protocol::{
    admission_fingerprint, Generation, IdempotencyKey, LogLevel, Phase, StopMode, StopParams,
    UpdateParams,
};
use openengine_cluster_server::admission::{AdmissionStore, StoreError as AdmissionStoreError};
use openengine_cluster_server::lifecycle::{
    LifecycleStore, StopProposal, TurnId, UpdateProposal, VerifiedCompletion,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use support::ledger::{graph_and_ir, DispatchRaceStore, ManualClock};
use tempfile::tempdir;
use zeroshot_engine::ledger::record::{LedgerRecord, RecordPayload};
use zeroshot_engine::ledger::store::{AppendRequest, LedgerStore, OpaqueMutationReceipt, StoreError};
use zeroshot_engine::ledger::{OwnerId, Position, ResourceId, SqliteLedgerStore};
use zeroshot_engine::ledger::{
    AbsoluteDeadline, AdmissionRequest, ClusterLedger, DispatchRequest, IdempotencyId, LedgerError,
    MemoryLedgerStore, MutationIdentity,
};
use zeroshot_engine::fault::FaultCode;
use zeroshot_engine::ledger::adapters::LedgerAdapters;

#[tokio::test]
async fn sqlite_uses_hardened_settings_and_digest_named_resource_database() {
    let directory = tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(100));
    let store = SqliteLedgerStore::new(directory.path(), clock).unwrap();
    let resource = ResourceId::new("settings-cluster").unwrap();
    store.create_resource(&resource).await.unwrap();
    let path = store.database_path(&resource);
    let stem = path.file_stem().unwrap().to_str().unwrap();
    assert_eq!(stem.len(), 64);
    assert!(
        stem.bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    let settings = store.settings(&resource).await.unwrap();
    assert_eq!(settings.journal_mode.to_ascii_lowercase(), "wal");
    assert_eq!(settings.synchronous, 2);
    assert!(settings.foreign_keys);
    assert_eq!(settings.busy_timeout_millis, 5000);
}

#[tokio::test]
async fn sqlite_discovery_is_ordered_and_fails_before_unbounded_directory_scans() {
    let directory = tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(100));
    let store = SqliteLedgerStore::new(directory.path(), clock).unwrap();
    for name in ["resource-z", "resource-a", "resource-m"] {
        store
            .create_resource(&ResourceId::new(name).unwrap())
            .await
            .unwrap();
    }
    let first = store.list_resources(None, 2).await.unwrap();
    assert_eq!(
        first
            .resources
            .iter()
            .map(|metadata| metadata.resource_id.as_str())
            .collect::<Vec<_>>(),
        ["resource-a", "resource-m"]
    );
    assert_eq!(first.next_after.as_ref().unwrap().as_str(), "resource-m");
    let second = store
        .list_resources(first.next_after.as_ref(), 2)
        .await
        .unwrap();
    assert_eq!(second.resources.len(), 1);
    assert_eq!(second.resources[0].resource_id.as_str(), "resource-z");
    assert!(second.next_after.is_none());

    let crowded = tempdir().unwrap();
    let crowded_store =
        SqliteLedgerStore::new(crowded.path(), Arc::new(ManualClock::at(100))).unwrap();
    for index in 0..4097 {
        std::fs::File::create(crowded.path().join(format!("entry-{index}"))).unwrap();
    }
    assert!(matches!(
        crowded_store.list_resources(None, 1).await,
        Err(StoreError::BoundExceeded(
            "discovery scan exceeds 4096 directory entries"
        ))
    ));
}

#[tokio::test]
async fn sqlite_create_recovers_an_unversioned_partial_initialization_file() {
    let directory = tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(100));
    let store = SqliteLedgerStore::new(directory.path(), clock).unwrap();
    let resource = ResourceId::new("partial-initialization").unwrap();
    std::fs::File::create(store.database_path(&resource)).unwrap();
    let metadata = store.create_resource(&resource).await.unwrap();
    assert_eq!(metadata.resource_id, resource);
    assert_eq!(metadata.position.get(), 0);
    assert_eq!(
        store
            .settings(&metadata.resource_id)
            .await
            .unwrap()
            .journal_mode,
        "wal"
    );
}

#[tokio::test]
async fn sqlite_fence_expiry_takeover_and_stale_owner_rejection_are_deterministic() {
    let directory = tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(100));
    let store = SqliteLedgerStore::new(directory.path(), clock.clone()).unwrap();
    let resource = ResourceId::new("fence-cluster").unwrap();
    store.create_resource(&resource).await.unwrap();
    let first = store
        .acquire_fence(&resource, &OwnerId::new("first").unwrap(), 10)
        .await
        .unwrap();
    assert!(
        store
            .acquire_fence(&resource, &OwnerId::new("second").unwrap(), 10)
            .await
            .is_err()
    );
    clock.advance(10);
    let second = store
        .acquire_fence(&resource, &OwnerId::new("second").unwrap(), 10)
        .await
        .unwrap();
    assert!(second.epoch > first.epoch);
    assert!(store.validate_fence(&resource, &first).await.is_err());
    store.validate_fence(&resource, &second).await.unwrap();
}

#[tokio::test]
async fn sqlite_samples_fence_time_only_after_acquiring_the_write_transaction() {
    let directory = tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(SqliteLedgerStore::new(directory.path(), clock.clone()).unwrap());
    let resource = ResourceId::new("transaction-clock").unwrap();
    store.create_resource(&resource).await.unwrap();
    let fence = store
        .acquire_fence(&resource, &OwnerId::new("owner").unwrap(), 10)
        .await
        .unwrap();
    let connection = rusqlite::Connection::open(store.database_path(&resource)).unwrap();
    connection
        .execute_batch("PRAGMA busy_timeout=5000; BEGIN IMMEDIATE")
        .unwrap();
    let reads_before_wait = clock.read_count();
    let renewal = {
        let store = store.clone();
        let resource = resource.clone();
        let fence = fence.clone();
        tokio::spawn(async move { store.renew_fence(&resource, &fence, 100).await })
    };
    let clock_waiter = clock.clone();
    let sampled_while_blocked = tokio::task::spawn_blocking(move || {
        clock_waiter.wait_for_read_after(reads_before_wait, Duration::from_millis(100))
    })
    .await
    .unwrap();
    assert!(
        !sampled_while_blocked,
        "clock must not be sampled while BEGIN IMMEDIATE is blocked"
    );
    clock.advance(10);
    connection.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        renewal.await.unwrap().unwrap_err(),
        zeroshot_engine::ledger::StoreError::FenceRejected
    );
}

#[tokio::test]
async fn sqlite_waiter_observes_a_commit_from_an_independent_store_instance() {
    let directory = tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(100));
    let waiting_store = Arc::new(SqliteLedgerStore::new(directory.path(), clock.clone()).unwrap());
    let writing_store = Arc::new(SqliteLedgerStore::new(directory.path(), clock).unwrap());
    let resource = ResourceId::new("cross-instance-wait").unwrap();
    waiting_store.create_resource(&resource).await.unwrap();
    let fence = waiting_store
        .acquire_fence(&resource, &OwnerId::new("owner").unwrap(), 1_000)
        .await
        .unwrap();
    let waiter = {
        let store = waiting_store.clone();
        let resource = resource.clone();
        tokio::spawn(async move { store.wait_for_advancement(&resource, Position::ZERO).await })
    };
    tokio::task::yield_now().await;
    let record = LedgerRecord::new(
        resource.clone(),
        Position::new(1).unwrap(),
        &RecordPayload::LifecycleUpdate {
            labels: None,
            log_level: None,
            suspended: Some(false),
        },
        [0; 32],
    )
    .unwrap();
    writing_store
        .compare_and_append(
            &resource,
            AppendRequest {
                expected_position: Position::ZERO,
                fence,
                records: vec![record],
                receipt: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Position::new(1).unwrap()
    );
}

#[tokio::test]
async fn aggregate_reads_are_coherent_while_control_and_lifecycle_advance() {
    let directory = tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(SqliteLedgerStore::new(directory.path(), clock).unwrap());
    let ledger = Arc::new(
        ClusterLedger::create(
            store,
            ResourceId::new("coherent-cluster").unwrap(),
            OwnerId::new("owner").unwrap(),
            10_000,
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
            mutation: MutationIdentity::for_value(
                IdempotencyId::new("admit").unwrap(),
                "admit",
                &json!({"graph":"fixture"}),
            )
            .unwrap(),
        })
        .await
        .unwrap();
    let adapters = LedgerAdapters::new(ledger);
    let writer = {
        let adapters = adapters.clone();
        tokio::spawn(async move {
            for index in 0..25 {
                let params = UpdateParams {
                    labels: None,
                    log_level: Some(if index % 2 == 0 {
                        LogLevel::Debug
                    } else {
                        LogLevel::Info
                    }),
                    suspended: None,
                    if_generation: Generation::new(1).unwrap(),
                    idempotency_key: IdempotencyKey::new(format!("update-{index}")).unwrap(),
                };
                let fingerprint =
                    admission_fingerprint("update", &serde_json::to_value(&params).unwrap())
                        .unwrap();
                adapters
                    .update_lifecycle(UpdateProposal {
                        params,
                        fingerprint,
                    })
                    .await
                    .unwrap();
                tokio::task::yield_now().await;
            }
        })
    };
    while !writer.is_finished() {
        let (admission, lifecycle) = adapters.read_aggregate().await.unwrap();
        assert_eq!(admission.control.cursor, lifecycle.latest_cursor);
        assert_eq!(
            admission.control.phase,
            openengine_cluster_protocol::Phase::Running
        );
        assert!(lifecycle.operational.is_some());
        tokio::task::yield_now().await;
    }
    writer.await.unwrap();
    let (admission, lifecycle) = adapters.read_aggregate().await.unwrap();
    assert_eq!(admission.control.cursor, lifecycle.latest_cursor);
}

#[tokio::test]
async fn corrupt_record_versions_and_receipts_map_through_fault_factory() {
    let directory = tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(SqliteLedgerStore::new(directory.path(), clock).unwrap());
    for (resource_name, corruption) in [
        (
            "corrupt-version",
            "UPDATE records SET version = 99 WHERE sequence = 1",
        ),
        (
            "corrupt-receipt",
            "UPDATE receipts SET value = X'00' WHERE idempotency_key = 'admit'",
        ),
        (
            "corrupt-hash",
            "UPDATE records SET record_hash = zeroblob(32) WHERE sequence = 1",
        ),
        ("corrupt-gap", "UPDATE records SET sequence = sequence + 10"),
    ] {
        let resource = ResourceId::new(resource_name).unwrap();
        let ledger = ClusterLedger::create(
            store.clone(),
            resource.clone(),
            OwnerId::new("owner").unwrap(),
            10_000,
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
                mutation: MutationIdentity::for_value(
                    IdempotencyId::new("admit").unwrap(),
                    "admit",
                    &json!({"graph":"fixture"}),
                )
                .unwrap(),
            })
            .await
            .unwrap();
        let connection = rusqlite::Connection::open(store.database_path(&resource)).unwrap();
        connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        connection.execute(corruption, []).unwrap();
        drop(connection);
        let LedgerError::Fault(fault) = ledger.replay().await.unwrap_err() else {
            panic!("corruption must map to a safe fault");
        };
        assert_eq!(fault.code(), FaultCode::IntegrityFailure);
    }
}

#[tokio::test]
async fn bounded_range_reads_do_not_scan_receipts_outside_the_requested_range() {
    let directory = tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(SqliteLedgerStore::new(directory.path(), clock).unwrap());
    let resource = ResourceId::new("bounded-receipt-range").unwrap();
    let ledger = ClusterLedger::create(
        store.clone(),
        resource.clone(),
        OwnerId::new("owner").unwrap(),
        10_000,
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
            mutation: MutationIdentity::for_value(
                IdempotencyId::new("admit").unwrap(),
                "admit",
                &json!({"graph":"fixture"}),
            )
            .unwrap(),
        })
        .await
        .unwrap();
    ledger
        .dispatch(DispatchRequest {
            turn_id: "turn".into(),
            mutation: MutationIdentity::for_value(
                IdempotencyId::new("dispatch").unwrap(),
                "dispatch",
                &json!({"turn":"turn"}),
            )
            .unwrap(),
        })
        .await
        .unwrap();
    let connection = rusqlite::Connection::open(store.database_path(&resource)).unwrap();
    connection
        .execute(
            "UPDATE receipts SET fingerprint = X'00' WHERE at_position = 2",
            [],
        )
        .unwrap();
    drop(connection);

    let range = store
        .read_range(&resource, Position::new(2).unwrap(), 1)
        .await
        .unwrap();
    assert_eq!(range.end, Position::new(3).unwrap());
    assert_eq!(range.records.len(), 1);
    assert!(range.receipts.is_empty());
    assert_eq!(
        store.read_prefix(&resource).await.unwrap_err(),
        StoreError::Corrupt
    );
}

#[tokio::test]
async fn orphan_receipt_rows_and_missing_range_tails_fail_closed() {
    let directory = tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(SqliteLedgerStore::new(directory.path(), clock).unwrap());
    let resource = ResourceId::new("orphan-and-tail").unwrap();
    let ledger = ClusterLedger::create(
        store.clone(),
        resource.clone(),
        OwnerId::new("owner").unwrap(),
        10_000,
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
            mutation: MutationIdentity::for_value(
                IdempotencyId::new("admit").unwrap(),
                "admit",
                &json!({"graph":"fixture"}),
            )
            .unwrap(),
        })
        .await
        .unwrap();
    let path = store.database_path(&resource);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO receipts(\
                resource_id, idempotency_key, method, fingerprint, value, at_position\
             ) VALUES(?1, 'orphan', 'admit', zeroblob(32), X'00', 1)",
            [resource.as_str()],
        )
        .unwrap();
    drop(connection);
    assert!(matches!(ledger.replay().await, Err(LedgerError::Fault(_))));

    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute("DELETE FROM receipts WHERE idempotency_key = 'orphan'", [])
        .unwrap();
    connection
        .execute("DELETE FROM records WHERE sequence = 2", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        store
            .read_range(&resource, Position::ZERO, 4096)
            .await
            .unwrap_err(),
        zeroshot_engine::ledger::StoreError::Corrupt
    );
    assert_eq!(
        store
            .read_range(&resource, Position::new(1).unwrap(), 4096)
            .await
            .unwrap_err(),
        zeroshot_engine::ledger::StoreError::Corrupt
    );
}

#[tokio::test]
async fn append_and_idempotent_replay_reject_a_corrupt_authoritative_tail() {
    let directory = tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(100));
    let store = SqliteLedgerStore::new(directory.path(), clock).unwrap();
    for (resource_name, replay) in [
        ("corrupt-replay-tail", true),
        ("corrupt-append-tail", false),
    ] {
        let resource = ResourceId::new(resource_name).unwrap();
        store.create_resource(&resource).await.unwrap();
        let fence = store
            .acquire_fence(&resource, &OwnerId::new("owner").unwrap(), 1000)
            .await
            .unwrap();
        let first = LedgerRecord::new(
            resource.clone(),
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
            key: IdempotencyId::new("tail-mutation").unwrap(),
            method: "contract".into(),
            fingerprint: [7; 32],
            value: b"receipt".to_vec(),
            at_position: Position::new(1).unwrap(),
        };
        let first_request = AppendRequest {
            expected_position: Position::ZERO,
            fence: fence.clone(),
            records: vec![first.clone()],
            receipt: Some(receipt),
        };
        store
            .compare_and_append(&resource, first_request.clone())
            .await
            .unwrap();
        let connection = rusqlite::Connection::open(store.database_path(&resource)).unwrap();
        connection
            .execute(
                "UPDATE records SET record_hash = zeroblob(32) WHERE sequence = 1",
                [],
            )
            .unwrap();
        drop(connection);
        let request = if replay {
            first_request
        } else {
            AppendRequest {
                expected_position: Position::new(1).unwrap(),
                fence,
                records: vec![
                    LedgerRecord::new(
                        resource.clone(),
                        Position::new(2).unwrap(),
                        &RecordPayload::LifecycleUpdate {
                            labels: None,
                            log_level: None,
                            suspended: Some(true),
                        },
                        first.record_hash,
                    )
                    .unwrap(),
                ],
                receipt: None,
            }
        };
        assert_eq!(
            store
                .compare_and_append(&resource, request)
                .await
                .unwrap_err(),
            StoreError::Corrupt
        );
        assert_eq!(
            store.open_resource(&resource).await.unwrap().position.get(),
            1
        );
    }
}

#[tokio::test]
async fn hash_consistent_noncanonical_payload_maps_through_fault_factory() {
    let directory = tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(SqliteLedgerStore::new(directory.path(), clock).unwrap());
    let resource = ResourceId::new("noncanonical-payload").unwrap();
    let ledger = ClusterLedger::create(
        store.clone(),
        resource.clone(),
        OwnerId::new("owner").unwrap(),
        10_000,
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
            mutation: MutationIdentity::for_value(
                IdempotencyId::new("admit").unwrap(),
                "admit",
                &json!({"graph":"fixture"}),
            )
            .unwrap(),
        })
        .await
        .unwrap();
    make_first_payload_noncanonical_and_rehash(&store.database_path(&resource), &resource);
    let LedgerError::Fault(fault) = ledger.replay().await.unwrap_err() else {
        panic!("noncanonical payload must map to a safe fault");
    };
    assert_eq!(fault.code(), FaultCode::IntegrityFailure);
}

fn make_first_payload_noncanonical_and_rehash(path: &std::path::Path, resource: &ResourceId) {
    let connection = rusqlite::Connection::open(path).unwrap();
    let mut statement = connection
        .prepare("SELECT sequence, family, kind, version, payload FROM records ORDER BY sequence")
        .unwrap();
    let mut rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    drop(statement);
    let value: serde_json::Value = serde_json::from_slice(&rows[0].4).unwrap();
    rows[0].4 = serde_json::to_vec_pretty(&value).unwrap();
    let mut previous_hash = [0; 32];
    for (sequence, family, kind, version, payload) in rows {
        let record_hash = calculate_record_hash(
            resource.as_str(),
            u64::try_from(sequence).unwrap(),
            &family,
            &kind,
            u16::try_from(version).unwrap(),
            &payload,
            previous_hash,
        );
        connection
            .execute(
                "UPDATE records SET payload = ?1, previous_hash = ?2, record_hash = ?3 WHERE sequence = ?4",
                rusqlite::params![payload, previous_hash.as_slice(), record_hash.as_slice(), sequence],
            )
            .unwrap();
        previous_hash = record_hash;
    }
}

fn calculate_record_hash(
    resource: &str,
    sequence: u64,
    family: &str,
    kind: &str,
    version: u16,
    payload: &[u8],
    previous_hash: [u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"openengine.cluster-ledger.record.v1\0");
    hash_field(&mut hasher, resource.as_bytes());
    hasher.update(sequence.to_be_bytes());
    hash_field(&mut hasher, family.as_bytes());
    hash_field(&mut hasher, kind.as_bytes());
    hasher.update(version.to_be_bytes());
    hasher.update(Sha256::digest(payload));
    hasher.update(previous_hash);
    hasher.finalize().into()
}

fn hash_field(hasher: &mut Sha256, field: &[u8]) {
    hasher.update(u64::try_from(field.len()).unwrap().to_be_bytes());
    hasher.update(field);
}

#[tokio::test]
async fn force_stop_durably_voids_dispatch_and_cancels_live_permit() {
    let directory = tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(SqliteLedgerStore::new(directory.path(), clock).unwrap());
    let ledger = Arc::new(
        ClusterLedger::create(
            store,
            ResourceId::new("force-stop-cluster").unwrap(),
            OwnerId::new("owner").unwrap(),
            10_000,
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
            mutation: MutationIdentity::for_value(
                IdempotencyId::new("admit").unwrap(),
                "admit",
                &json!({"graph":"fixture"}),
            )
            .unwrap(),
        })
        .await
        .unwrap();
    let adapters = LedgerAdapters::new(ledger);
    let permit = adapters
        .acquire_dispatch(TurnId::new("turn"))
        .await
        .unwrap();
    let duplicate = adapters
        .acquire_dispatch(TurnId::new("turn"))
        .await
        .unwrap();
    assert_eq!(duplicate.lease_id, permit.lease_id);
    let params = StopParams {
        mode: StopMode::Force,
        if_generation: Generation::new(1).unwrap(),
        idempotency_key: IdempotencyKey::new("stop").unwrap(),
    };
    let fingerprint =
        admission_fingerprint("stop", &serde_json::to_value(&params).unwrap()).unwrap();
    let result = adapters
        .stop_lifecycle(StopProposal {
            params,
            fingerprint,
        })
        .await
        .unwrap();
    assert_eq!(result.phase, Phase::Finished);
    assert!(permit.cancellation.is_cancelled());
    assert!(duplicate.cancellation.is_cancelled());
    assert_eq!(
        adapters
            .complete_dispatch(VerifiedCompletion {
                lease_id: permit.lease_id.clone(),
                output: json!({"late": true}),
            })
            .await
            .unwrap_err(),
        AdmissionStoreError::CompletionRejected
    );
    assert_eq!(
        adapters
            .acquire_dispatch(TurnId::new("after-stop"))
            .await
            .unwrap_err(),
        AdmissionStoreError::DispatchDenied {
            current: openengine_cluster_protocol::DispatchState::Stopped,
        }
    );
    let snapshot = adapters.read_lifecycle_snapshot().await.unwrap();
    assert_eq!(snapshot.void_turns.len(), 1);
    assert_eq!(
        snapshot
            .records
            .iter()
            .filter(|record| matches!(
                record.event,
                openengine_cluster_server::lifecycle::LifecycleEvent::Finished { .. }
            ))
            .count(),
        1
    );
}

#[derive(Clone, Copy)]
enum DispatchStopRace {
    BeforeReplay,
    BeforeAppend,
}

#[tokio::test]
async fn concurrent_stop_denies_dispatch_at_replay_and_append_boundaries() {
    for race in [
        DispatchStopRace::BeforeReplay,
        DispatchStopRace::BeforeAppend,
    ] {
        assert_dispatch_stop_race(race).await;
    }
}

async fn assert_dispatch_stop_race(race: DispatchStopRace) {
    let clock = Arc::new(ManualClock::at(100));
    let inner = Arc::new(MemoryLedgerStore::new(clock));
    let store = Arc::new(DispatchRaceStore::new(inner));
    let resource_name = match race {
        DispatchStopRace::BeforeReplay => "dispatch-stop-replay",
        DispatchStopRace::BeforeAppend => "dispatch-stop-append",
    };
    let ledger = Arc::new(
        ClusterLedger::create(
            store.clone(),
            ResourceId::new(resource_name).unwrap(),
            OwnerId::new("owner").unwrap(),
            10_000,
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
            mutation: MutationIdentity::for_value(
                IdempotencyId::new("admit").unwrap(),
                "admit",
                &json!({"graph":"fixture"}),
            )
            .unwrap(),
        })
        .await
        .unwrap();
    let adapters = LedgerAdapters::new(ledger);
    match race {
        DispatchStopRace::BeforeReplay => store.arm_prefix(),
        DispatchStopRace::BeforeAppend => store.arm_dispatch_append(),
    }
    let dispatch = {
        let adapters = adapters.clone();
        tokio::spawn(async move { adapters.acquire_dispatch(TurnId::new("racing-turn")).await })
    };
    match race {
        DispatchStopRace::BeforeReplay => store.wait_for_prefix().await,
        DispatchStopRace::BeforeAppend => store.wait_for_dispatch_append().await,
    }
    let params = StopParams {
        mode: StopMode::Force,
        if_generation: Generation::new(1).unwrap(),
        idempotency_key: IdempotencyKey::new("stop-race").unwrap(),
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
    match race {
        DispatchStopRace::BeforeReplay => store.release_prefix(),
        DispatchStopRace::BeforeAppend => store.release_dispatch_append(),
    }
    assert_eq!(
        dispatch.await.unwrap().unwrap_err(),
        AdmissionStoreError::DispatchDenied {
            current: openengine_cluster_protocol::DispatchState::Stopped,
        }
    );
}
