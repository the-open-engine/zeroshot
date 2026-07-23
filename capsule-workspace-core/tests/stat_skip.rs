//! O10 — the publish re-hash skip. `publish_pipelined` may reuse the PARENT MANIFEST's chunk list for a
//! file it can prove is quiescent, instead of re-reading and re-sha256ing it.
//!
//! A wrong skip is SILENT NON-DURABILITY (the manifest keeps a stale chunk list, so a later resume reverts
//! the agent's work). So the load-bearing test here is an ORACLE test: for a sequence of realistic
//! mutations, the manifest produced WITH the skip must be byte-identical to one produced by a full
//! re-hash. The rest pin each individual way a change could hide.

use capsule_workspace_core::cas::*;
use capsule_workspace_core::daemon::{publish_pipelined, PrevPublish};
use capsule_workspace_core::manifest::Manifest;
use capsule_workspace_core::stat_cache::StatCache;
use std::fs;
use std::path::Path;

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}
fn write(p: &Path, b: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b).unwrap();
}
fn blob(seed: u64, len: usize) -> Vec<u8> {
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

/// Simulate elapsed time between cycles WITHOUT sleeping. The racy guard requires a file to have been
/// quiescent for `SETTLE_NS` (2s) before the previous scan began, so back-to-back publishes in a test would
/// never skip anything. Advancing the recorded scan timestamp is exactly equivalent to that previous
/// publish having run 10s after the tree settled, which is what a real 30s publish interval looks like.
fn settled(mut c: StatCache) -> StatCache {
    c.scan_started_ns += 10_000_000_000;
    c
}

/// One daemon-like cycle carrying the parent manifest + stat cache forward.
struct Cycle {
    store: LocalBlobStore,
    parent: Option<(Manifest, String)>,
    cache: StatCache,
}
impl Cycle {
    fn new(dir: &Path) -> Self {
        Self {
            store: LocalBlobStore::new(dir).unwrap(),
            parent: None,
            cache: StatCache::default(),
        }
    }
    /// Publish `tree` with the skip enabled; returns (digest, files_skipped).
    fn publish(&mut self, tree: &Path) -> (String, usize) {
        let (known, pdigest) = match &self.parent {
            Some((m, d)) => (m.chunks.clone(), Some(d.clone())),
            None => (ChunkIndex::new(), None),
        };
        let prev = match &self.parent {
            Some((m, d)) => PrevPublish::new(m, d, &self.cache),
            None => None,
        };
        let st = publish_pipelined(tree, &self.store, &known, pdigest, 4, 8, prev).unwrap();
        let m = Manifest::from_bytes(&self.store.get_manifest(&st.manifest).unwrap()).unwrap();
        self.parent = Some((m, st.manifest.clone()));
        self.cache = settled(st.stat_cache);
        (st.manifest, st.skipped_files)
    }
}

/// Full re-hash of `tree` into a throwaway store — the oracle the skip must agree with.
fn oracle(tree: &Path, scratch: &Path) -> String {
    let s = LocalBlobStore::new(scratch).unwrap();
    publish_pipelined(tree, &s, &ChunkIndex::new(), None, 4, 8, None)
        .unwrap()
        .manifest
}

// THE property test. After each mutation, the skip-enabled manifest must equal a full re-hash of the same
// tree. Any divergence is data loss (or a spurious change) — this is the test that would catch it.
#[test]
fn skip_always_agrees_with_a_full_rehash() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("a.bin"), &blob(1, 300_000));
    write(&tree.join("dir/b.bin"), &blob(2, 300_000));
    write(&tree.join("c.txt"), b"hello");
    write(&tree.join("keep/never_touched.bin"), &blob(3, 300_000));

    let mut cyc = Cycle::new(&d.path().join("store"));
    let (dig0, _) = cyc.publish(&tree);
    assert_eq!(dig0, oracle(&tree, &d.path().join("o0")), "initial publish");

    // A sequence of realistic mutations. `never_touched.bin` stays quiescent throughout, so it should be
    // skipped from cycle 2 onward while every other case still lands correctly.
    let steps: Vec<(&str, Box<dyn Fn(&Path)>)> = vec![
        ("no-op (idle cycle)", Box::new(|_: &Path| {})),
        (
            "modify a file, SAME size",
            Box::new(|t: &Path| write(&t.join("a.bin"), &blob(99, 300_000))),
        ),
        (
            "modify a file, different size",
            Box::new(|t: &Path| write(&t.join("dir/b.bin"), &blob(7, 150_000))),
        ),
        (
            "add a new file",
            Box::new(|t: &Path| write(&t.join("new.bin"), &blob(11, 90_000))),
        ),
        (
            "delete a file",
            Box::new(|t: &Path| fs::remove_file(t.join("c.txt")).unwrap()),
        ),
        (
            "truncate to empty",
            Box::new(|t: &Path| fs::write(t.join("new.bin"), b"").unwrap()),
        ),
        (
            "replace by rename (same bytes, new inode)",
            Box::new(|t: &Path| {
                write(&t.join(".stage"), &blob(99, 300_000));
                fs::rename(t.join(".stage"), t.join("a.bin")).unwrap();
            }),
        ),
        (
            "chmod only",
            Box::new(|t: &Path| {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(t.join("a.bin"), fs::Permissions::from_mode(0o600)).unwrap();
            }),
        ),
        ("second idle cycle", Box::new(|_: &Path| {})),
    ];

    for (i, (label, apply)) in steps.iter().enumerate() {
        apply(&tree);
        let want = oracle(&tree, &d.path().join(format!("o{}", i + 1)));
        let (got, skipped) = cyc.publish(&tree);
        assert_eq!(
            got, want,
            "step {i} ({label}): skip diverged from a full re-hash"
        );
        let _ = skipped;
    }

    // And the skip must actually be doing something by the end (otherwise this test proves nothing).
    let (_, skipped) = cyc.publish(&tree);
    assert!(
        skipped > 0,
        "expected a quiescent tree to skip re-hashing, got {skipped}"
    );
}

// GC PROTECTION. `publish_cycle` refreshes the GC reuse-clock by iterating the NEW manifest's chunk
// INDEX. Skipped files never reach the packer, so if their chunks were missing from that index their
// blocks would go un-touched, age out, and be collected out from under a live HEAD — silent corruption.
// Assert the index (and therefore the touched block set) is IDENTICAL to a full re-hash's.
#[test]
fn skipped_files_chunks_stay_in_the_manifest_index_so_gc_still_touches_them() {
    use std::collections::BTreeSet;
    let d = tmp();
    let tree = d.path().join("t");
    for i in 0..6 {
        write(&tree.join(format!("f{i}.bin")), &blob(i, 300_000));
    }
    let mut cyc = Cycle::new(&d.path().join("store"));
    cyc.publish(&tree);
    let (dig, skipped) = cyc.publish(&tree); // every file skipped
    assert_eq!(skipped, 6, "all quiescent files skipped");

    let s = LocalBlobStore::new(d.path().join("store")).unwrap();
    let m = Manifest::from_bytes(&s.get_manifest(&dig).unwrap()).unwrap();
    // every chunk of every file must be resolvable in the index...
    for f in m
        .files
        .iter()
        .filter(|f| f.symlink.is_none() && f.hardlink.is_none())
    {
        for c in &f.chunks {
            assert!(
                m.chunks.contains_key(c),
                "chunk of skipped file {} missing from the manifest index → its block would never be \
                 touched and could be GC'd under a live HEAD",
                f.path
            );
        }
    }
    // ...and the touched BLOCK set must equal what a full re-hash would have produced.
    let full_dig = oracle(&tree, &d.path().join("o"));
    let fs_store = LocalBlobStore::new(d.path().join("o")).unwrap();
    let full = Manifest::from_bytes(&fs_store.get_manifest(&full_dig).unwrap()).unwrap();
    let blocks = |mm: &Manifest| -> BTreeSet<String> {
        mm.chunks.values().map(|l| l.block.clone()).collect()
    };
    assert_eq!(
        blocks(&m).len(),
        blocks(&full).len(),
        "skip must touch the same number of blocks as a full re-hash"
    );
    assert_eq!(
        m.chunks.len(),
        full.chunks.len(),
        "skip must reference the same chunk set as a full re-hash"
    );
}

// GUARD-PINNING TEST — deliberately does NOT use `settled()`.
//
// Every other test here advances the recorded scan timestamp to simulate elapsed time, which moves the
// safety-critical field in the PERMISSIVE direction. That made the whole file blind to the guard itself: an
// independent review reverted `may_skip` to its original (unsafe) form and all of these tests still passed.
// This one publishes back-to-back with the REAL timestamps, so a file written microseconds before the first
// scan must NOT be skippable — it has not been quiescent for SETTLE_NS. Weakening or removing the settle
// margin makes this test fail, which is the entire point of it.
#[test]
fn freshly_written_files_are_not_skippable_without_elapsed_time() {
    let d = tmp();
    let tree = d.path().join("t");
    for i in 0..4 {
        write(&tree.join(format!("f{i}.bin")), &blob(i, 200_000));
    }
    let s = LocalBlobStore::new(d.path().join("store")).unwrap();
    let st1 = publish_pipelined(&tree, &s, &ChunkIndex::new(), None, 4, 8, None).unwrap();
    let m1 = Manifest::from_bytes(&s.get_manifest(&st1.manifest).unwrap()).unwrap();

    // Real cache, real timestamps, immediately afterwards.
    let prev = PrevPublish::new(&m1, &st1.manifest, &st1.stat_cache).unwrap();
    let st2 = publish_pipelined(
        &tree,
        &s,
        &m1.chunks,
        Some(st1.manifest.clone()),
        4,
        8,
        Some(prev),
    )
    .unwrap();
    assert_eq!(
        st2.skipped_files, 0,
        "a file written just before the previous scan is inside the settle margin and MUST be re-hashed"
    );
    assert_eq!(st2.manifest, st1.manifest, "and the content is unchanged");
}

// An idle republish skips every regular file and still yields the identical digest.
#[test]
fn idle_republish_skips_everything_and_is_idempotent() {
    let d = tmp();
    let tree = d.path().join("t");
    for i in 0..8 {
        write(&tree.join(format!("f{i}.bin")), &blob(i, 200_000));
    }
    let mut cyc = Cycle::new(&d.path().join("store"));
    let (d1, s1) = cyc.publish(&tree);
    assert_eq!(s1, 0, "first publish has no cache → nothing skipped");
    let (d2, s2) = cyc.publish(&tree);
    assert_eq!(d1, d2, "idle republish is idempotent (O1)");
    assert_eq!(s2, 8, "every quiescent file skipped the re-hash");
}

// A cache is only usable against the manifest it was produced for. A publish that stored its manifest but
// then LOST THE FENCE leaves HEAD behind; pairing that cache with the older parent would resurrect stale
// chunk lists. `PrevPublish::new` must refuse it.
#[test]
fn cache_is_refused_against_a_different_parent_manifest() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("a.bin"), &blob(1, 200_000));
    let s = LocalBlobStore::new(d.path().join("store")).unwrap();
    let st1 = publish_pipelined(&tree, &s, &ChunkIndex::new(), None, 4, 8, None).unwrap();
    let m1 = Manifest::from_bytes(&s.get_manifest(&st1.manifest).unwrap()).unwrap();

    // a cache from a DIFFERENT generation
    write(&tree.join("a.bin"), &blob(2, 200_000));
    let st2 = publish_pipelined(
        &tree,
        &s,
        &m1.chunks,
        Some(st1.manifest.clone()),
        4,
        8,
        None,
    )
    .unwrap();
    assert_ne!(st1.manifest, st2.manifest);

    assert!(
        PrevPublish::new(&m1, &st1.manifest, &st2.stat_cache).is_none(),
        "gen2's cache must not pair with gen1's manifest"
    );
    assert!(
        PrevPublish::new(&m1, &st1.manifest, &st1.stat_cache).is_some(),
        "matching pair is accepted"
    );
    assert!(
        PrevPublish::new(&m1, "", &st1.stat_cache).is_none(),
        "empty digest never pairs"
    );
}

// The racy window: a file whose mtime is not strictly older than the previous scan must be re-hashed even
// though its fingerprint matches. Simulated by rewinding the cache's scan timestamp to before the file's
// mtime — exactly the state a same-tick write produces.
#[test]
fn racy_file_is_rehashed_not_skipped() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("a.bin"), &blob(1, 200_000));
    let s = LocalBlobStore::new(d.path().join("store")).unwrap();
    let st1 = publish_pipelined(&tree, &s, &ChunkIndex::new(), None, 4, 8, None).unwrap();
    let m1 = Manifest::from_bytes(&s.get_manifest(&st1.manifest).unwrap()).unwrap();

    // Sanity: with the real cache the file IS skippable.
    let mut cache = settled(st1.stat_cache);
    let prev = PrevPublish::new(&m1, &st1.manifest, &cache).unwrap();
    let ok = publish_pipelined(
        &tree,
        &s,
        &m1.chunks,
        Some(st1.manifest.clone()),
        4,
        8,
        Some(prev),
    )
    .unwrap();
    assert_eq!(ok.skipped_files, 1, "quiescent file is skippable");

    // Now make it racy: the scan appears to have started BEFORE the file's mtime.
    cache.scan_started_ns = 1;
    let prev = PrevPublish::new(&m1, &st1.manifest, &cache).unwrap();
    let racy =
        publish_pipelined(&tree, &s, &m1.chunks, Some(st1.manifest), 4, 8, Some(prev)).unwrap();
    assert_eq!(
        racy.skipped_files, 0,
        "a file not provably quiescent before the previous scan must be re-hashed"
    );
}

// A reused chunk must already be durable. If the parent's chunks are absent from `known`, the file is
// re-hashed rather than emitting a manifest that references something we never uploaded.
#[test]
fn does_not_skip_when_parent_chunks_are_not_in_known() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("a.bin"), &blob(1, 200_000));
    let s = LocalBlobStore::new(d.path().join("store")).unwrap();
    let st1 = publish_pipelined(&tree, &s, &ChunkIndex::new(), None, 4, 8, None).unwrap();
    let m1 = Manifest::from_bytes(&s.get_manifest(&st1.manifest).unwrap()).unwrap();

    // EMPTY `known` — nothing is known-durable, so nothing may be reused.
    let cache = settled(st1.stat_cache);
    let prev = PrevPublish::new(&m1, &st1.manifest, &cache).unwrap();
    let st2 = publish_pipelined(
        &tree,
        &s,
        &ChunkIndex::new(),
        Some(st1.manifest.clone()),
        4,
        8,
        Some(prev),
    )
    .unwrap();
    assert_eq!(
        st2.skipped_files, 0,
        "chunks absent from `known` must not be reused"
    );
    assert_eq!(st2.manifest, st1.manifest, "still the same content");
}

// Symlinks and hardlinks are never skipped (they carry no chunks and are rebuilt from the walk each time),
// and their presence doesn't disturb the skip of neighbouring regular files.
#[test]
fn links_are_unaffected_by_the_skip() {
    use std::os::unix::fs::symlink;
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("canon.bin"), &blob(5, 200_000));
    fs::hard_link(tree.join("canon.bin"), tree.join("hl.bin")).unwrap();
    symlink("canon.bin", tree.join("ln")).unwrap();
    write(&tree.join("plain.bin"), &blob(6, 200_000));

    let mut cyc = Cycle::new(&d.path().join("store"));
    let (d1, _) = cyc.publish(&tree);
    let (d2, skipped) = cyc.publish(&tree);
    assert_eq!(d1, d2, "idle republish with links is idempotent");
    // canon.bin + plain.bin are the only regulars; hl/ln are link entries.
    assert_eq!(skipped, 2, "both regular files skipped; links not counted");
    assert_eq!(
        d2,
        oracle(&tree, &d.path().join("o")),
        "matches a full re-hash"
    );
}
