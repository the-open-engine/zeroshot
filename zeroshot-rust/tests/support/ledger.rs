use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use zeroshot_engine::cluster_ledger::record::{
    CanonicalDigest, RecordPayload, StoredRecord, MAX_RECORD_PAYLOAD_BYTES,
};
use zeroshot_engine::cluster_ledger::store::{
    AppendBatch, AppendGuard, IdempotencyId, LedgerStore, MutationReceipt, OwnerId, Position,
    ResourceId,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

pub fn temp_root(label: &str) -> PathBuf {
    let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "zeroshot-rust-{label}-{}-{sequence}",
        std::process::id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).expect("stale test root must be removable");
    }
    std::fs::create_dir_all(&path).expect("test root must be creatable");
    path
}

pub fn resource(value: &str) -> ResourceId {
    ResourceId::new(value).expect("test resource must be valid")
}

pub fn owner(value: &str) -> OwnerId {
    OwnerId::new(value).expect("test owner must be valid")
}

pub fn key(value: &str) -> IdempotencyId {
    IdempotencyId::new(value).expect("test idempotency key must be valid")
}

pub fn cleanup_payload(label: &[u8]) -> RecordPayload {
    RecordPayload::CleanupReceipt {
        cleanup_digest: CanonicalDigest::of(label),
    }
}

pub fn one_record_batch(
    resource: &ResourceId,
    sequence: u64,
    previous_hash: [u8; 32],
    payload: RecordPayload,
    receipt_key: &str,
) -> AppendBatch {
    let sequence = Position::new(sequence).expect("test position must be valid");
    let record = StoredRecord::build(resource.clone(), sequence, &payload, previous_hash)
        .expect("test record must build");
    let receipt = MutationReceipt {
        idempotency_key: key(receipt_key),
        method: "contract".into(),
        fingerprint: CanonicalDigest::of(receipt_key.as_bytes()).as_bytes(),
        response: b"{}".to_vec(),
        committed_position: sequence,
    };
    AppendBatch::new(vec![record], Some(receipt)).expect("test batch must be valid")
}

pub async fn run_store_contract(store: Arc<dyn LedgerStore>, resource_id: &str) {
    let invalid_resource = resource(&format!("{resource_id}-invalid-create"));
    let resource = resource(resource_id);
    let owner = owner("contract-owner");
    assert!(
        store
            .create_fenced(&invalid_resource, &owner, 0)
            .await
            .is_err()
    );
    assert!(store.open(&invalid_resource).await.is_err());
    assert_eq!(
        store.create(&resource).await.unwrap().position,
        Position::ZERO
    );
    assert!(store.create(&resource).await.is_err());
    assert_eq!(
        store.open(&resource).await.unwrap().position,
        Position::ZERO
    );

    let page = store.discover(None, 1).await.unwrap();
    assert_eq!(page.resources[0].resource, resource);
    assert!(store.discover(None, 1_025).await.is_err());

    let fence = store.acquire_fence(&resource, &owner, 100).await.unwrap();
    store.check_fence(&fence).await.unwrap();
    let renewed = store.renew_fence(&fence, 200).await.unwrap();
    assert_eq!(renewed.epoch, fence.epoch);

    let cancelled_batch = one_record_batch(
        &resource,
        1,
        [0; 32],
        cleanup_payload(b"cancelled"),
        "contract-cancelled",
    );
    assert!(matches!(
        store
            .compare_and_append_guarded(
                &resource,
                &renewed,
                Position::ZERO,
                cancelled_batch,
                AppendGuard::cancelled_when(|| true),
            )
            .await,
        Err(zeroshot_engine::cluster_ledger::store::StoreError::AppendCancelled)
    ));
    assert_eq!(
        store.open(&resource).await.unwrap().position,
        Position::ZERO
    );

    let zero_position_receipt = MutationReceipt {
        idempotency_key: key("contract-zero-position"),
        method: "contract".into(),
        fingerprint: [0; 32],
        response: b"{}".to_vec(),
        committed_position: Position::ZERO,
    };
    let invalid_zero_batch = AppendBatch {
        records: Vec::new(),
        receipt: Some(zero_position_receipt),
    };
    assert!(invalid_zero_batch.validate().is_err());
    assert!(
        store
            .compare_and_append(&resource, &renewed, Position::ZERO, invalid_zero_batch,)
            .await
            .is_err()
    );
    assert_eq!(
        store.open(&resource).await.unwrap().position,
        Position::ZERO
    );

    let batch = one_record_batch(
        &resource,
        1,
        [0; 32],
        cleanup_payload(b"contract"),
        "contract-1",
    );
    let committed = store
        .compare_and_append(&resource, &renewed, Position::ZERO, batch.clone())
        .await
        .unwrap();
    assert!(matches!(
        committed,
        zeroshot_engine::cluster_ledger::store::AppendOutcome::Committed(_)
    ));
    let replayed = store
        .compare_and_append_guarded(
            &resource,
            &renewed,
            Position::ZERO,
            batch,
            AppendGuard::cancelled_when(|| true),
        )
        .await
        .unwrap();
    assert!(matches!(
        replayed,
        zeroshot_engine::cluster_ledger::store::AppendOutcome::Replayed(_)
    ));

    let snapshot = store.read_prefix(&resource, None).await.unwrap();
    assert_eq!(snapshot.position.get(), 1);
    assert_eq!(snapshot.records.len(), 1);
    assert_eq!(snapshot.receipts.len(), 1);
    assert_eq!(
        store
            .read_range(&resource, Position::ZERO, 4_096)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .read_range(&resource, Position::ZERO, 4_097)
            .await
            .is_err()
    );

    let waiter_store = Arc::clone(&store);
    let waiter_resource = resource.clone();
    let waiter = tokio::spawn(async move {
        waiter_store
            .wait_for_advance(&waiter_resource, Position::new(1).unwrap(), 5_000)
            .await
            .unwrap()
    });
    let second = one_record_batch(
        &resource,
        2,
        snapshot.records[0].record_hash,
        cleanup_payload(b"advance"),
        "contract-2",
    );
    store
        .compare_and_append(&resource, &renewed, Position::new(1).unwrap(), second)
        .await
        .unwrap();
    assert_eq!(waiter.await.unwrap().get(), 2);
    assert_eq!(
        store
            .wait_for_advance(&resource, Position::new(2).unwrap(), 0)
            .await
            .unwrap()
            .get(),
        2
    );
    let receipt_only = MutationReceipt {
        idempotency_key: key("contract-receipt-only"),
        method: "contract".into(),
        fingerprint: [7; 32],
        response: b"{}".to_vec(),
        committed_position: Position::new(2).unwrap(),
    };
    store
        .compare_and_append(
            &resource,
            &renewed,
            Position::new(2).unwrap(),
            AppendBatch::new(Vec::new(), Some(receipt_only)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(store.open(&resource).await.unwrap().position.get(), 2);
    let latest = store.read_prefix(&resource, None).await.unwrap();
    let mut oversized_record = latest.records[1].clone();
    oversized_record.sequence = Position::new(3).unwrap();
    oversized_record.payload = vec![0; MAX_RECORD_PAYLOAD_BYTES + 1];
    let oversized_batch = AppendBatch {
        records: vec![oversized_record],
        receipt: Some(MutationReceipt {
            idempotency_key: key("contract-oversized"),
            method: "contract".into(),
            fingerprint: [8; 32],
            response: b"{}".to_vec(),
            committed_position: Position::new(3).unwrap(),
        }),
    };
    assert!(
        store
            .compare_and_append(
                &resource,
                &renewed,
                Position::new(2).unwrap(),
                oversized_batch,
            )
            .await
            .is_err()
    );
    assert_eq!(store.open(&resource).await.unwrap().position.get(), 2);
    let mut reused = one_record_batch(
        &resource,
        3,
        latest.records[1].record_hash,
        cleanup_payload(b"reused"),
        "contract-1",
    );
    reused.receipt.as_mut().unwrap().fingerprint = [9; 32];
    assert!(
        store
            .compare_and_append(&resource, &renewed, Position::new(2).unwrap(), reused)
            .await
            .is_err()
    );
    assert_eq!(store.open(&resource).await.unwrap().position.get(), 2);

    let conflicting = one_record_batch(
        &resource,
        3,
        latest.records[1].record_hash,
        cleanup_payload(b"conflict"),
        "contract-conflict",
    );
    assert!(
        store
            .compare_and_append(&resource, &renewed, Position::ZERO, conflicting)
            .await
            .is_err()
    );
    assert_eq!(store.open(&resource).await.unwrap().position.get(), 2);

    store
        .remove(&resource, &renewed, Position::new(2).unwrap())
        .await
        .unwrap();
    assert!(store.open(&resource).await.is_err());
    assert!(
        store
            .lookup_receipt(&resource, &key("contract-1"))
            .await
            .is_err()
    );
}
