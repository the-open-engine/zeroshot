//! Experiment suite — realistic correctness / adversarial / data-model scenarios encoded as
//! auditable assertions. Each test maps to a tracker ID in issue #744. Run: `cargo test`.
//! These assert the CORRECT behavior; where the prototype was wrong, the fix landed with the
//! test (see git history). Scope is realistic (untrusted tenant tree, corrupt/tampered store,
//! weird-but-legal filenames) — not contrived (no post-quantum threat model).

use capsule_workspace_core::cas::*;
use capsule_workspace_core::daemon::{materialize, publish};
use capsule_workspace_core::ifaces::{Fence, LineageId};
use capsule_workspace_core::lineage::{FileLineageStore, LineageStore};
use capsule_workspace_core::manifest::Manifest;
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
    let mb = m.to_bytes();
    let bad = Manifest::digest(&mb);
    s.put_manifest(&bad, &mb).unwrap();
    let out = d.path().join("o");
    assert!(
        materialize(&s, &bad, &out).is_err(),
        "traversal must be refused"
    );
    assert!(!d.path().join("ESCAPED").exists() && !d.path().join("../ESCAPED").exists());
}

// ---------- C5: chunk-integrity — a corrupted block is detected ----------
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
    let mut ls = FileLineageStore::open(d.path().join("lin.json")).unwrap();
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

// minimal libc mkfifo shim (avoid pulling the libc crate for one call)
extern "C" {
    #[link_name = "mkfifo"]
    fn libc_mkfifo(path: *const std::os::raw::c_char, mode: u32) -> i32;
}
