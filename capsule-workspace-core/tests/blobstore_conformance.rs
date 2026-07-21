//! Trait-conformance suite for `BlobStore`: the SAME assertions run against `LocalBlobStore`
//! (always) and `S3BlobStore` (when an S3 endpoint is configured). Proves the S3 adapter is a true
//! drop-in for the local store on the contract the pipeline depends on.
//!
//! S3 target selection:
//! - `S3_ENDPOINT_URL` + `S3_BUCKET` set  → MinIO/localstack (local, no AWS).
//! - `S3_IT=1` (+ `S3_BUCKET`, AWS creds) → real AWS S3 (internal account).
//! - neither set → the S3 test PRINTS a skip notice and passes (degraded coverage is reported, not
//!   silent — repo rule).

use capsule_workspace_core::cas::{BlobStore, LocalBlobStore, StoreError};

/// The full contract, exercised against any `BlobStore`. `k` is a per-store-unique key prefix so a
/// shared real bucket doesn't collide across runs.
fn conformance(store: &dyn BlobStore, k: &str) {
    let block_id = format!("{k}1111111111111111111111111111111111111111111111111111111111111111");
    let manifest_id =
        format!("{k}2222222222222222222222222222222222222222222222222222222222222222");
    let absent = format!("{k}deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef");
    let block = vec![0xABu8; 300_000]; // a real multi-hundred-KB body (single PUT; 64 MiB blocks / multipart / large-object timeouts are Phase 5)
    let manifest = br#"{"schema":1,"files":[]}"#.to_vec();

    // absent before write
    assert!(!store.has_block(&block_id), "block absent before put");
    // get on absent → typed NotFound (distinct from a transient error)
    let e = store.get_block(&absent).expect_err("get absent must error");
    assert!(
        matches!(
            e.downcast_ref::<StoreError>(),
            Some(StoreError::NotFound(_))
        ),
        "get absent must be StoreError::NotFound, got: {e:?}"
    );

    // put + get round-trip (block + manifest), byte-identical
    store.put_block(&block_id, &block).unwrap();
    store.put_manifest(&manifest_id, &manifest).unwrap();
    assert!(store.has_block(&block_id), "block present after put");
    assert_eq!(
        store.get_block(&block_id).unwrap(),
        block,
        "block round-trip"
    );
    assert_eq!(
        store.get_manifest(&manifest_id).unwrap(),
        manifest,
        "manifest round-trip"
    );

    // unconditional idempotent overwrite with IDENTICAL bytes (content-addressed ⇒ always identical)
    store.put_block(&block_id, &block).unwrap();
    assert_eq!(
        store.get_block(&block_id).unwrap(),
        block,
        "overwrite stays byte-identical"
    );

    // idempotent delete: true when present, false when already gone
    assert!(
        store.delete_block(&block_id).unwrap(),
        "first delete removed it"
    );
    assert!(
        !store.delete_block(&block_id).unwrap(),
        "second delete is a no-op (false)"
    );
    assert!(!store.has_block(&block_id), "block absent after delete");
    // get after delete → NotFound again
    assert!(
        matches!(
            store
                .get_block(&block_id)
                .expect_err("get after delete errors")
                .downcast_ref::<StoreError>(),
            Some(StoreError::NotFound(_))
        ),
        "get after delete is NotFound"
    );
    // manifest delete idempotency
    assert!(store.delete_manifest(&manifest_id).unwrap());
    assert!(!store.delete_manifest(&manifest_id).unwrap());
}

#[test]
fn local_blobstore_conformance() {
    let d = tempfile::tempdir().unwrap();
    let store = LocalBlobStore::new(d.path().join("store")).unwrap();
    conformance(&store, "");
}

#[cfg(feature = "s3")]
#[test]
fn s3_blobstore_conformance() {
    use capsule_workspace_core::s3::S3BlobStore;
    let endpoint = std::env::var("S3_ENDPOINT_URL")
        .ok()
        .filter(|s| !s.is_empty());
    let it = std::env::var("S3_IT").ok().as_deref() == Some("1");
    let have_bucket = std::env::var("S3_BUCKET").is_ok();
    if !have_bucket || (endpoint.is_none() && !it) {
        eprintln!(
            "SKIPPED s3_blobstore_conformance: set S3_ENDPOINT_URL+S3_BUCKET (MinIO) or \
             S3_IT=1 + S3_BUCKET + AWS creds (real S3). Degraded coverage."
        );
        return;
    }
    let store = S3BlobStore::from_env().expect("build S3BlobStore from env");
    // unique prefix per run so a shared real bucket doesn't collide (hex-only to stay a valid key)
    let run = format!("{:x}", std::process::id());
    conformance(&store, &run);
}

/// MF2 concurrency proof: many threads share ONE `S3BlobStore` and call `get_block` concurrently —
/// each does `block_on` on the adapter's single multi-thread runtime. This is the exact path
/// `materialize`'s `par_iter` fan-out takes (concurrent `block_on` from rayon threads), and the whole
/// reason the adapter runtime must be MULTI-thread (a current-thread rt would panic/serialize). Proves
/// no nested-runtime panic and correct results under concurrency. Same S3 gating as the conformance test.
#[cfg(feature = "s3")]
#[test]
fn s3_concurrent_block_on() {
    use capsule_workspace_core::s3::S3BlobStore;
    use std::sync::{Arc, Barrier};
    let endpoint = std::env::var("S3_ENDPOINT_URL")
        .ok()
        .filter(|s| !s.is_empty());
    let it = std::env::var("S3_IT").ok().as_deref() == Some("1");
    if std::env::var("S3_BUCKET").is_err() || (endpoint.is_none() && !it) {
        eprintln!(
            "SKIPPED s3_concurrent_block_on: set S3_ENDPOINT_URL+S3_BUCKET or S3_IT=1. Degraded coverage."
        );
        return;
    }
    let store = Arc::new(S3BlobStore::from_env().expect("build S3BlobStore"));
    let run = format!("{:x}c", std::process::id());
    // seed 8 distinct blocks
    let n = 8usize;
    let blocks: Vec<(String, Vec<u8>)> = (0..n)
        .map(|i| (format!("{run}{i:063x}"), vec![i as u8; 200_000 + i * 1000]))
        .collect();
    for (k, v) in &blocks {
        store.put_block(k, v).unwrap();
    }
    // N threads all fire get_block at once (barrier) → concurrent block_on on the shared runtime.
    let barrier = Arc::new(Barrier::new(n));
    let mut handles = Vec::new();
    for (k, v) in blocks.clone() {
        let (store, barrier) = (Arc::clone(&store), Arc::clone(&barrier));
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            let got = store.get_block(&k).expect("concurrent get_block");
            assert_eq!(got, v, "concurrent get_block returned wrong bytes");
        }));
    }
    for h in handles {
        h.join().expect("no panic under concurrent block_on");
    }
    for (k, _) in &blocks {
        store.delete_block(k).unwrap();
    }
}
