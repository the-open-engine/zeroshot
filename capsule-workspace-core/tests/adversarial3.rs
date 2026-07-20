//! Third-wave adversarial probes (round-2 review of the round-2 code).
//! Targets the gaps the INDEPENDENT AUDIT and the AUTHOR both still missed, focused on the
//! NEW/changed code: publish_pipelined (E6), gc::collect (E8), hardlink walk/materialize (E11).
//! Each test PASSES by asserting the ACTUAL observed behavior and prints evidence under
//! `cargo test --release --test adversarial3 -- --nocapture`. Verdict in comments: BUG / DESIGN-GAP / FINE.

use anyhow::Result;
use capsule_workspace_core::cas::*;
use capsule_workspace_core::daemon::{materialize, publish, publish_pipelined};
use capsule_workspace_core::gc;
use capsule_workspace_core::manifest::Manifest;
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}
fn write(p: &Path, b: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b).unwrap();
}
/// One 256 KiB chunk of distinct, incompressible bytes seeded by `seed`.
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
fn multi(seeds: std::ops::Range<u64>) -> Vec<u8> {
    let mut v = Vec::new();
    for i in seeds {
        v.extend(prng(i));
    }
    v
}
fn load_index(s: &LocalBlobStore, dig: &str) -> ChunkIndex {
    Manifest::from_bytes(&s.get_manifest(dig).unwrap())
        .unwrap()
        .chunks
}
fn backdate(p: &Path) {
    std::process::Command::new("touch")
        .args(["-t", "202001010000"])
        .arg(p)
        .status()
        .unwrap();
}

// ============================================================================
// PROBE 1 — publish_pipelined ZERO-WORK / edge trees the e6 tests never exercised.
// The audit + author e6 tests always have NEW multi-chunk content. Attack 1c/1d: empty tree,
// symlink-only, canonical+hardlinks, 100%-dedup (producer sends NOTHING to any worker), and a
// large file whose interior chunk is shared with a small file (dedup ordering across sizes).
// Verdict target: FINE — pipeline must not hang/panic and must match streaming's logical digest.
// ============================================================================
#[test]
fn probe1_pipeline_zero_work_and_edge_trees() {
    let d = tmp();

    // (a) EMPTY TREE — no files, no symlinks. Producer loop never runs; packer sees no chunks.
    {
        let tree = d.path().join("empty_tree");
        fs::create_dir_all(&tree).unwrap();
        let ss = LocalBlobStore::new(d.path().join("empty_s")).unwrap();
        let sp = LocalBlobStore::new(d.path().join("empty_p")).unwrap();
        let a = publish(&tree, &ss, &ChunkIndex::new(), None).unwrap();
        let b = publish_pipelined(&tree, &sp, &ChunkIndex::new(), None, 4, 8).unwrap();
        assert_eq!(
            a.manifest, b.manifest,
            "empty-tree digest streaming!=pipelined"
        );
        assert_eq!((b.total_chunks, b.new_chunks, b.blocks), (0, 0, 0));
        materialize(&sp, &b.manifest, &d.path().join("empty_o")).unwrap();
        println!("[P1a] empty tree: streaming==pipelined, 0 blocks, materializes: FINE");
    }

    // (b) SYMLINK-ONLY — everything routes to `extra`, nothing to chunk.
    {
        let tree = d.path().join("sym_tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        symlink("target_a", tree.join("l1")).unwrap();
        symlink("nested/target_b", tree.join("sub/l2")).unwrap();
        let ss = LocalBlobStore::new(d.path().join("sym_s")).unwrap();
        let sp = LocalBlobStore::new(d.path().join("sym_p")).unwrap();
        let a = publish(&tree, &ss, &ChunkIndex::new(), None).unwrap();
        let b = publish_pipelined(&tree, &sp, &ChunkIndex::new(), None, 4, 8).unwrap();
        assert_eq!(a.manifest, b.manifest, "symlink-only digest mismatch");
        assert_eq!((b.new_chunks, b.symlinks), (0, 2));
        let out = d.path().join("sym_o");
        materialize(&sp, &b.manifest, &out).unwrap();
        assert_eq!(
            fs::read_link(out.join("l1")).unwrap().to_str().unwrap(),
            "target_a"
        );
        println!("[P1b] symlink-only: streaming==pipelined, 0 chunks, links preserved: FINE");
    }

    // (c) 100% DEDUP — re-publish the same tree with a FULL `known`. The producer dedups every
    // chunk and sends NOTHING through the worker channels; the packer writes 0 blocks.
    {
        let tree = d.path().join("dedup_tree");
        write(&tree.join("f1.bin"), &prng(77));
        write(&tree.join("sub/f2.bin"), &prng(88));
        let ss = LocalBlobStore::new(d.path().join("dedup_s")).unwrap();
        let sp = LocalBlobStore::new(d.path().join("dedup_p")).unwrap();
        let base = publish(&tree, &ss, &ChunkIndex::new(), None).unwrap();
        let known = load_index(&ss, &base.manifest);
        let b = publish_pipelined(&tree, &sp, &known, None, 8, 8).unwrap();
        assert_eq!(base.manifest, b.manifest, "100%-dedup digest mismatch");
        assert_eq!((b.total_chunks, b.new_chunks, b.blocks), (2, 0, 0));
        println!(
            "[P1c] 100% dedup: producer->workers sends 0, packer writes 0 blocks, no hang/panic: FINE"
        );
    }

    // (d) CANONICAL + HARDLINKS through the parallel path at several worker counts.
    {
        let tree = d.path().join("hl_tree");
        write(&tree.join("canon.bin"), &vec![42u8; 300_000]);
        fs::hard_link(tree.join("canon.bin"), tree.join("a_link.bin")).unwrap();
        fs::hard_link(tree.join("canon.bin"), tree.join("b_link.bin")).unwrap();
        let ss = LocalBlobStore::new(d.path().join("hl_s")).unwrap();
        let a = publish(&tree, &ss, &ChunkIndex::new(), None).unwrap();
        for w in [1usize, 4, 16] {
            let sp = LocalBlobStore::new(d.path().join(format!("hl_p{}", w))).unwrap();
            let b = publish_pipelined(&tree, &sp, &ChunkIndex::new(), None, w, 8).unwrap();
            assert_eq!(
                a.manifest, b.manifest,
                "hardlink-tree digest mismatch w={w}"
            );
            let out = d.path().join(format!("hl_o{}", w));
            materialize(&sp, &b.manifest, &out).unwrap();
            let inos: HashSet<u64> = ["canon.bin", "a_link.bin", "b_link.bin"]
                .iter()
                .map(|n| fs::metadata(out.join(n)).unwrap().ino())
                .collect();
            assert_eq!(inos.len(), 1, "hardlinks collapsed to 1 inode w={w}");
        }
        println!(
            "[P1d] canonical+hardlinks via pipeline (w=1/4/16): digest==streaming, 1 inode: FINE"
        );
    }

    // (e) LARGE file whose 4th chunk == a SMALL file's only chunk. The shared chunk is DEFINED by
    // the small file (sorts first) and DEDUP-REUSED mid-way through the large file.
    {
        let tree = d.path().join("share_tree");
        let c = prng(31337);
        write(&tree.join("small.bin"), &c);
        let mut big = Vec::new();
        for k in 0..6u64 {
            if k == 3 {
                big.extend_from_slice(&c);
            } else {
                big.extend(prng(9000 + k));
            }
        }
        write(&tree.join("z_large.bin"), &big);
        let ss = LocalBlobStore::new(d.path().join("share_s")).unwrap();
        let a = publish(&tree, &ss, &ChunkIndex::new(), None).unwrap();
        for w in [1usize, 2, 8] {
            let sp = LocalBlobStore::new(d.path().join(format!("share_p{}", w))).unwrap();
            let b = publish_pipelined(&tree, &sp, &ChunkIndex::new(), None, w, 8).unwrap();
            assert_eq!(a.manifest, b.manifest, "shared-chunk digest mismatch w={w}");
            assert_eq!(
                a.new_chunks, b.new_chunks,
                "shared-chunk dedup mismatch w={w}"
            );
            let out = d.path().join(format!("share_o{}", w));
            materialize(&sp, &b.manifest, &out).unwrap();
            assert_eq!(fs::read(out.join("z_large.bin")).unwrap(), big);
            assert_eq!(fs::read(out.join("small.bin")).unwrap(), c);
        }
        println!(
            "[P1e] large/small shared interior chunk: digest & round-trip stable across w: FINE"
        );
    }
}

// ============================================================================
// PROBE 2 — DESIGN-GAP: gc::collect is NOT concurrency-safe with itself. The sweep uses
// `e.metadata()?` and `remove_file(..)?` with no ENOENT tolerance. Two GCs racing the same
// orphan set (a GC job that overruns and a second is scheduled; two nodes both running GC) will
// double-delete: the loser hits ENOENT and the whole collect() errors out mid-sweep (partial).
// Live data is never corrupted, but GC spuriously fails + leaves the sweep incomplete.
// GC's block sweep is purely read_dir -> metadata -> remove_file, so synthesized orphan block
// files exercise the identical code path; a few real published gens are mixed in.
// ============================================================================
#[test]
fn probe2_concurrent_gc_double_delete_races() {
    let mut spurious = 0usize;
    let mut partial_sweeps = 0usize;
    let mut last_err = String::new();
    let rounds = 12;
    for r in 0..rounds {
        let d = tmp();
        let tree = d.path().join("t");
        let store = d.path().join("s");
        let s = LocalBlobStore::new(&store).unwrap();
        // real gens -> real orphan blocks + superseded manifests (live = HEAD only)
        let mut known = ChunkIndex::new();
        let mut head = String::new();
        for g in 0..3u64 {
            write(
                &tree.join("f.bin"),
                &multi((r as u64 * 1000 + g * 10)..(r as u64 * 1000 + g * 10 + 4)),
            );
            let st = publish(&tree, &s, &known, None).unwrap();
            known = load_index(&s, &st.manifest);
            head = st.manifest;
        }
        // synthesize many OLD-name orphan block files to make the two sweeps overlap reliably
        for i in 0..400u32 {
            let name = format!("{:064x}", 0x0DEAD_0000u64 + i as u64);
            fs::write(store.join("blocks").join(name), b"orphan-block").unwrap();
        }
        let orphans_before = fs::read_dir(store.join("blocks")).unwrap().count();
        let live = vec![head.clone()];
        let barrier = Arc::new(Barrier::new(2));
        let mut hs = Vec::new();
        for _ in 0..2 {
            let (store, live, barrier) = (store.clone(), live.clone(), barrier.clone());
            hs.push(std::thread::spawn(move || -> Result<gc::GcStats> {
                barrier.wait();
                gc::collect(&store, &live, Duration::ZERO)
            }));
        }
        for h in hs {
            if let Err(e) = h.join().unwrap() {
                spurious += 1;
                if last_err.is_empty() {
                    last_err = e.to_string();
                }
            }
        }
        // Even when a GC errored mid-sweep, blocks WERE deleted before the error (partial sweep):
        let orphans_after = fs::read_dir(store.join("blocks")).unwrap().count();
        if orphans_after < orphans_before {
            partial_sweeps += 1;
        }
        // Corruption check: HEAD's live blocks are never deletion candidates -> always intact.
        materialize(&s, &head, &d.path().join("o")).unwrap();
    }
    println!(
        "[P2] concurrent gc::collect over shared orphans: {} spurious errors / {} rounds x 2 threads (live data always survived)",
        spurious, rounds
    );
    println!(
        "[P2] first spurious error = {:?}; rounds where the store made sweep progress = {}/{}",
        last_err, partial_sweeps, rounds
    );
    // FIXED: idempotent under concurrency. Both sweepers tolerate the mid-sweep ENOENT (one deletes
    // a file the other already listed) -> zero spurious errors, and every round still makes progress.
    assert_eq!(
        spurious, 0,
        "concurrent GC must be race-safe (ENOENT tolerated), got error: {last_err}"
    );
    assert_eq!(
        partial_sweeps, rounds,
        "every round's racing sweepers collectively reclaimed the orphans"
    );
}

// ============================================================================
// PROBE 3 — GC + publish under REAL threads (attack 2b). The audit/author only SIMULATED the
// race by hand-removing/restoring manifests. Here a publisher thread runs 15 real generations
// while a GC thread continuously collects with the REAL config (live=[latest HEAD], PROPER
// grace=1h). Assert every committed manifest still materializes byte-correctly.
// Verdict target: FINE — proper grace + marked HEAD = zero corruption under real concurrency.
// ============================================================================
#[test]
fn probe3_real_concurrent_gc_and_publish_no_corruption() {
    let d = tmp();
    let tree = d.path().join("t");
    let store_dir = d.path().join("s");
    let store = Arc::new(LocalBlobStore::new(&store_dir).unwrap());
    write(&tree.join("stable.bin"), &multi(500_000..500_003)); // unchanged -> chunks inherited
    write(&tree.join("churn.bin"), &prng(0));

    let committed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));

    let gc_handle = {
        let (store_dir, committed, stop) = (store_dir.clone(), committed.clone(), stop.clone());
        std::thread::spawn(move || -> (usize, usize) {
            let (mut runs, mut deleted) = (0usize, 0usize);
            while !stop.load(Ordering::Relaxed) {
                let head = committed.lock().unwrap().last().cloned();
                if let Some(h) = head {
                    if let Ok(st) = gc::collect(&store_dir, &[h], Duration::from_secs(3600)) {
                        runs += 1;
                        deleted += st.blocks_deleted;
                    }
                }
                std::thread::sleep(Duration::from_micros(200));
            }
            (runs, deleted)
        })
    };

    let mut known = ChunkIndex::new();
    let mut heads = Vec::new();
    for gen in 0..15u64 {
        write(
            &tree.join("churn.bin"),
            &multi((gen * 100)..(gen * 100 + 4)),
        );
        let st = publish(&tree, &*store, &known, None).unwrap();
        known = load_index(&store, &st.manifest);
        committed.lock().unwrap().push(st.manifest.clone());
        heads.push(st.manifest.clone());
    }
    stop.store(true, Ordering::Relaxed);
    let (gc_runs, gc_deleted) = gc_handle.join().unwrap();

    let mut ok = 0usize;
    for (i, h) in heads.iter().enumerate() {
        match materialize(&*store, h, &d.path().join(format!("o{}", i))) {
            Ok(_) => ok += 1,
            Err(e) => panic!("gen {i} failed to materialize after concurrent GC: {e}"),
        }
    }
    println!(
        "[P3] real concurrent publish(15 gens) + GC(live=[HEAD],grace=1h): gc_runs={} gc_blocks_deleted={} -> all {} HEADs materialize: FINE",
        gc_runs, gc_deleted, ok
    );
    assert_eq!(
        ok,
        heads.len(),
        "all committed manifests survive concurrent GC with proper grace"
    );
    assert_eq!(
        gc_deleted, 0,
        "proper grace: nothing collectable while everything is young/marked"
    );
}

// ============================================================================
// PROBE 4 — attack 2a: a live manifest passed in `live` is MISSING from disk (a dangling
// lineage HEAD, a not-yet-replicated manifest, or one a prior buggy GC already deleted).
// collect() reads every live manifest in the MARK phase (before any deletion), so a missing one
// must FAIL CLOSED: clean Err, ZERO blocks deleted (no half-GC'd store).
// Verdict: FINE for safety (fail-closed) BUT a real ops DESIGN-GAP — one dangling live entry
// WEDGES GC entirely (store grows unbounded) with no partial progress / alerting.
// ============================================================================
#[test]
fn probe4_gc_missing_live_manifest_fails_closed() {
    let d = tmp();
    let tree = d.path().join("t");
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    write(&tree.join("f.bin"), &multi(0..6));
    let g0 = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
    let known0 = load_index(&s, &g0.manifest);
    write(&tree.join("f.bin"), &multi(100..106));
    let _g1 = publish(&tree, &s, &known0, None).unwrap(); // gen0 blocks now orphan on disk

    let blocks_before = fs::read_dir(store.join("blocks")).unwrap().count();
    let bogus = "0".repeat(64); // a live digest that is not on disk
    let r =
        gc::collect(&store, &[bogus], Duration::ZERO).expect("must not wedge on a dangling ref");
    let blocks_after = fs::read_dir(store.join("blocks")).unwrap().count();
    println!(
        "[P4] gc(live=[missing manifest], grace=0): missing={} blocks {}->{} deleted={}",
        r.missing_live_manifests, blocks_before, blocks_after, r.blocks_deleted
    );
    // FIXED: a missing live manifest is SKIPPED + counted (surfaced for alerting), never fatal — one
    // dangling live entry must not wedge GC (unbounded growth). It protects no blocks, so with no
    // real manifest live here every block is an orphan and gets reclaimed (progress, not a wedge).
    assert_eq!(r.missing_live_manifests, 1, "dangling live ref counted");
    assert!(blocks_before > 0);
    assert_eq!(
        blocks_after, 0,
        "GC still makes progress despite the dangling live ref (no wedge)"
    );
}

// ============================================================================
// PROBE 5 — cross-cutting: does GC agree with publish_pipelined (not just streaming publish)?
// Every prior GC test published via streaming `publish`. Here: pipelined publish, GC with it
// live (grace=0) keeps ALL its blocks and it materializes; then supersede with a second pipelined
// gen and confirm the orphaned blocks are reclaimed while the new HEAD stays intact.
// ============================================================================
#[test]
fn probe5_gc_agrees_with_pipelined_publish() {
    let d = tmp();
    let tree = d.path().join("t");
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    for i in 0..10u64 {
        write(&tree.join(format!("f{}.bin", i)), &prng(700 + i));
    }
    let g0 = publish_pipelined(&tree, &s, &ChunkIndex::new(), None, 4, 8).unwrap();
    let st0 = gc::collect(&store, &[g0.manifest.clone()], Duration::ZERO).unwrap();
    println!(
        "[P5] after pipelined publish, GC(live=[HEAD],grace=0): kept={} deleted={}",
        st0.blocks_kept, st0.blocks_deleted
    );
    assert_eq!(
        st0.blocks_deleted, 0,
        "no live block collected after a pipelined publish"
    );
    materialize(&s, &g0.manifest, &d.path().join("o0")).unwrap();

    let known0 = load_index(&s, &g0.manifest);
    for i in 0..10u64 {
        write(&tree.join(format!("f{}.bin", i)), &prng(800 + i));
    }
    let g1 = publish_pipelined(&tree, &s, &known0, None, 4, 8).unwrap();
    let st1 = gc::collect(&store, &[g1.manifest.clone()], Duration::ZERO).unwrap();
    println!(
        "[P5] after superseding pipelined gen, GC(live=[gen1],grace=0): kept={} deleted={}",
        st1.blocks_kept, st1.blocks_deleted
    );
    assert!(
        st1.blocks_deleted >= 1,
        "orphan blocks from the superseded pipelined gen are reclaimed"
    );
    materialize(&s, &g1.manifest, &d.path().join("o1")).unwrap();
    println!(
        "[P5] pipelined publish + GC agree (live kept, orphans reclaimed, HEAD materializes): FINE"
    );
}

// ============================================================================
// PROBE 6 — FIXED: leftover `.tmp` files are reclaimed once older than grace. put_block writes
// `<id>.<pid>.<n>.tmp` then renames; a crash between write and rename leaves a temp block on disk.
// GC now reclaims a `.tmp` when `age >= grace` (a young one could belong to an in-flight write, so
// it's protected). Verify an OLD leftover .tmp is collected and counted in tmp_deleted.
// ============================================================================
#[test]
fn probe6_gc_never_collects_leftover_tmp() {
    let d = tmp();
    let store = d.path().join("s");
    let _s = LocalBlobStore::new(&store).unwrap();
    let tmpname = format!("{:064x}.{}.{}.tmp", 0x00C0FFEEu64, 4242, 0);
    let tmp_path = store.join("blocks").join(&tmpname);
    fs::write(&tmp_path, &vec![0u8; 8192]).unwrap();
    backdate(&tmp_path); // old by every measure

    let st = gc::collect(&store, &[], Duration::ZERO).unwrap();
    let leaked = tmp_path.exists();
    println!(
        "[P6] old leftover .tmp (crashed publish): tmp_deleted={} -> .tmp still present? {}",
        st.tmp_deleted, leaked
    );
    assert!(
        !leaked,
        "stale .tmp must be reclaimed (no unbounded leak across crashes)"
    );
    assert_eq!(st.tmp_deleted, 1, "the reclaimed .tmp is counted");
}

// ============================================================================
// PROBE 7 — attack 2c: does grace protect the in-flight MANIFEST object itself (not just its
// blocks)? A publish writes blocks then the manifest, then advances HEAD. Between manifest-write
// and HEAD-advance the manifest is on disk but NOT in the live set. A GC in that window must
// protect the young manifest; and once aged past grace + still not live it is correctly collected
// (isolating grace as the protector).
// ============================================================================
#[test]
fn probe7_grace_protects_in_flight_manifest_object() {
    let d = tmp();
    let tree = d.path().join("t");
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    write(&tree.join("f.bin"), &multi(4000..4005));
    let g = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();

    let st = gc::collect(&store, &[], Duration::from_secs(3600)).unwrap();
    let present = store.join("manifests").join(&g.manifest).exists();
    println!(
        "[P7] grace=1h, live=[]: manifests_deleted={} -> young in-flight manifest survives? {}",
        st.manifests_deleted, present
    );
    assert!(
        present,
        "grace must protect the young (not-yet-live) manifest object"
    );
    assert_eq!(st.manifests_deleted, 0);
    materialize(&s, &g.manifest, &d.path().join("o")).unwrap();

    // CONTRAST: age everything past grace, still not live -> collected (grace WAS the protector).
    for sub in ["manifests", "blocks"] {
        for e in fs::read_dir(store.join(sub)).unwrap() {
            backdate(&e.unwrap().path());
        }
    }
    let st2 = gc::collect(&store, &[], Duration::ZERO).unwrap();
    let gone = !store.join("manifests").join(&g.manifest).exists();
    println!(
        "[P7] aged + still not live, grace=0: manifests_deleted={} -> collected? {}",
        st2.manifests_deleted, gone
    );
    assert!(
        gone,
        "an aged, non-live manifest is collected once grace no longer protects it: FINE"
    );
}

// A store whose put_block fails after `fail_after` successful writes (simulates an S3/disk hiccup).
struct FlakyStore {
    inner: LocalBlobStore,
    calls: AtomicUsize,
    fail_after: usize,
}
impl BlobStore for FlakyStore {
    fn put_block(&self, id: &BlockId, bytes: &[u8]) -> Result<()> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n >= self.fail_after {
            anyhow::bail!("injected put_block failure");
        }
        self.inner.put_block(id, bytes)
    }
    fn get_block(&self, id: &BlockId) -> Result<Vec<u8>> {
        self.inner.get_block(id)
    }
    fn put_manifest(&self, d: &str, b: &[u8]) -> Result<()> {
        self.inner.put_manifest(d, b)
    }
    fn get_manifest(&self, d: &str) -> Result<Vec<u8>> {
        self.inner.get_manifest(d)
    }
    fn has_block(&self, id: &BlockId) -> bool {
        self.inner.has_block(id)
    }
}

// ============================================================================
// PROBE 8 — attack 1a/1b: a store failure MID-STREAM (the packer flushes a full 64 MiB block and
// put_block fails WHILE the producer is still feeding). This is the classic pipeline deadlock
// shape: consumer (packer) dies, does the producer wedge forever on a full channel? The audit
// only tested fail_after=0 on a sub-block tree (single final flush). Here we push >64 MiB so a
// block flushes mid-stream, and run under a watchdog so a real hang is REPORTED, not left to
// wedge the whole suite.
// ============================================================================
#[test]
fn probe8_store_failure_midstream_no_hang() {
    let d = tmp();
    let tree = d.path().join("t");
    // ~100 MiB of NEW incompressible data -> compressed ~100 MiB > 64 MiB BLOCK_TARGET -> the
    // first flush happens mid-stream (producer still has ~36 MiB to feed when the packer fails).
    for i in 0..100u64 {
        let mut body = Vec::with_capacity(1024 * 1024);
        for k in 0..4u64 {
            body.extend(prng(2_000_000 + i * 10 + k));
        }
        write(&tree.join(format!("f{}.bin", i)), &body);
    }

    let treep = tree.clone();
    let sdir = d.path().join("s");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let store = FlakyStore {
            inner: LocalBlobStore::new(&sdir).unwrap(),
            calls: AtomicUsize::new(0),
            fail_after: 0, // fail the very first (mid-stream) block flush
        };
        let r = publish_pipelined(&treep, &store, &ChunkIndex::new(), None, 4, 4);
        let _ = tx.send(r.is_err());
    });
    match rx.recv_timeout(Duration::from_secs(60)) {
        Ok(is_err) => {
            println!(
                "[P8] pipelined mid-stream put_block failure -> err={} (returned, no hang)",
                is_err
            );
            assert!(
                is_err,
                "a packer put_block failure must surface as a clean Err"
            );
        }
        Err(_) => panic!(
            "[P8] DEADLOCK: publish_pipelined did not return within 60s after the packer died"
        ),
    }

    // Streaming path with the same failure (no threads) must also be a clean Err.
    let sdir2 = d.path().join("s2");
    let store2 = FlakyStore {
        inner: LocalBlobStore::new(&sdir2).unwrap(),
        calls: AtomicUsize::new(0),
        fail_after: 0,
    };
    let r2 = publish(&tree, &store2, &ChunkIndex::new(), None);
    println!(
        "[P8] streaming mid-stream put_block failure -> err={}",
        r2.is_err()
    );
    assert!(
        r2.is_err(),
        "streaming publish must also be a clean Err on put_block failure: FINE"
    );
}
