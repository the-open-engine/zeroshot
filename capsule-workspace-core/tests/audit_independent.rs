//! INDEPENDENT AUDIT tests (not authored by the engineer under review).
//! Purpose: attack the claims the author's own suite does NOT cover.

use anyhow::Result;
use capsule_workspace_core::cas::*;
use capsule_workspace_core::daemon::{materialize, publish, publish_pipelined};
use capsule_workspace_core::gc;
use capsule_workspace_core::manifest::{FileEntry, Manifest};
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
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
fn load_index(s: &LocalBlobStore, dig: &str) -> ChunkIndex {
    Manifest::from_bytes(&s.get_manifest(dig).unwrap())
        .unwrap()
        .chunks
}

// ===========================================================================
// AUDIT-E6: my OWN tree (nested dirs, empty file, exact chunk boundaries, cross-dir
// dups, symlink, AND hardlinks — which the author's e6 test omits) must give the SAME
// logical digest for streaming publish and pipelined publish at every worker count.
// ===========================================================================
#[test]
fn audit_e6_pipelined_equivalence_with_hardlinks() {
    let d = tmp();
    let tree = d.path().join("t");
    // varied bodies: multi-chunk, exact-boundary, tiny, empty
    write(&tree.join("a/one_chunk.bin"), &vec![7u8; CHUNK]);
    write(&tree.join("a/b/two_chunk.bin"), &vec![8u8; 2 * CHUNK]);
    write(&tree.join("a/b/c/odd.bin"), &prng(11));
    write(&tree.join("empty"), b"");
    for i in 0..30u64 {
        let mut body = Vec::new();
        for j in 0..(1 + i % 3) {
            body.extend(prng(1000 + i * 7 + j));
        }
        write(&tree.join(format!("d{}/f{}.bin", i % 4, i)), &body);
    }
    // cross-dir duplicate content -> must dedup identically in both paths
    write(&tree.join("dupdir1/x.bin"), &prng(424242));
    write(&tree.join("dupdir2/x.bin"), &prng(424242));
    std::os::unix::fs::symlink("a/one_chunk.bin", tree.join("sym")).unwrap();
    // hardlinks: 3 paths, 1 inode (canonical must be the lexicographically-first path)
    write(&tree.join("hl/zzz_created_first.bin"), &vec![5u8; 150_000]);
    fs::hard_link(
        tree.join("hl/zzz_created_first.bin"),
        tree.join("hl/aaa_lexfirst.bin"),
    )
    .unwrap();
    fs::hard_link(
        tree.join("hl/zzz_created_first.bin"),
        tree.join("hl/mmm.bin"),
    )
    .unwrap();

    let s0 = LocalBlobStore::new(d.path().join("s0")).unwrap();
    let base = publish(&tree, &s0, &ChunkIndex::new(), None).unwrap();

    for w in [1usize, 2, 3, 4, 8, 16] {
        let s = LocalBlobStore::new(d.path().join(format!("sp{}", w))).unwrap();
        let p = publish_pipelined(&tree, &s, &ChunkIndex::new(), None, w, 8, None).unwrap();
        assert_eq!(base.manifest, p.manifest, "digest mismatch at w={w}");
        assert_eq!(base.new_chunks, p.new_chunks, "dedup mismatch at w={w}");
        assert_eq!(base.total_chunks, p.total_chunks, "total mismatch at w={w}");
        // round-trip and check the hardlink relationship survived the parallel path
        let out = d.path().join(format!("o{}", w));
        materialize(&s, &p.manifest, &out).unwrap();
        let a = fs::metadata(out.join("hl/aaa_lexfirst.bin")).unwrap();
        let z = fs::metadata(out.join("hl/zzz_created_first.bin")).unwrap();
        let m = fs::metadata(out.join("hl/mmm.bin")).unwrap();
        let inodes: HashSet<u64> = [a.ino(), z.ino(), m.ino()].into_iter().collect();
        assert_eq!(inodes.len(), 1, "hardlinks collapsed to 1 inode at w={w}");
    }

    // canonical (the entry that carries chunks) must be the LEX-FIRST path, not created-first
    let m = Manifest::from_bytes(&s0.get_manifest(&base.manifest).unwrap()).unwrap();
    let canon = m
        .files
        .iter()
        .find(|f| f.path.starts_with("hl/") && f.hardlink.is_none() && f.symlink.is_none())
        .unwrap();
    assert_eq!(
        canon.path, "hl/aaa_lexfirst.bin",
        "canonical must be lexicographically-first path"
    );
    for hp in ["hl/mmm.bin", "hl/zzz_created_first.bin"] {
        let e = m.files.iter().find(|f| f.path == hp).unwrap();
        assert_eq!(
            e.hardlink.as_deref(),
            Some("hl/aaa_lexfirst.bin"),
            "{hp} must point at the canonical"
        );
        assert!(e.chunks.is_empty(), "{hp} must carry no chunks");
    }
    println!("[AUDIT-E6] streaming == pipelined at w=1..16, hardlink canonical = lex-first: OK");
}

// ===========================================================================
// AUDIT-E8-HOLE: the doc claims "the grace period is the ENTIRE safety mechanism."
// This is an OVERSTATEMENT. Grace only protects blocks physically WRITTEN during the
// in-flight publish (fresh mtime). A block a publish reuses via DEDUP is NOT rewritten,
// so it keeps its OLD mtime; if its source manifest has left the live set, grace does
// NOT protect it — only the mark set does. Demonstrate a dedup-reused old block being
// collected under a LARGE grace, corrupting the "in-flight" publish that references it.
// ===========================================================================
#[test]
fn audit_e8_grace_does_not_protect_dedup_reused_old_block() {
    let d = tmp();
    let tree = d.path().join("t");
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();

    // gen0: fileA=X, fileB=Y  -> block(s) with OLD mtime after we backdate
    write(&tree.join("A.bin"), &prng(1)); // X
    write(&tree.join("B.bin"), &prng(2)); // Y
    let g0 = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
    let known0 = load_index(&s, &g0.manifest);

    // backdate ALL gen0 blocks to the distant past (simulate "written long ago")
    for e in fs::read_dir(store.join("blocks")).unwrap() {
        let p = e.unwrap().path();
        let st = std::process::Command::new("touch")
            .args(["-t", "202001010000"])
            .arg(&p)
            .status()
            .unwrap();
        assert!(st.success());
    }

    // gen1 (the "in-flight" publish): A unchanged (DEDUPS X -> old block, NOT rewritten),
    // B replaced with Z (a fresh new block).
    write(&tree.join("B.bin"), &prng(3)); // Z
    let g1 = publish(&tree, &s, &known0, None).unwrap();
    let m1_path = store.join("manifests").join(&g1.manifest);
    let m1_bytes = fs::read(&m1_path).unwrap();

    // Which block holds X (dedup-reused by gen1)?
    let x_cid = Manifest::from_bytes(&m1_bytes)
        .unwrap()
        .files
        .iter()
        .find(|f| f.path == "A.bin")
        .unwrap()
        .chunks[0]
        .clone();
    let x_block = known0.get(&x_cid).unwrap().block.clone();
    assert!(
        store.join("blocks").join(&x_block).exists(),
        "X's block present pre-GC"
    );

    // Simulate the race window: gen1 manifest NOT yet committed, gen0 manifest superseded
    // and out of retention (removed). So NO live manifest references X's old block.
    fs::remove_file(&m1_path).unwrap();
    fs::remove_file(store.join("manifests").join(&g0.manifest)).unwrap();

    // GC with a LARGE grace (1h). Per the doc's framing this "protects the in-flight publish".
    let st = gc::collect(&store, &[], Duration::from_secs(3600)).unwrap();
    let x_block_survived = store.join("blocks").join(&x_block).exists();
    println!(
        "[AUDIT-E8-HOLE] grace=1h, deleted={} young_protected={} | dedup-reused old block survived? {}",
        st.blocks_deleted, st.blocks_young_orphans_protected, x_block_survived
    );
    assert!(
        !x_block_survived,
        "HOLE: grace=1h did NOT protect the dedup-reused old block (it was collected)"
    );

    // commit gen1 -> it now references a DELETED block -> materialize corrupts/fails.
    fs::write(&m1_path, &m1_bytes).unwrap();
    let out = d.path().join("o");
    let res = materialize(&s, &g1.manifest, &out);
    println!(
        "[AUDIT-E8-HOLE] after committing gen1 that referenced the collected block: materialize ok? {}",
        res.is_ok()
    );
    assert!(
        res.is_err(),
        "the 'in-flight' publish is corrupted despite grace=1h"
    );

    // PROOF that the MARK SET (not grace) is the real protection: redo with gen0 live.
    let d2 = tmp();
    let tree2 = d2.path().join("t");
    let store2 = d2.path().join("s");
    let s2 = LocalBlobStore::new(&store2).unwrap();
    write(&tree2.join("A.bin"), &prng(1));
    write(&tree2.join("B.bin"), &prng(2));
    let g0b = publish(&tree2, &s2, &ChunkIndex::new(), None).unwrap();
    let known0b = load_index(&s2, &g0b.manifest);
    for e in fs::read_dir(store2.join("blocks")).unwrap() {
        std::process::Command::new("touch")
            .args(["-t", "202001010000"])
            .arg(e.unwrap().path())
            .status()
            .unwrap();
    }
    write(&tree2.join("B.bin"), &prng(3));
    let g1b = publish(&tree2, &s2, &known0b, None).unwrap();
    // Keep gen0 in the live set. Its (old, backdated) block is protected ONLY by being MARKED
    // (grace can't help it — it is years old). gen1's fresh block is protected by grace=1h.
    // Both survive -> gen1 materializes: isolates the mark set as B0's protector.
    let st2 = gc::collect(&store2, &[g0b.manifest.clone()], Duration::from_secs(3600)).unwrap();
    let out2 = d2.path().join("o");
    let ok = materialize(&s2, &g1b.manifest, &out2).is_ok();
    println!(
        "[AUDIT-E8-HOLE] with gen0 kept LIVE (old block MARKED, not grace-protected), deleted={} -> gen1 materializes? {}",
        st2.blocks_deleted, ok
    );
    assert!(
        ok,
        "the mark set (live gen0) is what actually protects the dedup-reused OLD block"
    );
}

// ===========================================================================
// AUDIT-E8-DOWNSTREAM: strengthen the author's test-2. It asserts blocks_deleted>=1 at
// grace=0 but never proves the DOWNSTREAM corruption. Here: grace=0 collects the young
// in-flight blocks and the subsequently-committed manifest FAILS to materialize (real
// corruption), while grace=1h -> materialize succeeds. Closes the loop the author left open.
// ===========================================================================
#[test]
fn audit_e8_grace0_causes_real_downstream_corruption() {
    for (grace, expect_ok) in [(Duration::ZERO, false), (Duration::from_secs(3600), true)] {
        let d = tmp();
        let tree = d.path().join("t");
        let store = d.path().join("s");
        let s = LocalBlobStore::new(&store).unwrap();
        let mut body = Vec::new();
        for i in 0..8u64 {
            body.extend(prng(9000 + i));
        }
        write(&tree.join("f.bin"), &body);
        let st = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
        let mpath = store.join("manifests").join(&st.manifest);
        let mbytes = fs::read(&mpath).unwrap();
        // in-flight: blocks on disk (fresh), manifest not committed
        fs::remove_file(&mpath).unwrap();
        gc::collect(&store, &[], grace).unwrap();
        // commit and try to use it
        fs::write(&mpath, &mbytes).unwrap();
        let res = materialize(&s, &st.manifest, &d.path().join("o"));
        println!(
            "[AUDIT-E8-DOWN] grace={:?} -> materialize ok? {} (expected {})",
            grace,
            res.is_ok(),
            expect_ok
        );
        assert_eq!(
            res.is_ok(),
            expect_ok,
            "grace={:?}: downstream materialize outcome",
            grace
        );
    }
}

// ===========================================================================
// AUDIT-E11-ADVERSARIAL: a hand-crafted hardlink whose target does NOT exist, and a
// hardlink chain, must be a CLEAN error (no panic, no escape), never a silent success
// writing outside the tree.
// ===========================================================================
#[test]
fn audit_e11_hardlink_bad_target_is_clean_error_no_escape() {
    let d = tmp();
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    let seed = d.path().join("seed");
    write(&seed.join("x"), b"data");
    let sd = publish(&seed, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let cid = Manifest::from_bytes(&s.get_manifest(&sd).unwrap())
        .unwrap()
        .files[0]
        .chunks[0]
        .clone();

    // (1) hardlink -> absolute path escape attempt
    let mut m = Manifest::from_bytes(&s.get_manifest(&sd).unwrap()).unwrap();
    m.files = vec![FileEntry {
        path: "link".into(),
        mode: 0o100644,
        size: 0,
        chunks: vec![],
        symlink: None,
        hardlink: Some("/etc/passwd".into()),
    }];
    let ld = m.logical_digest();
    s.put_manifest(&ld, &m.to_bytes()).unwrap();
    let r = materialize(&s, &ld, &d.path().join("o1"));
    println!("[AUDIT-E11] hardlink->/etc/passwd: err? {}", r.is_err());
    assert!(r.is_err(), "absolute hardlink target must be refused");

    // (2) hardlink -> "../escape" traversal attempt
    let mut m2 = Manifest::from_bytes(&s.get_manifest(&sd).unwrap()).unwrap();
    m2.files = vec![
        FileEntry {
            path: "real".into(),
            mode: 0o100644,
            size: 4,
            chunks: vec![cid.clone()],
            symlink: None,
            hardlink: None,
        },
        FileEntry {
            path: "link".into(),
            mode: 0o100644,
            size: 0,
            chunks: vec![],
            symlink: None,
            hardlink: Some("../ESCAPE".into()),
        },
    ];
    let ld2 = m2.logical_digest();
    s.put_manifest(&ld2, &m2.to_bytes()).unwrap();
    let escape = d.path().join("ESCAPE");
    let r2 = materialize(&s, &ld2, &d.path().join("o2"));
    println!(
        "[AUDIT-E11] hardlink->../ESCAPE: err? {} escaped? {}",
        r2.is_err(),
        escape.exists()
    );
    assert!(r2.is_err(), "traversal hardlink target must be refused");
    assert!(!escape.exists(), "must not create a link outside the tree");

    // (3) hardlink -> nonexistent in-tree target: clean error, no panic
    let mut m3 = Manifest::from_bytes(&s.get_manifest(&sd).unwrap()).unwrap();
    m3.files = vec![FileEntry {
        path: "link".into(),
        mode: 0o100644,
        size: 0,
        chunks: vec![],
        symlink: None,
        hardlink: Some("does_not_exist".into()),
    }];
    let ld3 = m3.logical_digest();
    s.put_manifest(&ld3, &m3.to_bytes()).unwrap();
    let r3 = materialize(&s, &ld3, &d.path().join("o3"));
    println!("[AUDIT-E11] hardlink->missing: err? {}", r3.is_err());
    assert!(r3.is_err(), "missing hardlink target must be a clean error");
}

// ===========================================================================
// AUDIT-E11-OUTSIDE: an inode whose OTHER links live OUTSIDE the published tree (the
// pnpm-store case). In-tree nlink>1 but only one in-tree path -> must be chunked as a
// normal regular file (canonical), not emitted as a dangling hardlink entry.
// ===========================================================================
#[test]
fn audit_e11_link_target_outside_tree_is_regular() {
    let d = tmp();
    let tree = d.path().join("t");
    let outside = d.path().join("store_outside");
    write(&outside.join("blob"), &vec![9u8; 120_000]);
    fs::create_dir_all(&tree).unwrap();
    // single in-tree path, but hardlinked to a file OUTSIDE the tree -> nlink==2 in-tree
    fs::hard_link(outside.join("blob"), tree.join("node_modules_file.js")).unwrap();
    let nlink = fs::metadata(tree.join("node_modules_file.js"))
        .unwrap()
        .nlink();
    assert!(
        nlink >= 2,
        "sanity: nlink>1 even though only 1 in-tree path"
    );

    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let dig = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let m = Manifest::from_bytes(&s.get_manifest(&dig).unwrap()).unwrap();
    let e = m
        .files
        .iter()
        .find(|f| f.path == "node_modules_file.js")
        .unwrap();
    println!(
        "[AUDIT-E11-OUT] in-tree nlink={} -> emitted as hardlink? {} chunks={}",
        nlink,
        e.hardlink.is_some(),
        e.chunks.len()
    );
    assert!(
        e.hardlink.is_none(),
        "single in-tree path must be canonical regular"
    );
    assert!(!e.chunks.is_empty(), "must carry its own content");
    let out = d.path().join("o");
    materialize(&s, &dig, &out).unwrap();
    assert_eq!(
        fs::read(out.join("node_modules_file.js")).unwrap(),
        vec![9u8; 120_000],
        "materializes as a normal file with correct content"
    );
}

// ===========================================================================
// AUDIT-E6-DEADLOCK: minimal channel buffers (cap=1) with several worker counts must
// NOT deadlock (acyclic producer->compressors->packer pipeline). If it deadlocks, this
// test hangs -> caught by the harness/timeout. Also re-confirms digest equivalence.
// ===========================================================================
#[test]
fn audit_e6_min_buffers_no_deadlock() {
    let d = tmp();
    let tree = d.path().join("t");
    // many small distinct multi-chunk files -> lots of channel traffic through tiny buffers
    for i in 0..120u64 {
        let mut body = Vec::new();
        for j in 0..(1 + i % 5) {
            body.extend(prng(50_000 + i * 11 + j));
        }
        write(&tree.join(format!("d{}/f{}.bin", i % 7, i)), &body);
    }
    let s0 = LocalBlobStore::new(d.path().join("s0")).unwrap();
    let base = publish(&tree, &s0, &ChunkIndex::new(), None).unwrap();
    for (w, cap) in [(1usize, 1usize), (2, 1), (4, 1), (8, 1), (3, 2)] {
        let s = LocalBlobStore::new(d.path().join(format!("s_{}_{}", w, cap))).unwrap();
        let p = publish_pipelined(&tree, &s, &ChunkIndex::new(), None, w, cap, None).unwrap();
        assert_eq!(base.manifest, p.manifest, "digest at w={w} cap={cap}");
    }
    println!("[AUDIT-E6-DEADLOCK] cap=1 across w=1..8 completed, no deadlock: OK");
}

// A store whose put_block fails after `fail_after` successful writes (simulates an S3/disk
// error mid-publish). Everything else delegates to a LocalBlobStore.
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
    fn delete_block(&self, id: &BlockId) -> Result<bool> {
        self.inner.delete_block(id)
    }
    fn delete_manifest(&self, d: &str) -> Result<bool> {
        self.inner.delete_manifest(d)
    }
}

// ===========================================================================
// AUDIT-E6-ERRPATH: a put_block failure inside the packer thread must surface as a clean
// Err from publish_pipelined and must NOT hang (the classic pipeline failure mode where
// the producer blocks forever on a full channel after the consumer died).
// ===========================================================================
#[test]
fn audit_e6_put_block_failure_is_clean_err_no_hang() {
    let d = tmp();
    let tree = d.path().join("t");
    // enough new chunks to force multiple blocks so the packer flushes (and can fail) mid-stream
    for i in 0..400u64 {
        write(&tree.join(format!("f{}.bin", i)), &prng(80_000 + i));
    }
    let store = FlakyStore {
        inner: LocalBlobStore::new(d.path().join("s")).unwrap(),
        calls: AtomicUsize::new(0),
        fail_after: 0, // fail on the very first block flush
    };
    let r = publish_pipelined(&tree, &store, &ChunkIndex::new(), None, 4, 8, None);
    println!(
        "[AUDIT-E6-ERRPATH] put_block always-fails -> publish_pipelined err? {}",
        r.is_err()
    );
    assert!(
        r.is_err(),
        "a packer put_block failure must be a clean Err, not a hang"
    );
}

// ===========================================================================
// AUDIT-STORE: latent bug — put_MANIFEST (unlike put_BLOCK, which was fixed in probe 6)
// still uses a NON-unique temp path `<digest>.tmp`. Two writers committing the SAME
// content-addressed manifest digest concurrently (two lineages with an identical tree)
// collide on that temp: the loser's rename hits ENOENT and the publish spuriously fails.
// Content is never corrupted. This documents whether the probe-6 fix was also applied here.
// ===========================================================================
#[test]
fn audit_put_manifest_concurrent_same_digest_races() {
    use std::sync::Barrier;
    let mut spurious = 0usize;
    let rounds = 60;
    let bytes = Arc::new(vec![0x7Eu8; 512 * 1024]); // a ~0.5MB manifest payload
    let digest = hex_sha256(&bytes);
    for _ in 0..rounds {
        let rd = tmp();
        let store = Arc::new(LocalBlobStore::new(rd.path()).unwrap());
        let n = 8;
        let barrier = Arc::new(Barrier::new(n));
        let mut handles = Vec::new();
        for _ in 0..n {
            let (store, bytes, digest, barrier) = (
                store.clone(),
                bytes.clone(),
                digest.clone(),
                barrier.clone(),
            );
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store.put_manifest(&digest, &bytes)
            }));
        }
        for h in handles {
            if h.join().unwrap().is_err() {
                spurious += 1;
            }
        }
    }
    println!(
        "[AUDIT-STORE] concurrent put_manifest(same digest): {} spurious errors over {} rounds x 8 threads",
        spurious, rounds
    );
    // FIXED: put_manifest now uses a per-writer unique temp (same as put_block), so concurrent
    // commits of the same content-addressed digest no longer spuriously fail.
    assert_eq!(
        spurious, 0,
        "concurrent put_manifest(same digest) must not spuriously fail"
    );
}
