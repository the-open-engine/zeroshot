//! Phase 5 — the FULL production stack against REAL AWS: S3 blobs (`S3BlobStore`) + RDS Postgres
//! lineage & GC clock (`PgLineageStore`/`PgRefClock`) together. This is the drop-in the prototype was
//! built toward, exercised end-to-end on the services it targets.
//!
//! Gated (all required, else a printed skip): `S3_IT=1` + `S3_BUCKET` + AWS creds (real S3, no
//! `S3_ENDPOINT_URL`) + `DATABASE_URL` (the RDS instance, `sslmode=require`).
//!   S3_IT=1 S3_BUCKET=… AWS_REGION=us-east-1 DATABASE_URL='postgres://…@rds…:5432/capsule?sslmode=require' \
//!     cargo test --features pg,s3 --release --test e2e_aws -- --nocapture
#![cfg(all(feature = "pg", feature = "s3"))]

use capsule_workspace_core::cas::{BlobStore, BlockId, ChunkIndex};
use capsule_workspace_core::daemon::{materialize, publish};
use capsule_workspace_core::gc_pg;
use capsule_workspace_core::ifaces::{Fence, LineageId};
use capsule_workspace_core::lineage::LineageStore;
use capsule_workspace_core::manifest::Manifest;
use capsule_workspace_core::pg::PgLineageStore;
use capsule_workspace_core::refclock::RefClock;
use capsule_workspace_core::s3::S3BlobStore;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

fn enabled() -> bool {
    let it = std::env::var("S3_IT").ok().as_deref() == Some("1");
    let bucket = std::env::var("S3_BUCKET").is_ok();
    let db = std::env::var("DATABASE_URL").is_ok();
    if !(it && bucket && db) {
        eprintln!(
            "SKIPPED e2e_aws: needs S3_IT=1 + S3_BUCKET + AWS creds + DATABASE_URL (real S3 + RDS)."
        );
        return false;
    }
    true
}

fn write(p: &Path, b: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b).unwrap();
}
fn prng(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_add(0x9E3779B97F4A7C15);
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;
        out.extend_from_slice(&z.to_le_bytes());
        x = x.wrapping_add(0x9E3779B97F4A7C15);
    }
    out.truncate(len);
    out
}
/// One publish cycle (daemon MF3+MF4 ordering) against the real stack. Returns the manifest digest.
fn cycle(
    tree: &Path,
    store: &dyn BlobStore,
    ls: &PgLineageStore,
    clock: &dyn RefClock,
    lin: &LineageId,
) -> String {
    let head = ls.head(lin).unwrap();
    let (known, parent, expected): (ChunkIndex, Option<String>, Fence) = match &head {
        Some(h) => {
            let m = Manifest::from_bytes(&store.get_manifest(&h.manifest_digest).unwrap()).unwrap();
            (m.chunks, Some(h.manifest_digest.clone()), h.fence)
        }
        None => (ChunkIndex::new(), None, Fence(0)),
    };
    let st = publish(tree, store, &known, parent).unwrap();
    let m = Manifest::from_bytes(&store.get_manifest(&st.manifest).unwrap()).unwrap();
    let mut sizes: BTreeMap<BlockId, u64> = BTreeMap::new();
    for loc in m.chunks.values() {
        *sizes.entry(loc.block.clone()).or_insert(0) += loc.clen as u64;
    }
    clock.touch(&sizes.into_iter().collect::<Vec<_>>()).unwrap();
    ls.advance(lin, st.manifest.clone(), expected).unwrap();
    st.manifest
}

// Full round-trip: S3 publish → materialize (byte-identical) → GC reclaims a real superseded S3 orphan
// → live HEAD still materializes. Plus a cold-materialize wall-time (R2, laptop→S3 = an UPPER bound;
// the in-region prototype number is lower).
#[test]
fn e2e_publish_materialize_gc_over_s3_and_rds() {
    if !enabled() {
        return;
    }
    let store = S3BlobStore::from_env().expect("S3BlobStore");
    let ls = PgLineageStore::connect(&std::env::var("DATABASE_URL").unwrap()).expect("RDS connect");
    ls.init_schema().expect("schema");
    let clock = ls.ref_clock();
    // unique lineage per run
    let lin = LineageId(format!("e2e-{:x}", std::process::id()));

    let d = tempfile::tempdir().unwrap();
    let tree = d.path().join("t");

    // gen1: one file, a few MB → a block in S3.
    write(&tree.join("f.bin"), &prng(1, 5_000_000));
    let g1 = cycle(&tree, &store, &ls, &clock, &lin);
    assert_eq!(ls.head(&lin).unwrap().unwrap().fence, Fence(1));
    let g1blocks: Vec<BlockId> = Manifest::from_bytes(&store.get_manifest(&g1).unwrap())
        .unwrap()
        .chunks
        .values()
        .map(|l| l.block.clone())
        .collect();

    // cold materialize gen1 from REAL S3 → byte-identical + timed (R2 sanity, laptop→S3 upper bound).
    let out1 = d.path().join("o1");
    let t = Instant::now();
    let ms = materialize(&store, &g1, &out1).unwrap();
    let secs = t.elapsed().as_secs_f64();
    println!(
        "[e2e] cold-materialize gen1 from S3: {:.2}s, {} files, {} blocks fetched (laptop→S3 = upper bound vs 61s budget)",
        secs, ms.files, ms.blocks_fetched
    );
    assert_eq!(fs::read(out1.join("f.bin")).unwrap(), prng(1, 5_000_000));
    assert!(
        secs < 61.0,
        "cold-materialize must fit the activation budget"
    );

    // gen2: FULLY distinct content → gen1's block references nothing gen2 needs → gen1 is a real
    // orphan (a partial-overwrite would dedup-reuse gen1's block and correctly keep it live).
    write(&tree.join("f.bin"), &prng(99, 5_000_000));
    let g2 = cycle(&tree, &store, &ls, &clock, &lin);
    assert_eq!(ls.head(&lin).unwrap().unwrap().fence, Fence(2));
    let g2blocks: Vec<BlockId> = Manifest::from_bytes(&store.get_manifest(&g2).unwrap())
        .unwrap()
        .chunks
        .values()
        .map(|l| l.block.clone())
        .collect();

    // GC with live=[gen2], grace=0 → gen1's orphan block is DELETED from real S3; gen2 kept.
    let st = gc_pg::collect(&store, &clock, std::slice::from_ref(&g2), Duration::ZERO).unwrap();
    println!(
        "[e2e] gc over S3+RDS: scanned={} deleted={} kept_marked={} raced={} missing={}",
        st.scanned, st.deleted, st.kept_marked, st.raced_young, st.missing_live_manifests
    );
    assert!(st.deleted >= 1, "a real superseded S3 orphan was reclaimed");
    // gen1's orphan blocks are actually GONE from S3; gen2's are all present (marked → never claimed).
    for b in &g1blocks {
        if !g2blocks.contains(b) {
            assert!(
                !store.has_block(b),
                "orphan gen1 block really deleted from S3"
            );
        }
    }
    for b in &g2blocks {
        assert!(store.has_block(b), "live HEAD block survived GC on real S3");
    }
    // gen2 still materializes byte-identically from S3 after GC
    let out2 = d.path().join("o2");
    materialize(&store, &g2, &out2).unwrap();
    assert_eq!(fs::read(out2.join("f.bin")).unwrap(), prng(99, 5_000_000));

    // cleanup this run's remaining S3 objects (best-effort; the bucket is torn down anyway)
    for b in g1blocks.iter().chain(g2blocks.iter()) {
        let _ = store.delete_block(b);
    }
    let _ = store.delete_manifest(&g1);
    let _ = store.delete_manifest(&g2);
}

// Fence CAS over real RDS: two concurrent first-writers → exactly one wins, one StaleFence.
#[test]
fn e2e_fence_cas_over_rds() {
    if !enabled() {
        return;
    }
    use capsule_workspace_core::lineage::LineageError;
    use std::sync::{Arc, Barrier};
    let ls = Arc::new(
        PgLineageStore::connect(&std::env::var("DATABASE_URL").unwrap()).expect("RDS connect"),
    );
    ls.init_schema().unwrap();
    let lin = LineageId(format!("e2e-fence-{:x}", std::process::id()));
    let barrier = Arc::new(Barrier::new(2));
    let mut hs = Vec::new();
    for i in 0..2u32 {
        let (ls, lin, barrier) = (ls.clone(), lin.clone(), barrier.clone());
        hs.push(std::thread::spawn(move || {
            barrier.wait();
            ls.advance(&lin, format!("d{i}"), Fence(0))
        }));
    }
    let r: Vec<_> = hs.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(r.iter().filter(|x| x.is_ok()).count(), 1, "one writer wins");
    let err = r.into_iter().find_map(|x| x.err()).unwrap();
    assert!(
        matches!(
            err.downcast_ref::<LineageError>(),
            Some(LineageError::StaleFence { .. })
        ),
        "loser is a typed StaleFence over real RDS"
    );
}
