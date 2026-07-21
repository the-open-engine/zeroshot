//! Experiment suite — realistic correctness / adversarial / data-model scenarios encoded as
//! auditable assertions. Each test maps to a tracker ID in issue #744. Run: `cargo test`.
//! These assert the CORRECT behavior; where the prototype was wrong, the fix landed with the
//! test (see git history). Scope is realistic (untrusted tenant tree, corrupt/tampered store,
//! weird-but-legal filenames) — not contrived (no post-quantum threat model).

use capsule_workspace_core::cas::*;
use capsule_workspace_core::daemon::{materialize, publish};
use capsule_workspace_core::ifaces::{Fence, LineageId};
use capsule_workspace_core::lineage::{FileLineageStore, LineageStore};
use capsule_workspace_core::manifest::{FileEntry, Manifest};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}
fn write(p: &Path, bytes: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}
fn pub_(tree: &Path, store: &Path) -> String {
    let s = LocalBlobStore::new(store).unwrap();
    publish(tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest
}

// ---------- baseline: round-trip identity on a realistic mixed tree ----------
#[test]
fn round_trip_identity() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("a/small.txt"), b"hello world\n");
    write(&tree.join("a/b/mid.bin"), &vec![7u8; 300_000]); // spans 2 chunks
    write(&tree.join("dup.bin"), &vec![7u8; 300_000]); // identical -> dedup
    let dig = pub_(&tree, &d.path().join("s"));
    let out = d.path().join("o");
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    materialize(&s, &dig, &out).unwrap();
    for rel in ["a/small.txt", "a/b/mid.bin", "dup.bin"] {
        assert_eq!(
            fs::read(tree.join(rel)).unwrap(),
            fs::read(out.join(rel)).unwrap(),
            "{rel}"
        );
    }
}

// ---------- C4: symlink fidelity (was: silently dropped) ----------
#[test]
fn symlinks_preserved() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("real.txt"), b"data");
    symlink("real.txt", tree.join("rel.link")).unwrap();
    symlink("/etc/hostname", tree.join("abs.link")).unwrap();
    let dig = pub_(&tree, &d.path().join("s"));
    let out = d.path().join("o");
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    materialize(&s, &dig, &out).unwrap();
    assert_eq!(
        fs::read_link(out.join("rel.link"))
            .unwrap()
            .to_str()
            .unwrap(),
        "real.txt"
    );
    assert_eq!(
        fs::read_link(out.join("abs.link"))
            .unwrap()
            .to_str()
            .unwrap(),
        "/etc/hostname"
    );
}

// ---------- C4: file mode preservation ----------
#[test]
fn exec_mode_preserved() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("run.sh"), b"#!/bin/sh\n");
    fs::set_permissions(tree.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    let dig = pub_(&tree, &d.path().join("s"));
    let out = d.path().join("o");
    materialize(
        &LocalBlobStore::new(d.path().join("s")).unwrap(),
        &dig,
        &out,
    )
    .unwrap();
    assert_eq!(
        fs::metadata(out.join("run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}

// ---------- A2: weird-but-legal filenames round-trip; '..' path is rejected ----------
#[test]
fn adversarial_filenames_roundtrip() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("with space.txt"), b"x");
    write(&tree.join("un\u{00e9}code\u{1f600}.bin"), b"y");
    write(&tree.join("dots..in..name.txt"), b"z"); // dots in name (not a .. component) is fine
    let dig = pub_(&tree, &d.path().join("s"));
    let out = d.path().join("o");
    materialize(
        &LocalBlobStore::new(d.path().join("s")).unwrap(),
        &dig,
        &out,
    )
    .unwrap();
    assert!(out.join("with space.txt").exists());
    assert!(out.join("un\u{00e9}code\u{1f600}.bin").exists());
    // and the safe-path validator rejects true traversal
    assert!(safe_rel_path("../escape").is_err());
    assert!(safe_rel_path("/abs").is_err());
    assert!(safe_rel_path("a/../../b").is_err());
    assert!(safe_rel_path("a/b/c").is_ok());
}

// ---------- G2: path-traversal via a tampered manifest must be refused ----------
#[test]
fn tampered_manifest_path_traversal_refused() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("f.txt"), b"content");
    let store = d.path().join("s");
    let dig = pub_(&tree, &store);
    let s = LocalBlobStore::new(&store).unwrap();
    let mut m = Manifest::from_bytes(&s.get_manifest(&dig).unwrap()).unwrap();
    m.files[0].path = "../../ESCAPED".to_string();
    // store under the CORRECT logical digest of the tampered manifest, so it passes the
    // read-integrity check and we specifically exercise safe_rel_path (path validation)
    let bad = m.logical_digest();
    s.put_manifest(&bad, &m.to_bytes()).unwrap();
    let out = d.path().join("o");
    assert!(
        materialize(&s, &bad, &out).is_err(),
        "traversal must be refused"
    );
    assert!(!d.path().join("ESCAPED").exists() && !d.path().join("../ESCAPED").exists());
}

// ---------- G5 (STRONG): chunk integrity catches a VALID-but-WRONG block ----------
// (audit found the corrupt-block test only trips the zstd decoder; this exercises sha256==id)
#[test]
fn chunk_integrity_catches_content_swap() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("secret.txt"), &vec![1u8; 300_000]);
    write(&tree.join("public.txt"), &vec![2u8; 300_000]);
    let store = d.path().join("s");
    let dig = pub_(&tree, &store);
    let s = LocalBlobStore::new(&store).unwrap();
    let mut m = Manifest::from_bytes(&s.get_manifest(&dig).unwrap()).unwrap();
    // repoint secret's chunk at public's (genuine, valid-zstd) block location -> content swap
    let secret_cid = m
        .files
        .iter()
        .find(|f| f.path == "secret.txt")
        .unwrap()
        .chunks[0]
        .clone();
    let public_cid = m
        .files
        .iter()
        .find(|f| f.path == "public.txt")
        .unwrap()
        .chunks[0]
        .clone();
    let public_loc = m.chunks[&public_cid].clone();
    m.chunks.insert(secret_cid, public_loc);
    // logical content unchanged -> same digest key; the tampered PHYSICAL index is what we test.
    // Write directly to disk (bypass put_manifest's content-addressed idempotent skip).
    let ld = m.logical_digest();
    std::fs::write(store.join("manifests").join(&ld), m.to_bytes()).unwrap();
    assert!(
        materialize(&s, &ld, &d.path().join("o")).is_err(),
        "content swap must be caught"
    );
}

// ---------- audit gap: manifest not matching its digest is refused (read integrity) ----------
#[test]
fn manifest_tamper_wrong_digest_refused() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("f.txt"), b"content");
    fs::set_permissions(tree.join("f.txt"), fs::Permissions::from_mode(0o644)).unwrap();
    let store = d.path().join("s");
    let dig = pub_(&tree, &store);
    let s = LocalBlobStore::new(&store).unwrap();
    let mut m = Manifest::from_bytes(&s.get_manifest(&dig).unwrap()).unwrap();
    m.files[0].mode = 0o104755; // tamper: try to set setuid
    // simulate ON-DISK tamper at the original key (bypasses put_manifest's content-addressed
    // idempotent skip, as real corruption/tampering would)
    std::fs::write(store.join("manifests").join(&dig), m.to_bytes()).unwrap();
    assert!(
        materialize(&s, &dig, &d.path().join("o")).is_err(),
        "digest mismatch must be refused"
    );
}

// ---------- audit gap #4: logical identity ignores physical block layout (zstd-independent) ----
#[test]
fn logical_digest_ignores_block_layout() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("f.bin"), &vec![9u8; 300_000]);
    let store = d.path().join("s");
    let dig = pub_(&tree, &store);
    let s = LocalBlobStore::new(&store).unwrap();
    let mut m = Manifest::from_bytes(&s.get_manifest(&dig).unwrap()).unwrap();
    let before = m.logical_digest();
    let k = m.chunks.keys().next().unwrap().clone();
    m.chunks.get_mut(&k).unwrap().block = "different-block-id".into();
    m.chunks.get_mut(&k).unwrap().offset = 4242;
    assert_eq!(
        m.logical_digest(),
        before,
        "identity must not depend on block layout / zstd output"
    );
}

// ---------- audit gap #3: a malicious symlink cannot become a write-through escape ----------
#[test]
fn symlink_no_write_through() {
    let d = tmp();
    let escape = std::env::temp_dir().join(format!("ZS_ESCAPE_{}", std::process::id()));
    let _ = fs::remove_dir_all(&escape);
    // seed a real chunk into the store
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    let seed = d.path().join("seed");
    write(&seed.join("x"), b"pwned");
    let sd = publish(&seed, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let sm = Manifest::from_bytes(&s.get_manifest(&sd).unwrap()).unwrap();
    let cid = sm.files[0].chunks[0].clone();
    // hand-craft: symlink "s" -> escape dir, and a file "s/x" that would write THROUGH it
    let mut m = sm.clone();
    m.files = vec![
        FileEntry {
            path: "s".into(),
            mode: 0o120777,
            size: 0,
            chunks: vec![],
            symlink: Some(escape.to_string_lossy().into()),
            hardlink: None,
        },
        FileEntry {
            path: "s/x".into(),
            mode: 0o100644,
            size: 5,
            chunks: vec![cid],
            symlink: None,
            hardlink: None,
        },
    ];
    let ld = m.logical_digest();
    s.put_manifest(&ld, &m.to_bytes()).unwrap();
    let _ = materialize(&s, &ld, &d.path().join("o")); // may error; must NOT escape
    assert!(
        !escape.join("x").exists(),
        "must not write through a symlink to escape the workspace"
    );
    let _ = fs::remove_dir_all(&escape);
}

// ---------- D1 as a runnable test (audit: it was prose-only) ----------
#[test]
fn d1_fixed_block_shift_sensitivity() {
    let d = tmp();
    let tree = d.path().join("t");
    let mut base = Vec::new();
    for i in 0..40u8 {
        base.extend(std::iter::repeat(i).take(CHUNK));
    } // 40 distinct chunks
    write(&tree.join("f.bin"), &base);
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    let known = {
        let d0 = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
        Manifest::from_bytes(&s.get_manifest(&d0.manifest).unwrap())
            .unwrap()
            .chunks
    };
    // append at end -> only the new tail chunk
    let mut appended = base.clone();
    appended.extend(std::iter::repeat(200u8).take(1024));
    write(&tree.join("f.bin"), &appended);
    let na = publish(&tree, &s, &known, None).unwrap().new_chunks;
    // prepend -> shifts every boundary -> all chunks new
    let mut prepended = vec![201u8; 1024];
    prepended.extend(&base);
    write(&tree.join("f.bin"), &prepended);
    let nb = publish(&tree, &s, &known, None).unwrap().new_chunks;
    assert_eq!(na, 1, "append changes only the tail chunk");
    assert!(
        nb >= 40,
        "byte-insertion shift invalidates ~all chunks (got {nb})"
    );
}

// ---------- C5: a corrupted (invalid-zstd) block is detected on read ----------
#[test]
fn corrupt_block_detected() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("f.bin"), &vec![42u8; 100_000]);
    let store = d.path().join("s");
    let dig = pub_(&tree, &store);
    // corrupt the single block on disk
    let blocks = fs::read_dir(store.join("blocks")).unwrap();
    let bpath = blocks.into_iter().next().unwrap().unwrap().path();
    fs::write(&bpath, b"garbage not a valid zstd frame at all").unwrap();
    let out = d.path().join("o");
    let s = LocalBlobStore::new(&store).unwrap();
    assert!(
        materialize(&s, &dig, &out).is_err(),
        "corruption must be detected on read"
    );
}

// ---------- A1: decompression-bomb defense (bounded decompress) ----------
#[test]
fn decompression_bomb_bounded() {
    let bomb = zstd::stream::encode_all(&vec![0u8; 10 * CHUNK][..], 3).unwrap();
    assert!(
        decompress_bounded(&bomb, CHUNK).is_err(),
        "over-CHUNK output must be rejected"
    );
    let ok = zstd::stream::encode_all(&vec![1u8; 1000][..], 3).unwrap();
    assert_eq!(decompress_bounded(&ok, CHUNK).unwrap().len(), 1000);
}

// ---------- C2: concurrent-writer fence — only one advance from a given fence wins ----------
#[test]
fn concurrent_fence_rejects_stale() {
    let d = tmp();
    let ls = FileLineageStore::open(d.path().join("lin.json")).unwrap();
    let id = LineageId("proj-1".into());
    let h1 = ls.advance(&id, "digestA".into(), Fence(0)).unwrap();
    assert_eq!(h1.fence, Fence(1));
    // a second writer that still thinks fence is 0 must be rejected (not corrupt)
    assert!(ls.advance(&id, "digestB".into(), Fence(0)).is_err());
    // the winner can continue from the new fence
    assert!(ls.advance(&id, "digestC".into(), Fence(1)).is_ok());
}

// ---------- C7: idempotent republish + digest determinism ----------
#[test]
fn idempotent_and_deterministic() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("a.txt"), b"aaa");
    write(&tree.join("b.txt"), b"bbb");
    // same tree, two independent stores -> identical manifest digest (deterministic)
    let dig1 = pub_(&tree, &d.path().join("s1"));
    let dig2 = pub_(&tree, &d.path().join("s2"));
    assert_eq!(
        dig1, dig2,
        "identical input must yield identical manifest digest"
    );
    // republish against prior state -> zero new chunks
    let store = d.path().join("s3");
    let s = LocalBlobStore::new(&store).unwrap();
    let known: ChunkIndex = {
        let d0 = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
        Manifest::from_bytes(&s.get_manifest(&d0.manifest).unwrap())
            .unwrap()
            .chunks
    };
    let again = publish(&tree, &s, &known, None).unwrap();
    assert_eq!(
        again.new_chunks, 0,
        "re-publishing an unchanged tree uploads nothing"
    );
}

// ---------- C4: special files (fifo) are skipped and COUNTED, not silently misrepresented ----
#[test]
fn special_files_counted_not_silently_dropped() {
    let d = tmp();
    let tree = d.path().join("t");
    write(&tree.join("regular.txt"), b"ok");
    // make a fifo
    let fifo = tree.join("a.fifo");
    let cst = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    let rc = unsafe { libc_mkfifo(cst.as_ptr(), 0o644) };
    if rc == 0 {
        let s = LocalBlobStore::new(d.path().join("s")).unwrap();
        let st = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
        assert_eq!(st.skipped_special, 1, "fifo must be visibly counted");
        assert_eq!(st.files, 1, "only the regular file has content");
    }
}

// F1 (optimization): manifest identity is CONTENT-ONLY — `parent` is excluded from `logical_digest`,
// so a byte-identical tree always yields the same manifest digest regardless of lineage predecessor.
// This is what makes an idle daemon idempotent (no manifest churn / orphan leak) and dedups identical
// trees to a single manifest object.
#[test]
fn manifest_identity_content_only_ignores_parent() {
    let d = tmp();
    let tree = d.path().join("t");
    let store = d.path().join("s");
    let s = LocalBlobStore::new(&store).unwrap();
    write(&tree.join("f.bin"), &vec![7u8; 3 * CHUNK]);
    // SAME content, DIFFERENT parent → SAME manifest digest.
    let a = publish(&tree, &s, &ChunkIndex::new(), None).unwrap();
    let b = publish(
        &tree,
        &s,
        &ChunkIndex::new(),
        Some("a-different-parent".into()),
    )
    .unwrap();
    assert_eq!(
        a.manifest, b.manifest,
        "identical tree => identical manifest digest regardless of parent"
    );
    // the two publishes shared a single manifest object (put_manifest deduped on the shared key).
    assert_eq!(
        fs::read_dir(store.join("manifests")).unwrap().count(),
        1,
        "no manifest churn: one object for identical content"
    );
    // it still materializes.
    let out = d.path().join("o");
    materialize(&s, &a.manifest, &out).unwrap();
    assert_eq!(fs::read(out.join("f.bin")).unwrap(), vec![7u8; 3 * CHUNK]);
}

// minimal libc mkfifo shim (avoid pulling the libc crate for one call)
extern "C" {
    #[link_name = "mkfifo"]
    fn libc_mkfifo(path: *const std::os::raw::c_char, mode: u32) -> i32;
}
