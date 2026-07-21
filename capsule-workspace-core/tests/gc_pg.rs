//! `gc_pg` — the clock-driven GC, over `LocalBlobStore` + the in-memory `MemRefClock` (fast, no
//! Docker). The genuine-aging / server-side-`clock_timestamp()` MF1 guarantee is proven separately on
//! the real `PgRefClock` in `tests/pg_lineage.rs`; here we prove the collector's ORCHESTRATION: MARK
//! (invariant #2) protects live-HEAD blocks at any age, the grace clock + atomic claim reclaim only
//! aged orphans, and a live HEAD survives a GC sweep running concurrently with publishers.

use capsule_workspace_core::cas::*;
use capsule_workspace_core::daemon::{materialize, publish};
use capsule_workspace_core::gc_pg;
use capsule_workspace_core::manifest::Manifest;
use capsule_workspace_core::refclock::{MemRefClock, RefClock};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}
fn write(p: &Path, b: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b).unwrap();
}
fn prng(seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_add(0x9E3779B97F4A7C15);
    let mut out = Vec::with_capacity(CHUNK);
    while out.len() < CHUNK {
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;
        out.extend_from_slice(&z.to_le_bytes());
        x = x.wrapping_add(0x9E3779B97F4A7C15);
    }
    out.truncate(CHUNK);
    out
}

/// The blocks a manifest references (as `(block, byte_length)`), i.e. the touch set a publisher would
/// hand `RefClock::touch` — heavily duplicated (one entry per chunk), which the clock must tolerate.
fn ref_set(store: &LocalBlobStore, digest: &str) -> Vec<(BlockId, u64)> {
    let m = Manifest::from_bytes(&store.get_manifest(digest).unwrap()).unwrap();
    m.files
        .iter()
        .flat_map(|f| f.chunks.iter())
        .filter_map(|cid| m.chunks.get(cid))
        .map(|loc| (loc.block.clone(), loc.clen as u64))
        .collect()
}
fn n_blocks(store: &Path) -> usize {
    fs::read_dir(store.join("blocks")).unwrap().count()
}

// (1) reclaims aged orphans, keeps live-HEAD blocks (MARK), HEAD still materializes.
#[test]
fn gc_pg_reclaims_orphans_keeps_marked() {
    let d = tmp();
    let tree = d.path().join("t");
    let store_dir = d.path().join("s");
    let s = LocalBlobStore::new(&store_dir).unwrap();
    let clock = MemRefClock::new();

    // gen0 (orphan-to-be)
    let mut b0 = Vec::new();
    for i in 0..8u64 {
        b0.extend(prng(i));
    }
    write(&tree.join("f.bin"), &b0);
    let g0 = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
    clock.touch(&ref_set(&s, &g0.manifest)).unwrap();

    // gen1 (the live HEAD) — fully distinct content
    let mut b1 = Vec::new();
    for i in 100..108u64 {
        b1.extend(prng(i));
    }
    write(&tree.join("f.bin"), &b1);
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
    clock.touch(&ref_set(&s, &g1.manifest)).unwrap();

    let before = n_blocks(&store_dir);
    // grace=0 ⇒ everything is "old"; only the MARK protects g1.
    let st = gc_pg::collect(
        &s,
        &clock,
        std::slice::from_ref(&g1.manifest),
        Duration::ZERO,
    )
    .unwrap();
    let after = n_blocks(&store_dir);
    println!(
        "[gc_pg-1] before={before} after={after} deleted={} kept_marked={} raced={}",
        st.deleted, st.kept_marked, st.raced_young
    );
    assert!(st.deleted >= 1, "gen0 orphan blocks reclaimed");
    assert!(st.kept_marked >= 1, "gen1 (live) blocks protected by MARK");
    assert!(after < before, "store shrank");
    // the live HEAD still materializes byte-identically after GC
    let out = d.path().join("o");
    materialize(&s, &g1.manifest, &out).unwrap();
    assert_eq!(fs::read(out.join("f.bin")).unwrap(), b1, "live HEAD intact");
}

// (2) a MARKED block is never collected even at grace=0 (invariant #2 — the clock alone would take it).
#[test]
fn gc_pg_marked_block_never_collected() {
    let d = tmp();
    let tree = d.path().join("t");
    let store_dir = d.path().join("s");
    let s = LocalBlobStore::new(&store_dir).unwrap();
    let clock = MemRefClock::new();
    write(&tree.join("a.bin"), &prng(7));
    let g = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
    clock.touch(&ref_set(&s, &g.manifest)).unwrap();

    let st = gc_pg::collect(
        &s,
        &clock,
        std::slice::from_ref(&g.manifest),
        Duration::ZERO,
    )
    .unwrap();
    assert_eq!(st.deleted, 0, "no live block deleted even at grace=0");
    assert!(st.kept_marked >= 1);
    materialize(&s, &g.manifest, &d.path().join("o")).unwrap();
}

// (3) the MemRefClock models the MF1 server-side re-check: a candidate re-touched young defeats the
// claim at a large grace (mirrors the PgRefClock atomic_claim test — proves the fake is faithful).
#[test]
fn mem_refclock_atomic_claim_recheck() {
    let clock = MemRefClock::new();
    let b: BlockId = "recheckBlock".into();
    clock.touch(&[(b.clone(), 10)]).unwrap();
    // candidate at grace 0...
    assert!(
        clock
            .candidates_older_than(Duration::ZERO)
            .unwrap()
            .contains(&b)
    );
    // ...but a claim at 1h grace refuses it (young), and a racing re-touch keeps it young.
    assert!(
        !clock
            .claim_collectable(&b, Duration::from_secs(3600))
            .unwrap()
    );
    clock.touch(&[(b.clone(), 10)]).unwrap(); // racing publish
    assert!(
        !clock
            .claim_collectable(&b, Duration::from_secs(3600))
            .unwrap()
    );
    // it still exists (never claimed)
    assert!(
        clock
            .candidates_older_than(Duration::ZERO)
            .unwrap()
            .contains(&b)
    );
    // grace 0 finally claims it once; a second claim is a no-op
    assert!(clock.claim_collectable(&b, Duration::ZERO).unwrap());
    assert!(!clock.claim_collectable(&b, Duration::ZERO).unwrap());
}

// (4) CONCURRENCY: a GC thread sweeps continuously while a publisher advances 20 generations. Uses a
// REALISTIC grace (50 ms) with inter-gen spacing (20 ms) — honoring the operational invariant
// `grace > publish duration`: a freshly-touched block stays young (protected by the clock) until the
// next sweep marks it, while superseded gens age past grace and ARE reclaimed. The live HEAD must
// never lose a block. (grace=0 would be an INVALID config: a freshly-written block a stale GC read
// hasn't marked yet has no youth window — that's a misconfiguration, not a GC defect.)
#[test]
fn gc_pg_concurrent_publish_and_gc_no_live_loss() {
    let d = tmp();
    let store_dir = d.path().join("s");
    let s = Arc::new(LocalBlobStore::new(&store_dir).unwrap());
    let clock = Arc::new(MemRefClock::new());
    let head: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let gens = 20u64;
    let grace = Duration::from_millis(50);
    let stop = Arc::new(AtomicUsize::new(0));

    // publisher: put blocks + touch (BEFORE exposing as live) + publish manifest, THEN advance HEAD;
    // space generations so superseded blocks age past grace during the run.
    let pub_h = {
        let (s, clock, head, stop) = (s.clone(), clock.clone(), head.clone(), stop.clone());
        let tree = d.path().join("t");
        std::thread::spawn(move || {
            for k in 0..gens {
                let mut body = Vec::new();
                for i in 0..6u64 {
                    body.extend(prng(k * 1000 + i));
                }
                write(&tree.join("f.bin"), &body);
                let g = publish(&tree, &*s, &ChunkIndex::new(), None).unwrap();
                clock.touch(&ref_set(&s, &g.manifest)).unwrap();
                *head.lock().unwrap() = Some(g.manifest.clone());
                std::thread::sleep(Duration::from_millis(20));
            }
            stop.store(1, Ordering::SeqCst);
        })
    };
    // collector: sweep the current live HEAD; after each sweep assert every block of the swept head
    // still exists (catches a live-byte loss at the exact sweep, not just at the end).
    let gc_h = {
        let (s, clock, head, stop) = (s.clone(), clock.clone(), head.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut sweeps = 0usize;
            let mut deleted = 0usize;
            while stop.load(Ordering::SeqCst) == 0 {
                let Some(h) = head.lock().unwrap().clone() else {
                    continue;
                };
                let st = gc_pg::collect(&*s, &*clock, std::slice::from_ref(&h), grace).unwrap();
                deleted += st.deleted;
                sweeps += 1;
                // every block the swept live HEAD references must still be present.
                let m = Manifest::from_bytes(&s.get_manifest(&h).unwrap()).unwrap();
                for loc in m.chunks.values() {
                    assert!(
                        s.has_block(&loc.block),
                        "GC collected a block of the live HEAD it swept"
                    );
                }
            }
            (sweeps, deleted)
        })
    };
    pub_h.join().unwrap();
    let (sweeps, deleted) = gc_h.join().unwrap();
    println!("[gc_pg-4] sweeps={sweeps} deleted={deleted} (grace={grace:?})");
    assert!(sweeps > 0, "GC actually ran");
    assert!(deleted > 0, "GC reclaimed superseded orphans under load");
    // final HEAD materializes byte-identically (proof no live block was ever collected).
    let final_head = head.lock().unwrap().clone().unwrap();
    let out = d.path().join("o");
    materialize(&*s, &final_head, &out).unwrap();
    assert!(
        out.join("f.bin").exists(),
        "final live HEAD survived concurrent GC"
    );
}

// (F2) The `live` set MUST be store-wide. Two lineages with distinct blocks share one global store;
// a store-wide sweep protects both, but a per-lineage sweep DELETES the other lineage's live blocks.
// This is the footgun the loud warning on gc_pg::collect guards against.
#[test]
fn gc_store_wide_live_set_protects_all_lineages() {
    let d = tmp();
    let store_dir = d.path().join("s");
    let s = LocalBlobStore::new(&store_dir).unwrap();
    let clock = MemRefClock::new();

    let ta = d.path().join("ta");
    write(&ta.join("a.bin"), &prng(10));
    let ma = publish(&ta, &s, &ChunkIndex::new(), None).unwrap().manifest;
    clock.touch(&ref_set(&s, &ma)).unwrap();

    let tb = d.path().join("tb");
    write(&tb.join("b.bin"), &prng(20));
    let mb = publish(&tb, &s, &ChunkIndex::new(), None).unwrap().manifest;
    clock.touch(&ref_set(&s, &mb)).unwrap();

    // STORE-WIDE [A, B] at grace 0 → both marked, nothing deleted.
    let both = vec![ma.clone(), mb.clone()];
    let st = gc_pg::collect(&s, &clock, &both, Duration::ZERO).unwrap();
    assert_eq!(st.deleted, 0, "store-wide live set protects every lineage");
    assert!(st.kept_marked >= 2);

    // FOOTGUN: sweeping with only lineage A's HEAD collects lineage B's (unmarked) live blocks.
    let st2 = gc_pg::collect(&s, &clock, std::slice::from_ref(&ma), Duration::ZERO).unwrap();
    assert!(
        st2.deleted >= 1,
        "a per-lineage sweep collected ANOTHER lineage's blocks (the F2 footgun)"
    );
    assert!(
        materialize(&s, &mb, &d.path().join("ob")).is_err(),
        "lineage B lost its blocks to the per-lineage sweep"
    );
    // lineage A (the one that was in the live set) is intact.
    materialize(&s, &ma, &d.path().join("oa")).unwrap();
}
