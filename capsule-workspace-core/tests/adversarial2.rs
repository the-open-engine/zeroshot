//! Second-wave adversarial / realistic-edge probes (issue #744 follow-up).
//! These target gaps NOT covered by the first 15 tests or the prior audit. Each test is
//! written to PASS by asserting the ACTUAL observed behavior, and prints evidence under
//! `cargo test --release -- --nocapture`. Comments mark verdict: BUG / DESIGN-GAP / FINE.

use capsule_workspace_core::cas::*;
use capsule_workspace_core::daemon::{materialize, publish};
use capsule_workspace_core::manifest::{FileEntry, Manifest};
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt};
use std::path::Path;
use std::sync::Arc;

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}
fn write(p: &Path, bytes: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}
/// One 256 KiB chunk of DISTINCT, INCOMPRESSIBLE bytes seeded by `seed` (splitmix64 stream).
/// Distinct seed => distinct chunk id (no accidental cross-generation dedup); high entropy =>
/// zstd cannot collapse it, so on-disk block bytes reflect real churn.
fn prng_chunk(seed: u64) -> Vec<u8> {
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
fn load_index(s: &LocalBlobStore, dig: &str) -> ChunkIndex {
    Manifest::from_bytes(&s.get_manifest(dig).unwrap())
        .unwrap()
        .chunks
}

// ============================================================================
// PROBE 1 — DESIGN-GAP: empty directories are silently dropped.
// R3 says the tree is opaque ("no assumptions about project structure"); dropping empty
// dirs IS an assumption that they don't matter. Real trees rely on them: dist/, logs/,
// __pycache__ placeholders, python namespace dirs, .git/refs/*, cache dirs, mountpoints.
// ============================================================================
#[test]
fn empty_directories_silently_dropped() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("keep.txt"), b"x");
    fs::create_dir_all(tree.join("logs")).unwrap(); // empty dir with a real purpose
    fs::create_dir_all(tree.join("a/b/c/deep_empty")).unwrap(); // empty nested chain
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let dig = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let out = d.path().join("o");
    materialize(&s, &dig, &out).unwrap();

    let logs_gone = !out.join("logs").exists();
    let deep_gone = !out.join("a/b/c/deep_empty").exists();
    println!("[P1] empty 'logs/' preserved? {}", !logs_gone);
    println!("[P1] empty 'a/b/c/deep_empty' preserved? {}", !deep_gone);
    // DESIGN-GAP: both empty dirs vanish on round-trip.
    assert!(
        logs_gone && deep_gone,
        "documents the gap: empty dirs dropped"
    );
}

// ============================================================================
// PROBE 2 — DESIGN-GAP: hardlinks are broken; the two-paths-one-inode relationship is
// lost and the materialized tree explodes to N full copies. This is the node_modules /
// pnpm / npm case: a pnpm store links every package file; a cargo/bazel cache hardlinks.
// Storage in the CAS dedups (fine) but the *materialized NVMe tree* pays full size, which
// can push a "fits in 5GB" workspace past the R2 budget after materialize.
// ============================================================================
#[test]
fn hardlinks_preserved() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("pkg/real.js"), &vec![9u8; 200_000]);
    fs::create_dir_all(tree.join("node_modules")).unwrap();
    // 2 more paths hardlinked to the same inode (pnpm/npm-style dedup on disk)
    fs::hard_link(tree.join("pkg/real.js"), tree.join("node_modules/a.js")).unwrap();
    fs::hard_link(tree.join("pkg/real.js"), tree.join("node_modules/b.js")).unwrap();

    let src_meta = fs::metadata(tree.join("pkg/real.js")).unwrap();
    println!(
        "[P2] source nlink for the shared inode = {} (all 3 paths share 1 inode)",
        src_meta.nlink()
    );
    assert!(src_meta.nlink() >= 3, "sanity: source really is hardlinked");

    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let dig = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let out = d.path().join("o");
    materialize(&s, &dig, &out).unwrap();

    let m1 = fs::metadata(out.join("pkg/real.js")).unwrap();
    let m2 = fs::metadata(out.join("node_modules/a.js")).unwrap();
    let m3 = fs::metadata(out.join("node_modules/b.js")).unwrap();
    let distinct_inodes: HashSet<u64> = [m1.ino(), m2.ino(), m3.ino()].into_iter().collect();
    println!(
        "[P2] materialized: nlink={} inodes_distinct={} apparent_bytes={}",
        m1.nlink(),
        distinct_inodes.len(),
        m1.len() + m2.len() + m3.len()
    );
    // FIXED (E11): the hardlink relationship is preserved -> ONE inode, nlink==3, no N-copy
    // blowup on the materialized NVMe tree (pnpm/npm/cargo case).
    assert_eq!(
        distinct_inodes.len(),
        1,
        "all three paths share one inode after materialize"
    );
    assert_eq!(m1.nlink(), 3, "hardlink count preserved");
    // content still correct
    assert_eq!(
        fs::read(out.join("pkg/real.js")).unwrap(),
        fs::read(out.join("node_modules/a.js")).unwrap()
    );
}

// ============================================================================
// PROBE 3 — DESIGN-GAP/BUG: the manifest chunk index is CUMULATIVE across a lineage.
// publish() embeds ALL of `known` into every manifest (`index = known.clone(); extend(new)`),
// so a manifest carries chunk->block locations for chunks NO FILE in that manifest references.
// Over a weeks-long lineage with continuous 5s barriers + churn, the manifest — the first
// object a cold node downloads (R2) — grows without bound, decoupled from the live tree size.
// The 89MB@300k EC2 figure measured ONE publish and structurally could not see this.
// ============================================================================
#[test]
fn manifest_chunk_index_grows_with_churn() {
    let d = tmp();
    let tree = d.path().join("t");
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    const CHUNKS_PER_GEN: usize = 20;
    let mut known = ChunkIndex::new();
    let mut sizes = Vec::new();
    for gen in 0..8u64 {
        // each generation is an ENTIRELY new file body -> 20 all-new, all-distinct chunks
        let mut body = Vec::new();
        for i in 0..CHUNKS_PER_GEN as u64 {
            body.extend(prng_chunk(gen * 1000 + i));
        }
        write(&tree.join("f.bin"), &body);
        let st = publish(&tree, &s, &known, None).unwrap();
        let m = Manifest::from_bytes(&s.get_manifest(&st.manifest).unwrap()).unwrap();
        let referenced: HashSet<&ChunkId> = m.files.iter().flat_map(|f| f.chunks.iter()).collect();
        let idx_len = m.chunks.len();
        let mbytes = s.get_manifest(&st.manifest).unwrap().len();
        sizes.push((gen, idx_len, referenced.len(), mbytes));
        println!(
            "[P3] gen={} manifest.chunks={} referenced_by_files={} DEAD={} manifest_bytes={}",
            gen,
            idx_len,
            referenced.len(),
            idx_len - referenced.len(),
            mbytes
        );
        known = load_index(&s, &st.manifest); // exactly what the CLI's --state does
    }
    let (_, first_idx, first_ref, first_bytes) = sizes[0];
    let (_, last_idx, last_ref, last_bytes) = *sizes.last().unwrap();
    // live tree is CONSTANT (always 20 chunks) but the index and manifest bytes grow ~linearly.
    assert_eq!(first_ref, CHUNKS_PER_GEN);
    assert_eq!(last_ref, CHUNKS_PER_GEN, "live set never grows");
    // FIXED: manifest index now carries ONLY referenced chunks — bounded to live-tree size,
    // no cumulative dead entries across the lineage.
    assert_eq!(first_idx, first_ref, "index == referenced (gen 0)");
    assert_eq!(
        last_idx, last_ref,
        "index stays bounded across churn (no dead entries)"
    );
    println!(
        "[P3] SUMMARY over 8 gens of constant-size churn: manifest bytes {first_bytes} -> {last_bytes} ({:.1}x), live set unchanged",
        last_bytes as f64 / first_bytes as f64
    );
}

// ============================================================================
// PROBE 4 — DESIGN-GAP: no GC. Orphan blocks from superseded generations accumulate on
// disk unboundedly. Quantify: after N generations of full churn, blocks-on-disk / blocks
// referenced-by-HEAD grows with N. (Spec §13 defers GC to phase 3 — this measures the rate.)
// ============================================================================
#[test]
fn orphan_blocks_accumulate_no_gc() {
    let d = tmp();
    let tree = d.path().join("t");
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    let mut known = ChunkIndex::new();
    let mut last_dig = String::new();
    const GENS: usize = 10;
    const CHUNKS_PER_GEN: u64 = 16;
    for gen in 0..GENS as u64 {
        // full churn: 16 all-new, all-distinct, incompressible chunks each generation
        let mut body = Vec::new();
        for i in 0..CHUNKS_PER_GEN {
            body.extend(prng_chunk(1_000_000 + gen * 100 + i));
        }
        write(&tree.join("f.bin"), &body);
        let st = publish(&tree, &s, &known, None).unwrap();
        known = load_index(&s, &st.manifest);
        last_dig = st.manifest;
    }
    // blocks physically present
    let blocks_on_disk: u64 = fs::read_dir(store.join("blocks"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| fs::metadata(e.path()).map(|m| m.len()).unwrap_or(0))
        .sum();
    // blocks actually referenced by the current HEAD manifest, and their on-disk bytes
    let head = Manifest::from_bytes(&s.get_manifest(&last_dig).unwrap()).unwrap();
    let live_blocks: HashSet<BlockId> = head
        .files
        .iter()
        .flat_map(|f| f.chunks.iter())
        .map(|c| head.chunks[c].block.clone())
        .collect();
    let live_block_bytes: u64 = live_blocks
        .iter()
        .map(|b| {
            fs::metadata(store.join("blocks").join(b))
                .map(|m| m.len())
                .unwrap_or(0)
        })
        .sum();
    let manifests_on_disk = fs::read_dir(store.join("manifests")).unwrap().count();
    println!(
        "[P4] after {GENS} gens of full churn: blocks_on_disk_bytes={} live_HEAD_block_bytes={} live_blocks={} manifests_on_disk={} amplification={:.1}x",
        blocks_on_disk,
        live_block_bytes,
        live_blocks.len(),
        manifests_on_disk,
        blocks_on_disk as f64 / live_block_bytes.max(1) as f64
    );
    // DESIGN-GAP: HEAD references ~1 block of live data, but ~GENS generations of blocks +
    // every intermediate manifest remain on disk forever (no GC). On-disk bytes ~= GENS x live.
    assert!(
        live_blocks.len() <= 2,
        "HEAD references ~1 block of live data"
    );
    assert!(
        manifests_on_disk >= GENS,
        "every superseded manifest is retained ({manifests_on_disk})"
    );
    assert!(
        blocks_on_disk >= 8 * live_block_bytes,
        "orphan block bytes accumulate ~linearly: {blocks_on_disk} on disk vs {live_block_bytes} live"
    );
}

// ============================================================================
// PROBE 5 — BUG (latent sandbox escape): materialize() assumes `out` is empty but does not
// enforce it. If `out` already contains a symlink, a manifest regular-file write can go
// THROUGH it and land OUTSIDE the workspace. The existing symlink_no_write_through test only
// covers a symlink defined WITHIN the same manifest (defeated by the 2-phase ordering); a
// symlink already present on disk (reused/partially-materialized scratch dir) bypasses it.
// ============================================================================
#[test]
fn prepopulated_symlink_write_through_escape() {
    let d = tmp();
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    // seed a real chunk so we have a valid (block,loc) to reference
    let seed = d.path().join("seed");
    write(&seed.join("x"), b"PWNED-OUTSIDE");
    let sd = publish(&seed, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let sm = Manifest::from_bytes(&s.get_manifest(&sd).unwrap()).unwrap();
    let cid = sm.files[0].chunks[0].clone();

    // craft a manifest with a single regular file at "evil/pwned"
    let mut m = sm.clone();
    m.files = vec![FileEntry {
        path: "evil/pwned".into(),
        mode: 0o100644,
        size: 13,
        chunks: vec![cid],
        symlink: None,
        hardlink: None,
    }];
    let ld = m.logical_digest();
    s.put_manifest(&ld, &m.to_bytes()).unwrap();

    // The escape target OUTSIDE the workspace `out`.
    let escape = d.path().join("ESCAPE_TARGET");
    fs::create_dir_all(&escape).unwrap();

    // Pre-populate `out` (NON-empty!) with a symlink evil -> escape, exactly what a
    // reused/leftover scratch dir could contain.
    let out = d.path().join("o");
    fs::create_dir_all(&out).unwrap();
    symlink(&escape, out.join("evil")).unwrap();

    let res = materialize(&s, &ld, &out);
    let escaped = escape.join("pwned").exists();
    let escaped_content = fs::read(escape.join("pwned")).ok();
    println!(
        "[P5] materialize result_ok={:?} escaped_write_landed_outside={} content={:?}",
        res.is_ok(),
        escaped,
        escaped_content
            .as_deref()
            .map(|b| String::from_utf8_lossy(b).to_string())
    );
    // FIXED: materialize refuses a non-empty `out`, so a pre-existing symlink cannot be a
    // write-through escape.
    assert!(
        res.is_err(),
        "materialize into a non-empty out must be refused"
    );
    assert!(
        !escaped,
        "no write may escape the workspace via a pre-existing symlink"
    );
}

// ============================================================================
// PROBE 6 — BUG (concurrency): LocalBlobStore::put_block uses a FIXED tmp path
// `<blockid>.tmp` (not a per-writer unique temp). Two writers persisting the SAME block id
// concurrently (two pods on one node that produced identical new content, e.g. same
// `npm install`) collide on that tmp path; the loser's rename can hit ENOENT and fail the
// whole publish. Content is never corrupted, but publishes spuriously error.
// ============================================================================
#[test]
fn concurrent_put_block_same_id_races() {
    use std::sync::Barrier;
    let d = tmp();
    let store = Arc::new(LocalBlobStore::new(d.path().join("s")).unwrap());
    // a ~8 MiB block worth of bytes so the write window is non-trivial
    let bytes = Arc::new(vec![0xABu8; 8 * 1024 * 1024]);
    let id: BlockId = hex_sha256(&bytes);

    let mut total_errors = 0usize;
    let rounds = 40;
    for _ in 0..rounds {
        // fresh store dir per round so put_block's `p.exists()` fast-path doesn't mask the race
        let rd = tmp();
        let store = Arc::new(LocalBlobStore::new(rd.path()).unwrap());
        let n = 8;
        let barrier = Arc::new(Barrier::new(n));
        let mut handles = Vec::new();
        for _ in 0..n {
            let store = store.clone();
            let bytes = bytes.clone();
            let id = id.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store.put_block(&id, &bytes)
            }));
        }
        for h in handles {
            if h.join().unwrap().is_err() {
                total_errors += 1;
            }
        }
    }
    println!(
        "[P6] concurrent put_block of identical id: {} spurious errors across {} rounds x 8 threads",
        total_errors, rounds
    );
    // The block content, when present, is always correct (no corruption) — verify once.
    let rd = tmp();
    let store2 = LocalBlobStore::new(rd.path()).unwrap();
    store2.put_block(&id, &bytes).unwrap();
    assert_eq!(
        hex_sha256(&store2.get_block(&id).unwrap()),
        id,
        "no corruption"
    );
    // FIXED: per-writer unique temp name -> concurrent identical put_block never spuriously fails.
    assert_eq!(
        total_errors, 0,
        "concurrent identical put_block must not spuriously fail"
    );
    let _ = &store;
}

// ============================================================================
// PROBE 7 — BUG (latent DoS): materialize() feeds the manifest's `size` field straight into
// `Vec::with_capacity(f.size as usize)` with no validation against the actual chunk bytes.
// The read-integrity check authenticates CONTENT (chunk hashes) but NOT resource bounds. A
// self-consistent manifest (digest recomputed) with a huge size aborts/panics the daemon,
// which serves every tenant on the node. Content trust != resource safety.
// ============================================================================
#[test]
fn materialize_trusts_size_field_for_allocation() {
    let d = tmp();
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    let seed = d.path().join("seed");
    write(&seed.join("x"), b"pwned");
    let sd = publish(&seed, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let sm = Manifest::from_bytes(&s.get_manifest(&sd).unwrap()).unwrap();
    let cid = sm.files[0].chunks[0].clone();

    let mut m = sm.clone();
    m.files = vec![FileEntry {
        path: "big.txt".into(),
        mode: 0o100644,
        size: u64::MAX, // <-- 5-byte content, but claims u64::MAX
        chunks: vec![cid],
        symlink: None,
        hardlink: None,
    }];
    // recompute digest so the integrity check PASSES — this is a *self-consistent* manifest.
    let ld = m.logical_digest();
    s.put_manifest(&ld, &m.to_bytes()).unwrap();

    let out = d.path().join("o");
    // silence the default panic hook noise for the expected capacity-overflow panic
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let caught =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| materialize(&s, &ld, &out)));
    std::panic::set_hook(prev);
    let clean_err = matches!(&caught, Ok(Err(_)));
    println!(
        "[P7] materialize with size=u64::MAX -> panicked={} clean_err={}",
        caught.is_err(),
        clean_err
    );
    // FIXED: size is validated against actual chunk bytes -> clean Err, NOT a capacity panic.
    assert!(caught.is_ok(), "must not panic on a bogus size field");
    assert!(clean_err, "bogus size field is now a clean error");
}

// ============================================================================
// PROBE 8 — BUG (fidelity, construct-level): paths & symlink targets are carried as UTF-8
// `String` via `to_string_lossy()`. On Linux/xfs (where the daemon runs) filenames are
// arbitrary bytes; two DISTINCT non-UTF8 names can map to the SAME lossy string and collide,
// silently losing a file. macOS/APFS rejects such names at the syscall, so this is shown at
// the manifest layer: two entries that a lossy walk would produce collapse to one on write.
// ============================================================================
#[test]
fn non_utf8_paths_collide_and_lose_files() {
    let d = tmp();
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    let seed = d.path().join("seed");
    write(&seed.join("x"), b"data-A");
    let sd = publish(&seed, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let sm = Manifest::from_bytes(&s.get_manifest(&sd).unwrap()).unwrap();
    let cid = sm.files[0].chunks[0].clone();

    // Two distinct byte-names on Linux — e.g. b"file\xFF" and b"file\xFE" — BOTH lossy-decode
    // to "file\u{FFFD}". A real walk_entries() would emit two FileEntry with the SAME path.
    let collided = "file\u{FFFD}".to_string();
    let mut m = sm.clone();
    m.files = vec![
        FileEntry {
            path: collided.clone(),
            mode: 0o100644,
            size: 6,
            chunks: vec![cid.clone()],
            symlink: None,
            hardlink: None,
        },
        FileEntry {
            path: collided.clone(),
            mode: 0o100644,
            size: 6,
            chunks: vec![cid],
            symlink: None,
            hardlink: None,
        },
    ];
    let ld = m.logical_digest();
    s.put_manifest(&ld, &m.to_bytes()).unwrap();
    let out = d.path().join("o");
    let r = materialize(&s, &ld, &out);
    let n_files = fs::read_dir(&out).map(|it| it.count()).unwrap_or(0);
    println!(
        "[P8] two distinct non-UTF8 source names -> lossy path {:?} -> materialize: {:?}, {} file(s)",
        collided,
        r.as_ref().err().map(|e| e.to_string()),
        n_files
    );
    // WAS: the two entries collapsed to one file on disk (the second silently overwrote the first) and
    // this asserted `n_files == 1` — i.e. it PINNED the data loss. materialize now REJECTS a manifest that
    // lists the same path twice, which is the fail-fast this repo prefers: a duplicate path can only mean
    // a lossy-decoded collision or a tampered manifest, and either way writing one of the two and calling
    // it success is the worst available outcome. Nothing is left behind on the failure path.
    assert!(
        r.is_err(),
        "a manifest listing the same path twice must be refused, not silently deduplicated"
    );
    assert_eq!(n_files, 0, "a refused materialize leaves nothing behind");
}

// ============================================================================
// PROBE 1-FIX — streaming publish is correct on large + sparse files (bounded memory).
// (The RSS number is re-measured on Linux/EC2; here we assert correctness: a large file and a
// 256 MiB sparse file both publish and round-trip byte-identically without buffering the whole
// file — the streaming path processes CHUNK-at-a-time.)
// ============================================================================
#[test]
fn streaming_large_and_sparse_roundtrip() {
    let d = tmp();
    let tree = d.path().join("t");
    // a 40 MiB incompressible file (spans 160 chunks; streamed, never whole-file-read)
    let mut big = Vec::with_capacity(40 * 1024 * 1024);
    for i in 0..(40 * 1024 * 1024 / CHUNK) as u64 {
        big.extend(prng_chunk(7_000_000 + i));
    }
    write(&tree.join("big.bin"), &big);
    // a 256 MiB SPARSE file: ~0 allocated blocks, huge apparent size (the OOM-DoS shape)
    {
        use std::io::{Seek, SeekFrom, Write};
        let f = fs::File::create(tree.join("sparse.bin")).unwrap();
        let mut f = f;
        f.seek(SeekFrom::Start(256 * 1024 * 1024 - 1)).unwrap();
        f.write_all(&[0u8]).unwrap();
    }
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let st = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
    println!(
        "[P1-fix] streamed {} chunks, {} blocks, {:.0} MB raw",
        st.total_chunks, st.blocks, st.raw_mb
    );
    let out = d.path().join("o");
    materialize(&s, &st.manifest, &out).unwrap();
    assert_eq!(
        fs::read(out.join("big.bin")).unwrap(),
        big,
        "large file round-trips"
    );
    assert_eq!(
        fs::metadata(out.join("sparse.bin")).unwrap().len(),
        256 * 1024 * 1024,
        "sparse size preserved"
    );
}

// ============================================================================
// PROBE 3-FIX — referenced-only index must still materialize INHERITED chunks across
// generations (an unchanged file's chunks come from a PRIOR generation's blocks via
// `known`). Confirms the fix didn't break multi-generation cold materialize.
// ============================================================================
#[test]
fn referenced_index_inheritance_materializes() {
    let d = tmp();
    let tree = d.path().join("t");
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    let body = |seeds: std::ops::Range<u64>| -> Vec<u8> {
        let mut v = Vec::new();
        for i in seeds {
            v.extend(prng_chunk(i));
        }
        v
    };
    // gen 0
    write(&tree.join("keep.bin"), &body(50_000..50_003)); // 3 chunks, will be UNCHANGED
    write(&tree.join("churn.bin"), &body(60_000..60_003));
    let g0 = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
    let known0 = load_index(&s, &g0.manifest);
    // gen 1: keep.bin unchanged (chunks inherited from gen-0 blocks), churn.bin replaced
    write(&tree.join("churn.bin"), &body(70_000..70_003));
    let g1 = publish(&tree, &s, &known0, None).unwrap();
    let m1 = Manifest::from_bytes(&s.get_manifest(&g1.manifest).unwrap()).unwrap();
    let keep = m1.files.iter().find(|f| f.path == "keep.bin").unwrap();
    let inherited = keep.chunks.iter().all(|c| known0.contains_key(c));
    let out = d.path().join("o");
    materialize(&s, &g1.manifest, &out).unwrap();
    println!(
        "[P3-fix] keep.bin chunks inherited from gen0={} index_len={} (referenced-only)",
        inherited,
        m1.chunks.len()
    );
    assert!(
        inherited,
        "unchanged file inherits its chunks from the prior generation"
    );
    assert_eq!(
        fs::read(out.join("keep.bin")).unwrap(),
        fs::read(tree.join("keep.bin")).unwrap(),
        "inherited chunks materialize byte-identically from the OLD block"
    );
    // 6 referenced (3 inherited + 3 new), NOT cumulative 9
    assert_eq!(
        m1.chunks.len(),
        6,
        "index = referenced only, not cumulative"
    );
}

// ============================================================================
// PROBE 9 — FINE bundle: confirm the genuinely-safe edges so the report can say so.
//   (a) empty file round-trips; (b) files at exact chunk boundaries (256K, 512K) round-trip;
//   (c) symlink loops are preserved, not followed (no hang); (d) a manifest referencing a
//   MISSING block yields a clean Err (no panic); (e) empty-tree materialize (minor: `out`
//   is not created at all).
// ============================================================================
#[test]
fn fine_edges_confirmed() {
    // (a) empty file
    {
        let d = tmp();
        let tree = d.path().join("t");
        write(&tree.join("empty"), b"");
        write(&tree.join("nonempty"), b"z");
        let s = LocalBlobStore::new(d.path().join("s")).unwrap();
        let dig = publish(&tree, &s, &ChunkIndex::new(), None)
            .unwrap()
            .manifest;
        let out = d.path().join("o");
        materialize(&s, &dig, &out).unwrap();
        assert!(out.join("empty").exists() && fs::read(out.join("empty")).unwrap().is_empty());
        println!("[P9a] empty file round-trips: OK");
    }
    // (b) exact chunk-boundary sizes
    {
        let d = tmp();
        let tree = d.path().join("t");
        write(&tree.join("one_chunk"), &vec![3u8; CHUNK]); // exactly 256 KiB
        write(&tree.join("two_chunks"), &vec![4u8; 2 * CHUNK]); // exactly 512 KiB
        write(&tree.join("chunk_plus_1"), &vec![5u8; CHUNK + 1]);
        let s = LocalBlobStore::new(d.path().join("s")).unwrap();
        let dig = publish(&tree, &s, &ChunkIndex::new(), None)
            .unwrap()
            .manifest;
        let out = d.path().join("o");
        materialize(&s, &dig, &out).unwrap();
        for (name, len) in [
            ("one_chunk", CHUNK),
            ("two_chunks", 2 * CHUNK),
            ("chunk_plus_1", CHUNK + 1),
        ] {
            assert_eq!(fs::read(out.join(name)).unwrap().len(), len, "{name}");
        }
        println!("[P9b] exact chunk-boundary files (256K/512K/+1) round-trip: OK");
    }
    // (c) symlink loop preserved, not followed
    {
        let d = tmp();
        let tree = d.path().join("t");
        fs::create_dir_all(&tree).unwrap();
        symlink("loop_b", tree.join("loop_a")).unwrap();
        symlink("loop_a", tree.join("loop_b")).unwrap();
        let s = LocalBlobStore::new(d.path().join("s")).unwrap();
        let dig = publish(&tree, &s, &ChunkIndex::new(), None)
            .unwrap()
            .manifest; // must not hang
        let out = d.path().join("o");
        materialize(&s, &dig, &out).unwrap();
        assert_eq!(
            fs::read_link(out.join("loop_a")).unwrap().to_str().unwrap(),
            "loop_b"
        );
        println!("[P9c] symlink loop preserved (not followed): OK");
    }
    // (d) missing block -> clean Err, not panic
    {
        let d = tmp();
        let tree = d.path().join("t");
        write(&tree.join("f.bin"), &vec![7u8; 100_000]);
        let store = d.path().join("s");
        let s = LocalBlobStore::new(&store).unwrap();
        let dig = publish(&tree, &s, &ChunkIndex::new(), None)
            .unwrap()
            .manifest;
        for e in fs::read_dir(store.join("blocks")).unwrap() {
            fs::remove_file(e.unwrap().path()).unwrap(); // delete the block, keep the manifest
        }
        let out = d.path().join("o");
        let r = materialize(&s, &dig, &out);
        assert!(r.is_err(), "missing block must be a clean error");
        println!("[P9d] manifest referencing a missing block -> clean Err: OK");
    }
    // (e) empty tree: `out` is NOT created (minor inconsistency for a bind-mount target)
    {
        let d = tmp();
        let tree = d.path().join("t");
        fs::create_dir_all(&tree).unwrap(); // no files at all
        let s = LocalBlobStore::new(d.path().join("s")).unwrap();
        let dig = publish(&tree, &s, &ChunkIndex::new(), None)
            .unwrap()
            .manifest;
        let out = d.path().join("o");
        materialize(&s, &dig, &out).unwrap();
        println!(
            "[P9e] empty-tree materialize: out dir created? {} (FIXED: now true)",
            out.exists()
        );
        assert!(
            out.exists(),
            "FIXED: empty manifest still creates the (empty) out dir"
        );
    }
}
