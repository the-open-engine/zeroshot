//! E6 — bounded-parallel publish must be IDENTITY-EQUIVALENT to streaming publish (same
//! logical digest for the same tree) and round-trip byte-identically. Packing order differs
//! (parallel), so this proves the logical digest is genuinely layout-independent.

use capsule_workspace_core::cas::*;
use capsule_workspace_core::daemon::{materialize, publish, publish_pipelined};
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

#[test]
fn pipelined_equivalent_to_streaming() {
    let d = tmp();
    let tree = d.path().join("t");
    // a mixed tree: many files, distinct multi-chunk bodies, some dups, a symlink
    for i in 0..40u64 {
        let mut body = Vec::new();
        for j in 0..(1 + i % 4) {
            body.extend(prng(i * 10 + j));
        }
        write(&tree.join(format!("d{}/f{}.bin", i % 5, i)), &body);
    }
    write(&tree.join("dup_a.bin"), &prng(999));
    write(&tree.join("sub/dup_b.bin"), &prng(999)); // identical -> dedup
    std::os::unix::fs::symlink("d0/f0.bin", tree.join("link")).unwrap();

    // streaming
    let s1 = LocalBlobStore::new(d.path().join("s1")).unwrap();
    let a = publish(&tree, &s1, &ChunkIndex::new(), None).unwrap();
    // pipelined with several worker counts -> all must yield the SAME logical digest
    for w in [1usize, 2, 4, 8] {
        let sd = d.path().join(format!("sp{}", w));
        let s = LocalBlobStore::new(&sd).unwrap();
        let b = publish_pipelined(&tree, &s, &ChunkIndex::new(), None, w, 8).unwrap();
        assert_eq!(
            a.manifest, b.manifest,
            "pipelined(w={w}) digest must equal streaming"
        );
        assert_eq!(a.new_chunks, b.new_chunks, "same dedup (w={w})");
        // round-trip identical
        let out = d.path().join(format!("o{}", w));
        materialize(&s, &b.manifest, &out).unwrap();
        for i in 0..40u64 {
            let rel = format!("d{}/f{}.bin", i % 5, i);
            assert_eq!(
                fs::read(tree.join(&rel)).unwrap(),
                fs::read(out.join(&rel)).unwrap()
            );
        }
        assert_eq!(
            fs::read_link(out.join("link")).unwrap().to_str().unwrap(),
            "d0/f0.bin"
        );
    }
}
