use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use zeroshot_engine::cluster_ledger::mutations::AdmissionRequest;
use zeroshot_engine::cluster_ledger::record::CanonicalDigest;
use zeroshot_engine::cluster_ledger::store::{IdempotencyId, OwnerId, ResourceId};

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

pub fn admission_request(
    graph: Vec<u8>,
    input: Vec<u8>,
    compiled_ir: Vec<u8>,
    absolute_deadline_ms: u64,
) -> AdmissionRequest {
    AdmissionRequest {
        graph_digest: CanonicalDigest::of(&graph),
        input_digest: CanonicalDigest::of(&input),
        policy_digest: CanonicalDigest::of(b"policy"),
        catalog_digest: CanonicalDigest::of(b"catalog"),
        profile_digest: CanonicalDigest::of(b"profile"),
        absolute_deadline_ms,
        verified_input: input,
        canonical_graph: graph,
        canonical_compiled_ir: compiled_ir,
    }
}
