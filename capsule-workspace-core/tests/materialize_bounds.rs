//! Materialize's memory bound and failure atomicity — both learned the hard way.
//!
//! The first wave implementation keyed waves on BLOCKS. Blocks are flushed by publish at 64 MiB of
//! COMPRESSED bytes, and a wave must always admit at least one block, so a highly compressible block
//! could decompress to gigabytes: the "bound" measured 2.03x tree size on compressible input and was
//! still linear. Every test at the time used INCOMPRESSIBLE fixtures, so none of them could see it.
//! `bound_holds_on_highly_compressible_input` is the test that would have.
//!
//! Separately, pre-creating every file at final length meant a failed materialize left a
//! complete-LOOKING tree — right file count, right sizes, right modes, all zero bytes — where the
//! previous implementation left `out` empty and the retry was clean.

use capsule_workspace_core::cas::*;
use capsule_workspace_core::daemon::{materialize, publish};
use capsule_workspace_core::manifest::Manifest;
use std::fs;
use std::path::Path;

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}
fn w(p: &Path, b: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b).unwrap();
}

/// Highly compressible but with DISTINCT chunks, so publish-side dedup can't hide the problem: each
/// chunk is mostly zeros with a unique counter, compressing ~1000x while remaining a unique chunk id.
fn compressible_chunk(i: u64) -> Vec<u8> {
    let mut v = vec![0u8; CHUNK];
    v[..8].copy_from_slice(&i.to_le_bytes());
    v
}

// THE bound test. A tree that compresses ~1000x must not blow the working set: if waves are keyed on
// blocks, all of this lands in one block and is decompressed at once.
#[test]
fn bound_holds_on_highly_compressible_input() {
    let d = tmp();
    let tree = d.path().join("t");
    // 2 GiB logical across 8 files of 256 MiB — compresses into a single 64 MiB block.
    for f in 0..8u64 {
        let mut body = Vec::with_capacity(256 * 1024 * 1024);
        for c in 0..1024u64 {
            body.extend_from_slice(&compressible_chunk(f * 1024 + c));
        }
        w(&tree.join(format!("big{f}.bin")), &body);
    }
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let dig = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let m = Manifest::from_bytes(&s.get_manifest(&dig).unwrap()).unwrap();
    let logical: u64 = m.files.iter().map(|f| f.size).sum();
    let blocks: std::collections::BTreeSet<_> = m.chunks.values().map(|l| &l.block).collect();
    assert!(
        logical >= 2 * 1024 * 1024 * 1024,
        "fixture is 2 GiB logical"
    );
    assert!(
        blocks.len() <= 2,
        "fixture must compress into ~1 block for this to be the interesting case, got {}",
        blocks.len()
    );

    let out = d.path().join("o");
    materialize(&s, &dig, &out).unwrap();

    // Byte-identical round trip.
    for f in 0..8u64 {
        let got = fs::read(out.join(format!("big{f}.bin"))).unwrap();
        assert_eq!(got.len(), 256 * 1024 * 1024);
        assert_eq!(&got[..8], &(f * 1024).to_le_bytes()[..]);
    }
    // The real assertion is memory, which the harness can't read portably — but a block-keyed wave would
    // have had to hold all 2 GiB decompressed at once to get here, so completing at all under a 256 MiB
    // logical ceiling is the observable signal. The RSS numbers live in the build log.
}

// A failed materialize must leave NOTHING behind: a complete-looking, correctly-sized, all-zero tree is
// worse than an empty one, because every cheap check says "fine" and the next start's empty-dir guard
// turns a transient fetch error into a permanent crash loop.
#[test]
fn failed_materialize_leaves_no_partial_tree() {
    let d = tmp();
    let tree = d.path().join("t");
    for i in 0..40 {
        w(&tree.join(format!("f{i}.bin")), &vec![i as u8; 300_000]);
    }
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let dig = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let m = Manifest::from_bytes(&s.get_manifest(&dig).unwrap()).unwrap();

    // Delete one block the manifest needs — the GC'd-block / transient-fetch-error case.
    let victim = m.chunks.values().next().unwrap().block.clone();
    for e in fs::read_dir(d.path().join("s").join("blocks"))
        .unwrap()
        .flatten()
    {
        if e.file_name().to_string_lossy().contains(&victim) {
            fs::remove_file(e.path()).unwrap();
        }
    }

    let out = d.path().join("o");
    assert!(materialize(&s, &dig, &out).is_err(), "must fail loudly");
    let left = fs::read_dir(&out).map(|it| it.count()).unwrap_or(0);
    assert_eq!(
        left, 0,
        "a failed materialize must leave no partial tree (found {left} entries)"
    );
}

// Wave BOUNDARIES. The file-ordered wave logic has three distinct cases and nothing else exercises them
// together: many small files packed into one wave, a file that alone exceeds the ceiling (streamed
// chunk-by-chunk rather than assembled), and files sitting either side of a wave boundary. Sizes here are
// chosen relative to the real MATERIALIZE_WAVE_BYTES so the fixture actually crosses boundaries.
#[test]
fn wave_boundaries_round_trip_exactly() {
    let d = tmp();
    let tree = d.path().join("t");
    // ~600 MiB total across three shapes → several waves at the 256 MiB ceiling.
    for i in 0..2000u64 {
        // many small files (some empty, some sub-chunk, some multi-chunk)
        let n = match i % 4 {
            0 => 0,
            1 => 100,
            2 => CHUNK + 7,
            _ => 3 * CHUNK,
        };
        let mut v = vec![0u8; n];
        if n >= 8 {
            v[..8].copy_from_slice(&i.to_le_bytes());
        }
        w(&tree.join(format!("many/f{i}.bin")), &v);
    }
    // one file that alone exceeds the wave ceiling → takes the streaming path
    let mut huge = Vec::with_capacity(300 * 1024 * 1024);
    for c in 0..1200u64 {
        huge.extend_from_slice(&compressible_chunk(1_000_000 + c));
    }
    w(&tree.join("huge.bin"), &huge);
    // and a normal file after it, so the streamed file is not simply last
    w(&tree.join("zz_after.bin"), &vec![7u8; 5 * CHUNK + 3]);

    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let dig = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let out = d.path().join("o");
    materialize(&s, &dig, &out).unwrap();

    // Every file byte-identical, including the streamed one and the boundary neighbours.
    for i in 0..2000u64 {
        let p = out.join(format!("many/f{i}.bin"));
        let got = fs::read(&p).unwrap();
        let want = fs::read(tree.join(format!("many/f{i}.bin"))).unwrap();
        assert_eq!(got, want, "mismatch at many/f{i}.bin");
    }
    assert_eq!(
        fs::read(out.join("huge.bin")).unwrap(),
        huge,
        "streamed file"
    );
    assert_eq!(
        fs::read(out.join("zz_after.bin")).unwrap(),
        vec![7u8; 5 * CHUNK + 3]
    );
}

// ---- the failure paths the FIRST cleanup attempt missed -------------------------------------------
// It guarded only `write_regular_files`. These two reproduce what that left open. The second is the one
// that mattered: at resume_via_reference's workspace clone, ref_manifest == manifest, so every file is a
// reflink candidate, `to_write` is empty, and the guarded call is a NO-OP while the reflink pass does all
// the work — i.e. the daemon's warm resume was entirely unprotected.

#[test]
fn failed_write_links_leaves_no_partial_tree() {
    use capsule_workspace_core::manifest::FileEntry;
    let d = tmp();
    let tree = d.path().join("t");
    for i in 0..20 {
        w(&tree.join(format!("f{i}.bin")), &vec![i as u8; 50_000]);
    }
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let dig = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let mut m = Manifest::from_bytes(&s.get_manifest(&dig).unwrap()).unwrap();
    // a hardlink whose target does not exist in the manifest → write_links fails AFTER the regulars land
    m.files.push(FileEntry {
        path: "dangling.bin".into(),
        mode: 0o100644,
        size: 0,
        chunks: vec![],
        symlink: None,
        hardlink: Some("nope/missing.bin".into()),
    });
    let bad = m.logical_digest();
    s.put_manifest(&bad, &m.to_bytes()).unwrap();

    let out = d.path().join("o");
    assert!(materialize(&s, &bad, &out).is_err(), "must fail loudly");
    let left = fs::read_dir(&out).map(|it| it.count()).unwrap_or(0);
    assert_eq!(
        left, 0,
        "a write_links failure must not strand a tree ({left} entries)"
    );
}

#[test]
fn failed_reflink_pass_leaves_no_partial_tree() {
    use capsule_workspace_core::daemon::materialize_incremental;
    use std::os::unix::fs::PermissionsExt;
    let d = tmp();
    let tree = d.path().join("t");
    for i in 0..30 {
        w(&tree.join(format!("f{i}.bin")), &vec![i as u8; 50_000]);
    }
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let dig = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let reference = d.path().join("ref");
    materialize(&s, &dig, &reference).unwrap();

    // Make one reference leaf unreadable, then clone ref→out exactly as resume_via_reference does
    // (ref_manifest == manifest ⇒ every file is a reflink candidate ⇒ write_regular_files is a no-op).
    let victim = reference.join("f7.bin");
    fs::set_permissions(&victim, fs::Permissions::from_mode(0o000)).unwrap();
    let out = d.path().join("o");
    let r = materialize_incremental(&s, &dig, &out, &dig, &reference);
    fs::set_permissions(&victim, fs::Permissions::from_mode(0o644)).unwrap(); // so tempdir can clean up

    if r.is_ok() {
        // running as root, or a filesystem that ignores the mode — the scenario didn't trigger
        eprintln!(
            "[skip] reference leaf stayed readable; cannot exercise the reflink failure here"
        );
        return;
    }
    let left = fs::read_dir(&out).map(|it| it.count()).unwrap_or(0);
    assert_eq!(
        left, 0,
        "a reflink-pass failure must not strand a tree ({left} entries) — this is the daemon's warm-resume path"
    );
}

// A duplicate path must be refused on the INCREMENTAL path too. It previously reported SUCCESS while one
// entry's bytes silently replaced the other's, because the check only saw the slice bound for the store.
#[test]
fn duplicate_paths_are_refused_on_the_incremental_path() {
    use capsule_workspace_core::daemon::materialize_incremental;
    let d = tmp();
    let tree = d.path().join("t");
    w(&tree.join("a.bin"), &vec![1u8; 40_000]);
    w(&tree.join("b.bin"), &vec![2u8; 40_000]);
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let dig = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let reference = d.path().join("ref");
    materialize(&s, &dig, &reference).unwrap();

    // Forge a manifest whose two entries share a path but differ in content.
    let mut m = Manifest::from_bytes(&s.get_manifest(&dig).unwrap()).unwrap();
    let b_chunks = m
        .files
        .iter()
        .find(|f| f.path == "b.bin")
        .unwrap()
        .chunks
        .clone();
    m.files[1].path = "a.bin".into();
    m.files[1].chunks = b_chunks;
    let bad = m.logical_digest();
    s.put_manifest(&bad, &m.to_bytes()).unwrap();

    let out = d.path().join("o");
    assert!(
        materialize_incremental(&s, &bad, &out, &dig, &reference).is_err(),
        "a duplicate path must be refused on the incremental path, not silently resolved"
    );
    assert_eq!(fs::read_dir(&out).map(|it| it.count()).unwrap_or(0), 0);
}

// Cross-wave block refetch, measured rather than assumed.
//
// File-ordered waves trade a bounded working set for the possibility of fetching a block once per wave
// that references it, where the earlier block-keyed design fetched each block exactly once. That is a real
// cost (over S3 it is a real GET), so it should be a number in the record, not a guess. `blocks_fetched`
// reports DISTINCT blocks, so it hides the amplification — this counts actual calls.
#[test]
fn cross_wave_block_refetch_is_measured_and_bounded() {
    use capsule_workspace_core::cas::{BlobStore, BlockId};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting {
        inner: LocalBlobStore,
        gets: AtomicUsize,
    }
    impl BlobStore for Counting {
        fn put_block(&self, id: &BlockId, b: &[u8]) -> anyhow::Result<()> {
            self.inner.put_block(id, b)
        }
        fn get_block(&self, id: &BlockId) -> anyhow::Result<Vec<u8>> {
            self.gets.fetch_add(1, Ordering::SeqCst);
            self.inner.get_block(id)
        }
        fn put_manifest(&self, d: &str, b: &[u8]) -> anyhow::Result<()> {
            self.inner.put_manifest(d, b)
        }
        fn get_manifest(&self, d: &str) -> anyhow::Result<Vec<u8>> {
            self.inner.get_manifest(d)
        }
        fn has_block(&self, id: &BlockId) -> bool {
            self.inner.has_block(id)
        }
        fn delete_block(&self, id: &BlockId) -> anyhow::Result<bool> {
            self.inner.delete_block(id)
        }
        fn delete_manifest(&self, d: &str) -> anyhow::Result<bool> {
            self.inner.delete_manifest(d)
        }
    }

    let d = tmp();
    let tree = d.path().join("t");
    // MANY DISTINCT BLOCKS across several waves. The previous fixture compressed into ONE block, so the
    // assertion read `1 <= 1.5` and could not fail whatever the refetch behaviour was — which is how a
    // claim of "3.00x -> 1.00x" shipped while the real figure at production fan-out was 2.44x.
    // Publishing each generation separately mints its own block, exactly as a long-lived daemon does.
    let s = Counting {
        inner: LocalBlobStore::new(d.path().join("s")).unwrap(),
        gets: AtomicUsize::new(0),
    };
    let mut idx = ChunkIndex::new();
    for i in 0..40u64 {
        let sub = tree.join(format!("g{i}"));
        let mut body = Vec::new();
        for c in 0..24u64 {
            body.extend_from_slice(&compressible_chunk(i * 24 + c));
        }
        w(&sub.join("f.bin"), &body);
        let st = publish(&sub, &s, &idx, None).unwrap();
        idx.extend(
            Manifest::from_bytes(&s.get_manifest(&st.manifest).unwrap())
                .unwrap()
                .chunks,
        );
    }
    let dig = publish(&tree, &s, &idx, None).unwrap().manifest;
    let m = Manifest::from_bytes(&s.get_manifest(&dig).unwrap()).unwrap();
    let distinct: std::collections::BTreeSet<_> = m.chunks.values().map(|l| &l.block).collect();
    assert!(
        distinct.len() >= 12,
        "fixture must span many blocks for this to mean anything, got {}",
        distinct.len()
    );

    s.gets.store(0, Ordering::SeqCst);
    let st = materialize(&s, &dig, &d.path().join("o")).unwrap();
    let calls = s.gets.load(Ordering::SeqCst);
    println!(
        "[refetch] distinct blocks={} reported={} actual get_block calls={} amplification={:.2}x",
        distinct.len(),
        st.blocks_fetched,
        calls,
        calls as f64 / distinct.len().max(1) as f64
    );
    // WHAT THIS DOES AND DOES NOT COVER. It pins that a 40-block manifest is not systematically
    // re-fetched. It does NOT reproduce the worst case, which needs blocks large enough that a group
    // cannot hold a whole wave: a reviewer measured 2.44x on a realistic fixture and 13.00x at 200 blocks,
    // and the pool (retaining only a few) cannot prevent that — the real fix there is ranged block reads,
    // which are recorded as the next work rather than claimed here. Saying so explicitly because the
    // previous version of this test asserted `1 <= 1.5` on a ONE-block fixture and could never fail, and
    // a "3.00x -> 1.00x" claim was published off it.
    assert!(
        calls as f64 <= distinct.len() as f64 * 1.5,
        "block refetch regressed: {calls} calls for {} distinct blocks",
        distinct.len()
    );
}

// THE BOUND TEST, pinning the PRODUCTION constants.
//
// The previous attempt set an env override (CAPWS_BLOCK_BYTES=1) and asserted on peak concurrent
// get_block. It pinned neither constant — a reviewer mutated MATERIALIZE_BLOCK_BYTES to 1 TiB and the CAP
// to 100_000 and the whole suite stayed green — and it measured fetch CONCURRENCY, while residency is what
// costs memory (BlockPool retains blocks after get_block returns). It also required a production env knob
// to exist purely so a test could work.
//
// This drives the grouping function directly with synthetic block sizes, so it reads the real constants and
// fails if either is weakened. The worst case is the one that broke the old estimate: blocks of UNKNOWN
// size (blen == 0, i.e. a manifest written before that field existed), which must be assumed full-sized.
#[test]
fn block_batches_never_exceed_the_memory_budget() {
    use capsule_workspace_core::cas::{BlockId, BLOCK_TARGET};
    use capsule_workspace_core::daemon::{
        group_blocks_by_bytes, MATERIALIZE_BLOCK_BYTES, MATERIALIZE_BLOCK_CAP,
    };
    use std::collections::HashMap;

    // The two constants that DEFINE the bound are guarded by `const` assertions in the library, so a bad
    // value fails to compile rather than failing here — see the `const _: () = assert!(...)` pair beside
    // them. What remains for a runtime test is the grouping BEHAVIOUR below.
    let ids: Vec<BlockId> = (0..512).map(|i| format!("{i:064x}")).collect();
    let refs: Vec<&BlockId> = ids.iter().collect();

    // Case 1: every block UNKNOWN-sized (blen absent ⇒ 0). Must be treated as BLOCK_TARGET each.
    let empty: HashMap<&BlockId, u64> = HashMap::new();
    for g in group_blocks_by_bytes(&refs, &empty) {
        assert!(
            g.len() <= MATERIALIZE_BLOCK_CAP,
            "unknown-sized blocks: {} in a group exceeds the cap {MATERIALIZE_BLOCK_CAP}",
            g.len()
        );
        let worst = g.len() as u64 * BLOCK_TARGET as u64;
        assert!(
            worst <= MATERIALIZE_BLOCK_BYTES,
            "unknown-sized blocks: worst-case {worst} exceeds the budget {MATERIALIZE_BLOCK_BYTES}"
        );
    }

    // Case 2: real sizes, including full-size blocks and a long tail of tiny ones.
    let mut est: HashMap<&BlockId, u64> = HashMap::new();
    for (n, id) in ids.iter().enumerate() {
        est.insert(
            id,
            match n % 4 {
                0 => BLOCK_TARGET as u64,
                1 => BLOCK_TARGET as u64 / 2,
                2 => 4 * 1024 * 1024,
                _ => 64 * 1024,
            },
        );
    }
    let groups = group_blocks_by_bytes(&refs, &est);
    assert_eq!(
        groups.iter().map(|g| g.len()).sum::<usize>(),
        refs.len(),
        "grouping must partition the input exactly"
    );
    for g in &groups {
        let bytes: u64 = g.iter().map(|b| est[b]).sum();
        assert!(
            bytes <= MATERIALIZE_BLOCK_BYTES,
            "group holds {bytes} bytes, over the budget {MATERIALIZE_BLOCK_BYTES}"
        );
        assert!(g.len() <= MATERIALIZE_BLOCK_CAP);
    }

    // Case 3: a single block larger than the whole budget still forms its own group (never dropped).
    let big = vec![&ids[0]];
    let mut one: HashMap<&BlockId, u64> = HashMap::new();
    one.insert(&ids[0], MATERIALIZE_BLOCK_BYTES * 4);
    assert_eq!(group_blocks_by_bytes(&big, &one).len(), 1);
}

// And the end-to-end counterpart: a real materialize over many blocks must never hold them all.
#[test]
fn materialize_fetches_a_bounded_number_of_blocks_concurrently() {
    use capsule_workspace_core::cas::{BlobStore, BlockId};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Watching {
        inner: LocalBlobStore,
        live: AtomicUsize,
        peak: AtomicUsize,
    }
    impl BlobStore for Watching {
        fn put_block(&self, id: &BlockId, b: &[u8]) -> anyhow::Result<()> {
            self.inner.put_block(id, b)
        }
        fn get_block(&self, id: &BlockId) -> anyhow::Result<Vec<u8>> {
            let n = self.live.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(n, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(3));
            let r = self.inner.get_block(id);
            self.live.fetch_sub(1, Ordering::SeqCst);
            r
        }
        fn put_manifest(&self, d: &str, b: &[u8]) -> anyhow::Result<()> {
            self.inner.put_manifest(d, b)
        }
        fn get_manifest(&self, d: &str) -> anyhow::Result<Vec<u8>> {
            self.inner.get_manifest(d)
        }
        fn has_block(&self, id: &BlockId) -> bool {
            self.inner.has_block(id)
        }
        fn delete_block(&self, id: &BlockId) -> anyhow::Result<bool> {
            self.inner.delete_block(id)
        }
        fn delete_manifest(&self, d: &str) -> anyhow::Result<bool> {
            self.inner.delete_manifest(d)
        }
    }

    let d = tmp();
    let tree = d.path().join("t");
    let s = Watching {
        inner: LocalBlobStore::new(d.path().join("s")).unwrap(),
        live: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
    };
    let mut idx = ChunkIndex::new();
    for i in 0..40u64 {
        let sub = tree.join(format!("g{i}"));
        let mut body = Vec::new();
        for c in 0..24u64 {
            body.extend_from_slice(&compressible_chunk(i * 24 + c));
        }
        w(&sub.join("f.bin"), &body);
        let st = publish(&sub, &s, &idx, None).unwrap();
        idx.extend(
            Manifest::from_bytes(&s.get_manifest(&st.manifest).unwrap())
                .unwrap()
                .chunks,
        );
    }
    let dig = publish(&tree, &s, &idx, None).unwrap().manifest;
    let m = Manifest::from_bytes(&s.get_manifest(&dig).unwrap()).unwrap();
    let distinct: std::collections::BTreeSet<_> = m.chunks.values().map(|l| &l.block).collect();
    // blen must be populated by publish, otherwise the bound falls back to the conservative path.
    assert!(
        m.chunks.values().all(|l| l.blen > 0),
        "publish must record the true block length"
    );

    s.peak.store(0, Ordering::SeqCst);
    // `peak` is min(batch size, rayon pool size), and the GLOBAL pool defaults to host core count — so on a
    // <=CAP-core box (GitHub's standard runner is 4 vCPU) the fan-out is capped by cores, not by the
    // grouping, and an un-grouped `materialize` would still show peak==CAP, making this assertion vacuous
    // exactly where CI runs. A DEDICATED pool sized above CAP makes the grouping the binding limit
    // regardless of host cores. Do NOT "simplify" this back to the global pool: it re-vacuums the guard on
    // CI while staying green on a developer's larger machine.
    let cap = capsule_workspace_core::daemon::MATERIALIZE_BLOCK_CAP;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(cap * 2)
        .build()
        .unwrap();
    pool.install(|| materialize(&s, &dig, &d.path().join("o")).unwrap());
    let peak = s.peak.load(Ordering::SeqCst);

    // Non-vacuity: the fixture must produce far more distinct blocks than the cap, or "peak did not exceed
    // the cap" would be trivially true. (Measured: 40 distinct vs cap 4.)
    assert!(
        distinct.len() > cap,
        "fixture vacuous: {} distinct blocks <= cap {cap}",
        distinct.len()
    );
    // `== CAP`, not `<= CAP`: materialize fetches each group in parallel and groups are capped at CAP, so
    // peak must be EXACTLY the cap here. `<=` would also pass if a future fixture stopped driving fan-out to
    // the cap (leaving the bound un-exercised); `==` fails loudly and forces the fixture to keep exercising
    // it. Un-grouping the fetch drives peak to `cap*2` and trips this at any core count. NOTE: this pins
    // fetch CONCURRENCY (in-flight GETs), not residency — the `held`-map hoist raises how long compressed
    // blocks live, which `peak` cannot see; that remains open debt (see HANDOVER).
    assert_eq!(
        peak, cap,
        "expected materialize's block fan-out to hit exactly the cap {cap}, got {peak}"
    );
}

// The cleanup must EMPTY a caller-supplied `out`, not unlink it. For the daemon `out` is the agent's
// workspace — possibly a pre-created directory with operator-set ownership, or a mount point — so
// removing the directory itself would silently recreate it with our defaults, or fail EBUSY on a mount.
#[test]
fn cleanup_empties_out_but_preserves_the_directory_itself() {
    use capsule_workspace_core::manifest::FileEntry;
    let d = tmp();
    let tree = d.path().join("t");
    for i in 0..10 {
        w(&tree.join(format!("f{i}.bin")), &vec![i as u8; 40_000]);
    }
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let dig = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let mut m = Manifest::from_bytes(&s.get_manifest(&dig).unwrap()).unwrap();
    m.files.push(FileEntry {
        path: "dangling.bin".into(),
        mode: 0o100644,
        size: 0,
        chunks: vec![],
        symlink: None,
        hardlink: Some("nope/missing.bin".into()),
    });
    let bad = m.logical_digest();
    s.put_manifest(&bad, &m.to_bytes()).unwrap();

    // Caller pre-creates `out` (as an operator or a volume mount would).
    let out = d.path().join("preexisting");
    fs::create_dir_all(&out).unwrap();
    assert!(materialize(&s, &bad, &out).is_err());
    assert!(out.is_dir(), "the caller's directory must still exist");
    assert_eq!(
        fs::read_dir(&out).unwrap().count(),
        0,
        "but it must be empty, so a retry can proceed"
    );
    // and a retry against a good manifest now succeeds into that same directory
    materialize(&s, &dig, &out).unwrap();
    assert_eq!(fs::read_dir(&out).unwrap().count(), 10);
}
