//! O8 — the daemon's reflink WARM RESUME lifecycle (`resume_via_reference`). The daemon keeps a
//! daemon-owned PRISTINE reference under `ref_root` and hands the agent a reflink-clone as the live
//! workspace, so resuming to the next generation fetches only the delta. These tests pin the lifecycle
//! (cold → warm-incremental → ref-reuse), the crash-residue sweep + fallback paths, path-injection
//! hygiene on the `current` file, and — the load-bearing one — that an agent mutating its live workspace
//! can NEVER corrupt the next resume, because the reflink source is the pristine reference, not the
//! mutated workspace. (The physical O(1)-reflink win is measured on real XFS in the EC2 batch.)

use capsule_workspace_core::cas::*;
use capsule_workspace_core::daemon::{
    lineage_ref_subdir, materialize, publish, resume_via_reference, ResumeKind,
};
use capsule_workspace_core::manifest::Manifest;
use std::fs;
use std::path::Path;

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
fn current(ref_root: &Path) -> Option<String> {
    fs::read_to_string(ref_root.join("current"))
        .ok()
        .map(|s| s.trim().to_string())
}
/// committed reference dirs (64-hex names) present under ref_root
fn ref_dirs(ref_root: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(ref_root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.len() == 64 && n.bytes().all(|b| b.is_ascii_hexdigit()))
        .collect();
    v.sort();
    v
}

// Cold first resume, then a warm incremental resume to the next generation: the second resume fetches
// only the delta, the workspace is byte-identical to gen2, and the old reference is GC'd.
#[test]
fn cold_then_warm_incremental_fetches_only_delta() {
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let tree = d.path().join("t");
    write(&tree.join("a.bin"), &prng(1));
    write(&tree.join("dir/b.bin"), &prng(2));
    write(&tree.join("c.bin"), &prng(3));
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;

    let ref_root = d.path().join("refroot");
    let ws1 = d.path().join("ws1");
    let r1 = resume_via_reference(&s, &g1, &ws1, &ref_root).unwrap();
    assert_eq!(r1.kind, ResumeKind::ColdFull, "first resume is cold");
    assert_eq!(current(&ref_root).as_deref(), Some(g1.as_str()));
    assert_eq!(ref_dirs(&ref_root), vec![g1.clone()], "one committed ref");
    // workspace == gen1
    assert_eq!(fs::read(ws1.join("a.bin")).unwrap(), prng(1));
    assert_eq!(fs::read(ws1.join("dir/b.bin")).unwrap(), prng(2));

    // gen2: change b.bin + add e.bin; a.bin + c.bin unchanged (dedup vs g1).
    write(&tree.join("dir/b.bin"), &prng(99));
    write(&tree.join("e.bin"), &prng(50));
    let g2 = publish(&tree, &s, &load_index(&s, &g1), Some(g1.clone()))
        .unwrap()
        .manifest;

    let ws2 = d.path().join("ws2");
    let r2 = resume_via_reference(&s, &g2, &ws2, &ref_root).unwrap();
    assert_eq!(
        r2.kind,
        ResumeKind::WarmIncremental,
        "second resume is warm"
    );
    // built the new reference by fetching fewer blocks than a full materialize of gen2 would.
    let full_blocks = materialize(&s, &g2, &d.path().join("full"))
        .unwrap()
        .blocks_fetched;
    assert!(
        r2.ref_blocks_fetched < full_blocks,
        "incremental ref fetched {} blocks < full {full_blocks}",
        r2.ref_blocks_fetched
    );
    // workspace == gen2, byte-for-byte.
    assert_eq!(fs::read(ws2.join("a.bin")).unwrap(), prng(1)); // reflinked-unchanged
    assert_eq!(fs::read(ws2.join("dir/b.bin")).unwrap(), prng(99)); // changed
    assert_eq!(fs::read(ws2.join("c.bin")).unwrap(), prng(3));
    assert_eq!(fs::read(ws2.join("e.bin")).unwrap(), prng(50)); // new
    // current advanced; old reference (dir + its sentinel) GC'd (only g2 remains).
    assert_eq!(current(&ref_root).as_deref(), Some(g2.as_str()));
    assert_eq!(ref_dirs(&ref_root), vec![g2.clone()], "old ref g1 GC'd");
    assert!(
        !ref_root.join(format!("{g1}.ok")).exists(),
        "old sentinel GC'd"
    );
    assert!(
        ref_root.join(format!("{g2}.ok")).exists(),
        "current sentinel kept"
    );
}

// THE safety test. The agent mutates its live workspace (in place, unpublished). The NEXT resume must
// still produce a correct tree — because the reflink source is the daemon-owned PRISTINE reference, never
// the agent's mutated workspace. If the design ever reflinked from the live workspace, the unchanged
// files below would carry the agent's garbage instead of the true content.
#[test]
fn pristine_reference_survives_agent_mutation() {
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let tree = d.path().join("t");
    write(&tree.join("keep.bin"), &prng(11)); // stays unchanged across gen1→gen2
    write(&tree.join("edit.bin"), &prng(12)); // will change in gen2
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;

    let ref_root = d.path().join("refroot");
    let ws = d.path().join("ws");
    resume_via_reference(&s, &g1, &ws, &ref_root).unwrap();

    // AGENT mutates its live workspace in place (never published): scribble garbage over keep.bin.
    fs::write(ws.join("keep.bin"), b"AGENT GARBAGE - must never resurface").unwrap();

    // gen2: edit.bin changes; keep.bin is genuinely unchanged (== gen1).
    write(&tree.join("edit.bin"), &prng(99));
    let g2 = publish(&tree, &s, &load_index(&s, &g1), Some(g1.clone()))
        .unwrap()
        .manifest;

    // resume gen2 into a FRESH workspace.
    let ws2 = d.path().join("ws2");
    resume_via_reference(&s, &g2, &ws2, &ref_root).unwrap();

    // keep.bin was reflinked from the PRISTINE reference → true gen1 bytes, NOT the agent's garbage.
    assert_eq!(
        fs::read(ws2.join("keep.bin")).unwrap(),
        prng(11),
        "unchanged file must carry pristine content, not the agent's in-place mutation"
    );
    assert_eq!(fs::read(ws2.join("edit.bin")).unwrap(), prng(99));
}

// Restart at the SAME HEAD with an empty workspace (pod rescheduled): the reference already exists, so
// the resume reuses it (0 fetch) and just re-clones the workspace.
#[test]
fn ref_reused_on_restart_same_head() {
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let tree = d.path().join("t");
    write(&tree.join("a.bin"), &prng(7));
    write(&tree.join("b.bin"), &prng(8));
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;

    let ref_root = d.path().join("refroot");
    let ws1 = d.path().join("ws1");
    resume_via_reference(&s, &g1, &ws1, &ref_root).unwrap();

    // pod restart: fresh empty workspace, same HEAD.
    let ws2 = d.path().join("ws2");
    let r = resume_via_reference(&s, &g1, &ws2, &ref_root).unwrap();
    assert_eq!(
        r.kind,
        ResumeKind::RefReused,
        "same HEAD → reference reused"
    );
    assert_eq!(r.ref_blocks_fetched, 0, "reuse fetches nothing");
    assert_eq!(fs::read(ws2.join("a.bin")).unwrap(), prng(7));
    assert_eq!(fs::read(ws2.join("b.bin")).unwrap(), prng(8));
}

// Crash residue: a leftover `.tmp-*` build dir and a stray `.current.tmp` are swept on the next resume;
// and an already-committed `<target>/` reference with a STALE `current` (crash after rename, before the
// current update) is reused and the current pointer is advanced.
#[test]
fn crash_residue_swept_and_committed_ref_reused() {
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let tree = d.path().join("t");
    write(&tree.join("a.bin"), &prng(21));
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;

    let ref_root = d.path().join("refroot");
    // Simulate a crash AFTER the sentinel but before `current`: a COMMITTED reference dir + its `.ok`
    // sentinel exist, `current` still points at a bogus prior, plus crash-residue staging entries.
    fs::create_dir_all(&ref_root).unwrap();
    materialize(&s, &g1, &ref_root.join(&g1)).unwrap(); // the committed ref body
    fs::write(ref_root.join(format!("{g1}.ok")), b"").unwrap(); // completeness sentinel
    fs::write(ref_root.join("current"), "deadbeef").unwrap(); // stale/bogus prior
    fs::create_dir_all(ref_root.join(".tmp-deadbeef")).unwrap(); // crash residue
    fs::write(ref_root.join(".tmp-deadbeef/junk"), b"x").unwrap();
    fs::write(ref_root.join(".current.tmp"), "garbage").unwrap();

    let ws = d.path().join("ws");
    let r = resume_via_reference(&s, &g1, &ws, &ref_root).unwrap();
    assert_eq!(
        r.kind,
        ResumeKind::RefReused,
        "committed (sentinel) ref reused"
    );
    assert_eq!(
        current(&ref_root).as_deref(),
        Some(g1.as_str()),
        "current advanced"
    );
    assert!(!ref_root.join(".tmp-deadbeef").exists(), ".tmp-* swept");
    assert!(
        !ref_root.join(".current.tmp").exists(),
        ".current.tmp swept"
    );
    assert_eq!(fs::read(ws.join("a.bin")).unwrap(), prng(21));
    assert_eq!(ref_dirs(&ref_root), vec![g1], "no stray ref dirs");
}

// The prior reference dir exists but its MANIFEST is gone from the store (e.g. GC'd): the incremental
// build errors, and the resume falls back to a full materialize — still correct.
#[test]
fn falls_back_to_full_when_prior_manifest_missing() {
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let tree = d.path().join("t");
    write(&tree.join("a.bin"), &prng(31));
    write(&tree.join("b.bin"), &prng(32));
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let ref_root = d.path().join("refroot");
    resume_via_reference(&s, &g1, &d.path().join("ws1"), &ref_root).unwrap();

    // gen2 (change a.bin), then delete g1's manifest from the store so the incremental diff can't load it.
    write(&tree.join("a.bin"), &prng(99));
    let g2 = publish(&tree, &s, &load_index(&s, &g1), Some(g1.clone()))
        .unwrap()
        .manifest;
    fs::remove_file(d.path().join("s").join("manifests").join(&g1)).unwrap();

    let ws2 = d.path().join("ws2");
    let r = resume_via_reference(&s, &g2, &ws2, &ref_root).unwrap();
    assert_eq!(
        r.kind,
        ResumeKind::ColdFull,
        "unusable prior → full rebuild"
    );
    assert_eq!(fs::read(ws2.join("a.bin")).unwrap(), prng(99));
    assert_eq!(fs::read(ws2.join("b.bin")).unwrap(), prng(32));
    assert_eq!(current(&ref_root).as_deref(), Some(g2.as_str()));
}

// A corrupt/non-hex `current` (here a path-traversal string) is ignored — treated as no prior (cold
// rebuild) — and nothing is created or removed outside `ref_root`.
#[test]
fn corrupt_current_is_ignored_no_path_escape() {
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let tree = d.path().join("t");
    write(&tree.join("a.bin"), &prng(41));
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let ref_root = d.path().join("refroot");
    resume_via_reference(&s, &g1, &d.path().join("ws1"), &ref_root).unwrap();

    // Poison `current` with a traversal string; publish gen2.
    fs::write(ref_root.join("current"), "../../../etc/PWNED\n").unwrap();
    write(&tree.join("a.bin"), &prng(99));
    let g2 = publish(&tree, &s, &load_index(&s, &g1), Some(g1.clone()))
        .unwrap()
        .manifest;

    let ws2 = d.path().join("ws2");
    let r = resume_via_reference(&s, &g2, &ws2, &ref_root).unwrap();
    assert_eq!(
        r.kind,
        ResumeKind::ColdFull,
        "non-hex current → no prior → cold"
    );
    assert_eq!(fs::read(ws2.join("a.bin")).unwrap(), prng(99));
    // no traversal target created anywhere near the ref root's parents.
    assert!(!d.path().join("etc/PWNED").exists());
    assert!(!ref_root.join("../../../etc/PWNED").exists());
    assert_eq!(current(&ref_root).as_deref(), Some(g2.as_str()));
}

// SAFETY (design-review finding #1): a present-but-INCOMPLETE `<target>/` with NO `.ok` sentinel (an
// externally-planted or crash-partial dir) must NOT be trusted/reflinked — it is removed and rebuilt,
// yielding correct bytes rather than the planted corruption.
#[test]
fn incomplete_reference_without_sentinel_is_rebuilt() {
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let tree = d.path().join("t");
    write(&tree.join("a.bin"), &prng(61));
    write(&tree.join("b.bin"), &prng(62));
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;

    // Plant a corrupt `<g1>/` (right name, WRONG bytes, NO sentinel) — as a bad backup/rsync/cp might.
    let ref_root = d.path().join("refroot");
    let planted = ref_root.join(&g1);
    fs::create_dir_all(&planted).unwrap();
    fs::write(planted.join("a.bin"), b"CORRUPT planted content").unwrap();
    // no `<g1>.ok`, no b.bin.

    let ws = d.path().join("ws");
    let r = resume_via_reference(&s, &g1, &ws, &ref_root).unwrap();
    assert_eq!(
        r.kind,
        ResumeKind::ColdFull,
        "un-sentineled dir is not trusted → full rebuild"
    );
    // workspace carries the TRUE content, not the planted corruption.
    assert_eq!(fs::read(ws.join("a.bin")).unwrap(), prng(61));
    assert_eq!(fs::read(ws.join("b.bin")).unwrap(), prng(62));
    assert!(
        ref_root.join(format!("{g1}.ok")).exists(),
        "reference rebuilt + sentineled"
    );
}

// `lineage_ref_subdir` (design-review finding #2): a filesystem-safe, injection-proof, per-lineage name —
// distinct lineages map to distinct names, and a traversal-y lineage id can never escape its parent.
#[test]
fn lineage_ref_subdir_is_safe_and_distinct() {
    let a = lineage_ref_subdir("capsule-run-123");
    let b = lineage_ref_subdir("capsule-run-124");
    let evil = lineage_ref_subdir("../../../etc/passwd");
    assert_ne!(a, b, "distinct lineages → distinct subdirs");
    for n in [&a, &b, &evil] {
        assert!(n.starts_with("lineage-"), "stable prefix: {n}");
        assert!(
            !n.contains('/') && !n.contains("..") && Path::new(n).components().count() == 1,
            "single safe path component: {n}"
        );
    }
}

// Symlinks + hardlinks survive a WARM resume: the reflink clone only handles regular files; links are
// recreated from the manifest (`write_links`). Exercise that a resumed workspace has a correct symlink and
// a hardlink that shares the (reflinked) canonical target's inode, across a cold then a warm generation.
#[test]
fn warm_resume_recreates_symlinks_and_hardlinks() {
    use std::os::unix::fs::{symlink, MetadataExt};
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let tree = d.path().join("t");
    write(&tree.join("canon.bin"), &prng(11));
    fs::hard_link(tree.join("canon.bin"), tree.join("hl.bin")).unwrap();
    symlink("canon.bin", tree.join("ln")).unwrap();
    write(&tree.join("data.bin"), &prng(12));
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;

    let ref_root = d.path().join("refroot");
    resume_via_reference(&s, &g1, &d.path().join("ws1"), &ref_root).unwrap();

    // gen2: change data.bin; canon/hl/ln unchanged → warm resume from g1.
    write(&tree.join("data.bin"), &prng(99));
    let g2 = publish(&tree, &s, &load_index(&s, &g1), Some(g1.clone()))
        .unwrap()
        .manifest;
    let ws2 = d.path().join("ws2");
    let r = resume_via_reference(&s, &g2, &ws2, &ref_root).unwrap();
    assert_eq!(r.kind, ResumeKind::WarmIncremental);

    assert_eq!(fs::read(ws2.join("canon.bin")).unwrap(), prng(11));
    assert_eq!(fs::read(ws2.join("data.bin")).unwrap(), prng(99));
    // symlink recreated verbatim
    assert_eq!(
        fs::read_link(ws2.join("ln")).unwrap().to_str().unwrap(),
        "canon.bin"
    );
    // hardlink shares the reflinked canonical target's inode
    assert_eq!(
        fs::metadata(ws2.join("canon.bin")).unwrap().ino(),
        fs::metadata(ws2.join("hl.bin")).unwrap().ino(),
        "hardlink shares the reflinked target's inode"
    );
}

// A 3-generation chain (g1→g2→g3) each resumes warm from the prior committed reference, the workspace is
// correct at g3, and GC keeps EXACTLY the current reference (never accumulates old gens).
#[test]
fn multi_generation_chain_keeps_only_current() {
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let tree = d.path().join("t");
    write(&tree.join("a.bin"), &prng(1));
    write(&tree.join("b.bin"), &prng(2));
    write(&tree.join("c.bin"), &prng(3));
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let ref_root = d.path().join("refroot");
    let k1 = resume_via_reference(&s, &g1, &d.path().join("w1"), &ref_root).unwrap();
    assert_eq!(k1.kind, ResumeKind::ColdFull);

    write(&tree.join("a.bin"), &prng(11)); // gen2 changes a
    let g2 = publish(&tree, &s, &load_index(&s, &g1), Some(g1.clone()))
        .unwrap()
        .manifest;
    let k2 = resume_via_reference(&s, &g2, &d.path().join("w2"), &ref_root).unwrap();
    assert_eq!(k2.kind, ResumeKind::WarmIncremental);

    write(&tree.join("b.bin"), &prng(22)); // gen3 changes b
    let g3 = publish(&tree, &s, &load_index(&s, &g2), Some(g2.clone()))
        .unwrap()
        .manifest;
    let w3 = d.path().join("w3");
    let k3 = resume_via_reference(&s, &g3, &w3, &ref_root).unwrap();
    assert_eq!(k3.kind, ResumeKind::WarmIncremental);

    // workspace at g3 = latest of each file.
    assert_eq!(fs::read(w3.join("a.bin")).unwrap(), prng(11));
    assert_eq!(fs::read(w3.join("b.bin")).unwrap(), prng(22));
    assert_eq!(fs::read(w3.join("c.bin")).unwrap(), prng(3));
    // exactly one reference kept (the current).
    assert_eq!(
        ref_dirs(&ref_root),
        vec![g3.clone()],
        "only current ref kept"
    );
    assert_eq!(current(&ref_root).as_deref(), Some(g3.as_str()));
}

// GC (finding #4) lstat's ref dirs: a hex-named SYMLINK planted in ref_root is neither followed nor its
// target removed (a hardened `is_dir()` would have followed it).
#[test]
fn gc_ignores_hex_named_symlink() {
    use std::os::unix::fs::symlink;
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let tree = d.path().join("t");
    write(&tree.join("a.bin"), &prng(71));
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let ref_root = d.path().join("refroot");
    resume_via_reference(&s, &g1, &d.path().join("ws1"), &ref_root).unwrap();

    // a valuable sibling dir, and a hex-named symlink under ref_root pointing at it.
    let victim = d.path().join("victim");
    fs::create_dir_all(&victim).unwrap();
    fs::write(victim.join("keep"), b"precious").unwrap();
    let hexname = "b".repeat(64);
    symlink(&victim, ref_root.join(&hexname)).unwrap();

    // resume again (runs gc_other_refs with keep=g1).
    resume_via_reference(&s, &g1, &d.path().join("ws2"), &ref_root).unwrap();
    assert!(
        victim.join("keep").exists(),
        "GC must not follow a hex-named symlink and delete its target"
    );
}

// The `target` digest is validated (finding #3): a non-hex target is rejected before any fs mutation.
#[test]
fn resume_rejects_non_hex_target() {
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let r = resume_via_reference(
        &s,
        "not-a-64-hex-digest",
        &d.path().join("ws"),
        &d.path().join("refroot"),
    );
    assert!(r.is_err(), "non-hex target must be rejected");
}
