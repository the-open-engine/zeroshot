//! The manifest's `logical_digest` IS the content address. If its canonical serialization ever changes,
//! every stored manifest silently becomes unreachable, dedup stops matching across versions, and a node
//! running the new code cannot resume a workspace published by the old one — with no error anywhere.
//!
//! That failure is both catastrophic and completely invisible to every other test in this suite (they all
//! compare a digest to another digest computed by the SAME build, so they agree even if the canonical form
//! drifts). This file pins the digest of a fixed tree to a constant, so any change to the canonical form
//! has to be a deliberate, reviewed act.
//!
//! If this test fails, do NOT just update the constant. Either revert the serialization change, or treat it
//! as a store-format migration.

use capsule_workspace_core::cas::{ChunkIndex, LocalBlobStore};
use capsule_workspace_core::daemon::{publish, publish_pipelined};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;

/// Deterministic bytes (no RNG, no timestamps) so the digest is a pure function of this file.
fn det(seed: u64, len: usize) -> Vec<u8> {
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

fn w(p: &Path, b: &[u8], mode: u32) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b).unwrap();
    // Explicit mode: `mode` is part of the canonical form, so leaving it to the process umask would make
    // this test environment-dependent.
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}

/// A tree covering every shape the canonical form encodes: nested paths, an empty file, a file that is
/// EXACTLY one chunk, a multi-chunk file, distinct modes, a hardlink, and a symlink.
fn build(root: &Path) {
    w(&root.join("a.bin"), &det(1, 300_000), 0o644); // multi-chunk
    w(&root.join("exact_chunk.bin"), &det(2, 262_144), 0o600); // exactly CHUNK
    w(&root.join("empty.bin"), b"", 0o644);
    w(&root.join("dir/nested/c.txt"), b"hello world", 0o755);
    fs::hard_link(root.join("a.bin"), root.join("hard.bin")).unwrap();
    symlink("a.bin", root.join("link")).unwrap();
}

/// Golden digest of the tree built by `build`. Changing this constant is a store-format migration.
const GOLDEN: &str = "93dc01078e11e837304218c0e27b4923177aa56bb9e82a6118c520e42bea8225";

#[test]
fn manifest_digest_is_stable_and_path_independent() {
    let d = tempfile::tempdir().unwrap();
    let tree = d.path().join("t");
    build(&tree);
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let got = publish_pipelined(&tree, &s, &ChunkIndex::new(), None, 4, 8, None)
        .unwrap()
        .manifest;

    // The digest must not depend on WHERE the tree lives (paths are stored relative).
    let d2 = tempfile::tempdir().unwrap();
    let tree2 = d2.path().join("somewhere/else/entirely");
    build(&tree2);
    let s2 = LocalBlobStore::new(d2.path().join("s")).unwrap();
    let got2 = publish_pipelined(&tree2, &s2, &ChunkIndex::new(), None, 4, 8, None)
        .unwrap()
        .manifest;
    assert_eq!(
        got, got2,
        "digest must be independent of the tree's location"
    );

    assert_eq!(
        got, GOLDEN,
        "\nMANIFEST CANONICAL FORM CHANGED.\nEvery previously stored manifest is now unreachable and \
         cross-version resume is broken.\nDo not simply update GOLDEN — revert the serialization change, \
         or handle this as a store-format migration."
    );
}

/// The two publish implementations must agree on the digest, or which one ran would change the content
/// address of an identical tree.
#[test]
fn streaming_and_pipelined_publish_agree() {
    let d = tempfile::tempdir().unwrap();
    let tree = d.path().join("t");
    build(&tree);
    let s1 = LocalBlobStore::new(d.path().join("s1")).unwrap();
    let s2 = LocalBlobStore::new(d.path().join("s2")).unwrap();
    let streaming = publish(&tree, &s1, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let pipelined = publish_pipelined(&tree, &s2, &ChunkIndex::new(), None, 4, 8, None)
        .unwrap()
        .manifest;
    assert_eq!(streaming, pipelined);
    assert_eq!(streaming, GOLDEN);
}
