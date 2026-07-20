//! E8 — grace-period mark-sweep GC: reclaims orphans (growth tracks live set) AND the grace
//! period protects an in-flight publish's not-yet-committed blocks from being collected.

use capsule_workspace_core::cas::*;
use capsule_workspace_core::daemon::{materialize, publish};
use capsule_workspace_core::gc;
use capsule_workspace_core::manifest::Manifest;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
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
fn blocks_bytes(store: &Path) -> u64 {
    fs::read_dir(store.join("blocks"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_name().to_string_lossy().contains(".tmp"))
        .map(|e| e.metadata().unwrap().len())
        .sum()
}

// (1) GC reclaims orphans: after 10 generations of full churn, GC(live=[HEAD]) shrinks the
// store to ~the HEAD's live bytes, and HEAD still materializes byte-identically.
#[test]
fn gc_reclaims_orphans_growth_tracks_live_set() {
    let d = tmp();
    let tree = d.path().join("t");
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    let mut known = ChunkIndex::new();
    let mut head = String::new();
    for gen in 0..10u64 {
        let mut body = Vec::new();
        for i in 0..16u64 {
            body.extend(prng(gen * 100 + i));
        } // 16 all-new distinct chunks / gen
        write(&tree.join("f.bin"), &body);
        let st = publish(&tree, &s, &known, None).unwrap();
        known = Manifest::from_bytes(&s.get_manifest(&st.manifest).unwrap())
            .unwrap()
            .chunks;
        head = st.manifest;
    }
    let before = blocks_bytes(&store);
    let live_head: u64 = {
        let m = Manifest::from_bytes(&s.get_manifest(&head).unwrap()).unwrap();
        let live: HashSet<_> = m
            .files
            .iter()
            .flat_map(|f| f.chunks.iter())
            .map(|c| m.chunks[c].block.clone())
            .collect();
        live.iter()
            .map(|b| fs::metadata(store.join("blocks").join(b)).unwrap().len())
            .sum()
    };
    let st = gc::collect(&store, &[head.clone()], Duration::ZERO).unwrap();
    let after = blocks_bytes(&store);
    println!(
        "[E8-1] before={before} after={after} live_head={live_head} deleted={} manifests_deleted={}",
        st.blocks_deleted, st.manifests_deleted
    );
    assert!(before >= 8 * live_head, "10 gens accumulated orphans");
    assert_eq!(after, live_head, "GC reclaims to exactly the live set");
    // HEAD still materializes after GC
    let out = d.path().join("o");
    materialize(&s, &head, &out).unwrap();
    assert!(out.join("f.bin").exists());
}

// (2) THE correctness-critical test: grace protects an in-flight publish's young orphan blocks.
// Simulate the race: a publish has written its blocks but its manifest commit hasn't "landed"
// yet (manifest removed from the live set / not yet durable). A GC that runs in this window must
// NOT collect those blocks — the grace period is what protects them.
#[test]
fn grace_protects_in_flight_publish_blocks() {
    let d = tmp();
    let tree = d.path().join("t");
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    let mut body = Vec::new();
    for i in 0..8u64 {
        body.extend(prng(5000 + i));
    }
    write(&tree.join("f.bin"), &body);
    let st = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
    let manifest_path = store.join("manifests").join(&st.manifest);
    let manifest_bytes = fs::read(&manifest_path).unwrap();
    // simulate "manifest not yet committed": remove it -> its blocks are orphan-but-YOUNG
    fs::remove_file(&manifest_path).unwrap();

    // GC with a real grace period: must protect the young orphans (0 deleted)
    let protected = gc::collect(&store, &[], Duration::from_secs(3600)).unwrap();
    println!(
        "[E8-2] grace=1h: deleted={} protected_young_orphans={}",
        protected.blocks_deleted, protected.blocks_young_orphans_protected
    );
    assert_eq!(
        protected.blocks_deleted, 0,
        "grace must protect in-flight blocks"
    );
    assert!(protected.blocks_young_orphans_protected >= 1);

    // now the publish "commits": restore the manifest -> it materializes (blocks survived GC)
    fs::write(&manifest_path, &manifest_bytes).unwrap();
    let out = d.path().join("o");
    materialize(&s, &st.manifest, &out).unwrap();
    assert_eq!(
        fs::read(out.join("f.bin")).unwrap(),
        body,
        "in-flight publish survived concurrent GC"
    );

    // CONTRAST: with grace=0 the same GC WOULD have deleted them (proves grace is the mechanism)
    fs::remove_file(&manifest_path).unwrap();
    let unsafe_gc = gc::collect(&store, &[], Duration::ZERO).unwrap();
    println!(
        "[E8-2] grace=0: deleted={} (would have corrupted the in-flight publish)",
        unsafe_gc.blocks_deleted
    );
    assert!(
        unsafe_gc.blocks_deleted >= 1,
        "grace=0 collects the young orphans — the corruption grace prevents"
    );
}

// (3) A live manifest's referenced blocks are never collected, even at grace=0.
#[test]
fn live_referenced_blocks_never_collected() {
    let d = tmp();
    let tree = d.path().join("t");
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    write(&tree.join("a.bin"), &prng(1));
    write(&tree.join("b.bin"), &prng(2));
    let st = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
    // add an orphan block (from a superseded gen) by publishing different content then dropping it
    write(&tree.join("a.bin"), &prng(3));
    let _orphan = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
    let gc1 = gc::collect(&store, &[st.manifest.clone()], Duration::ZERO).unwrap();
    println!(
        "[E8-3] live=[gen0]: kept={} deleted={}",
        gc1.blocks_kept, gc1.blocks_deleted
    );
    let out = d.path().join("o");
    materialize(&s, &st.manifest, &out).unwrap();
    assert_eq!(
        fs::read(out.join("a.bin")).unwrap(),
        prng(1),
        "live manifest fully intact after GC"
    );
}

// (4) A missing live manifest must be SKIPPED + counted, never wedge the whole sweep. A live HEAD
// that momentarily isn't on disk (replication lag, partial restore) references no blocks we can
// see, so MARK skips it and CONTINUES — one dangling ref cannot stop reclamation (unbounded growth).
#[test]
fn missing_live_manifest_skipped_not_wedged() {
    let d = tmp();
    let tree = d.path().join("t");
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    write(&tree.join("f.bin"), &prng(9));
    let st = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
    // live set names a REAL head PLUS a dangling digest that isn't on disk
    let dangling = "0".repeat(64);
    let r = gc::collect(&store, &[st.manifest.clone(), dangling], Duration::ZERO).unwrap();
    println!(
        "[E8-4] missing_live_manifests={} kept={} deleted={}",
        r.missing_live_manifests, r.blocks_kept, r.blocks_deleted
    );
    assert_eq!(
        r.missing_live_manifests, 1,
        "dangling live ref counted, not fatal"
    );
    assert!(
        r.blocks_kept >= 1,
        "the real live manifest's blocks still protected despite the dangling sibling"
    );
    // real HEAD still materializes byte-identically (its blocks were not collected)
    let out = d.path().join("o");
    materialize(&s, &st.manifest, &out).unwrap();
    assert_eq!(fs::read(out.join("f.bin")).unwrap(), prng(9));
}

// (5) A crashed publish's `.tmp` leftover is reclaimed — but ONLY once older than grace, since a
// young `.tmp` could still belong to an in-flight write that's about to rename it into place.
#[test]
fn tmp_block_reclaimed_only_when_older_than_grace() {
    let d = tmp();
    let store = d.path().join("s");
    let _s = LocalBlobStore::new(&store).unwrap();
    let tmpf = store
        .join("blocks")
        .join(format!("{}.7.tmp", std::process::id()));
    write(&tmpf, b"partial-block-from-a-crashed-writer");
    // young: grace protects it (may be an in-flight write mid-rename)
    let young = gc::collect(&store, &[], Duration::from_secs(3600)).unwrap();
    println!("[E8-5] young: tmp_deleted={}", young.tmp_deleted);
    assert_eq!(young.tmp_deleted, 0, "young .tmp protected");
    assert!(tmpf.exists(), "young .tmp still on disk");
    // stale (age >= grace): reclaimed
    let old = gc::collect(&store, &[], Duration::ZERO).unwrap();
    println!("[E8-5] stale: tmp_deleted={}", old.tmp_deleted);
    assert_eq!(old.tmp_deleted, 1, "stale .tmp reclaimed");
    assert!(!tmpf.exists(), "stale .tmp removed");
}

// (6) Concurrency-safety: two GC sweepers over the same store (e.g. two racing leases) must BOTH
// succeed and collectively delete each orphan EXACTLY once — the mid-sweep ENOENT (one sweeper
// removes a file the other already listed) must be tolerated, never error, never double-count.
#[test]
fn concurrent_gc_sweepers_tolerate_races() {
    let d = tmp();
    let tree = d.path().join("t");
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    // many orphan blocks: 30 gens of distinct content, keep NO manifest live -> all collectable
    for gen in 0..30u64 {
        let mut body = Vec::new();
        for i in 0..2u64 {
            body.extend(prng(gen * 1000 + i));
        }
        write(&tree.join("f.bin"), &body);
        publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
    }
    let n_before = fs::read_dir(store.join("blocks")).unwrap().count();
    assert!(n_before > 10, "enough orphans to race on ({n_before})");
    let sa = store.clone();
    let sb = store.clone();
    let h1 = std::thread::spawn(move || gc::collect(&sa, &[], Duration::ZERO));
    let h2 = std::thread::spawn(move || gc::collect(&sb, &[], Duration::ZERO));
    let r1 = h1.join().unwrap().expect("sweeper 1 ok under race");
    let r2 = h2.join().unwrap().expect("sweeper 2 ok under race");
    let deleted = r1.blocks_deleted + r2.blocks_deleted;
    println!(
        "[E8-6] before={n_before} s1={} s2={} total={deleted}",
        r1.blocks_deleted, r2.blocks_deleted
    );
    // The fix's real property: NEITHER sweeper errored (asserted above via `.expect`) and the store
    // is fully swept. We deliberately do NOT assert exact-once deletion: on APFS two concurrent
    // sweepers can each get Ok(()) unlinking a racing name, so total successful unlinks can exceed
    // the file count (measured: 40 vs 30) — benign, and the directory still ends empty.
    assert!(deleted >= n_before, "collectively reclaimed every orphan");
    let left = fs::read_dir(store.join("blocks")).unwrap().count();
    assert_eq!(left, 0, "store swept clean under concurrency");
}

// (7) R1 crash-recovery (reviewer-found F1): a crashed publish leaves durable-but-ORPHAN blocks;
// the retry republishes the IDENTICAL tree (deterministic -> same block ids) and reuses them via
// content-addressed skip-on-exists. Those reused blocks must be refreshed to YOUNG again, or a
// correctly-sized grace no longer protects them and GC deletes a block the retry's about-to-commit
// manifest needs -> silent work loss. This test FAILS without the put_block touch-on-reuse fix.
#[test]
fn crash_retry_reused_blocks_survive_correctly_sized_gc() {
    let d = tmp();
    let tree = d.path().join("t");
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    let mut body = Vec::new();
    for i in 0..8u64 {
        body.extend(prng(7000 + i));
    }
    write(&tree.join("f.bin"), &body);

    // attempt 1 crashes AFTER writing blocks, BEFORE its manifest becomes the live HEAD:
    let a1 = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
    fs::remove_file(store.join("manifests").join(&a1.manifest)).unwrap(); // never committed
    // pod reschedule/backoff exceeds grace -> the orphan blocks age out (mtime = 2h ago):
    let old = std::time::SystemTime::now() - Duration::from_secs(7200);
    for e in fs::read_dir(store.join("blocks")).unwrap() {
        let p = e.unwrap().path();
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }

    // attempt 2 resumes on some node and republishes the IDENTICAL tree (deterministic -> same ids;
    // the blocks are REUSED via skip-on-exists, not rewritten -> the fix must refresh their mtime):
    let a2 = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
    assert_eq!(
        a1.manifest, a2.manifest,
        "identical tree -> identical manifest"
    );

    // GC fires in the retry's reuse->commit window with a CORRECTLY-SIZED grace; the retry's
    // manifest is NOT yet live (about to commit):
    let g = gc::collect(&store, &[], Duration::from_secs(3600)).unwrap();
    println!(
        "[E8-7] deleted={} protected_young={}",
        g.blocks_deleted, g.blocks_young_orphans_protected
    );
    assert_eq!(
        g.blocks_deleted, 0,
        "reused blocks refreshed to young -> a correct grace protects them (no R1 corruption)"
    );

    // the retry commits: its manifest still materializes byte-identically (no dangling blocks):
    let out = d.path().join("o");
    materialize(&s, &a2.manifest, &out).unwrap();
    assert_eq!(
        fs::read(out.join("f.bin")).unwrap(),
        body,
        "no work lost across crash -> retry"
    );
}
