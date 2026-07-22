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

// O6: uploads run CONCURRENTLY in the pipeline (the S3-publish speedup). A latency-injecting store
// records the MAX concurrent put_block calls — with a multi-block tree + N uploaders it must exceed 1.
struct SlowStore {
    inner: LocalBlobStore,
    cur: std::sync::atomic::AtomicUsize,
    max: std::sync::atomic::AtomicUsize,
}
impl BlobStore for SlowStore {
    fn put_block(&self, id: &BlockId, b: &[u8]) -> anyhow::Result<()> {
        use std::sync::atomic::Ordering::SeqCst;
        let c = self.cur.fetch_add(1, SeqCst) + 1;
        self.max.fetch_max(c, SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(80)); // overlap window (simulates S3 latency)
        let r = self.inner.put_block(id, b);
        self.cur.fetch_sub(1, SeqCst);
        r
    }
    fn get_block(&self, id: &BlockId) -> anyhow::Result<Vec<u8>> {
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

#[test]
fn parallel_uploads_overlap() {
    let d = tmp();
    let tree = d.path().join("t");
    // ~140 MiB of DISTINCT incompressible data → several 64 MiB blocks (multi-block is required to
    // observe upload concurrency).
    let mut body = Vec::new();
    for i in 0..560u64 {
        body.extend(prng(i));
    }
    write(&tree.join("f.bin"), &body);
    let store = SlowStore {
        inner: LocalBlobStore::new(d.path().join("s")).unwrap(),
        cur: 0.into(),
        max: 0.into(),
    };
    let st = publish_pipelined(&tree, &store, &ChunkIndex::new(), None, 4, 8).unwrap();
    assert!(
        st.blocks >= 2,
        "need multiple blocks to observe upload concurrency (got {})",
        st.blocks
    );
    let max = store.max.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        max >= 2,
        "uploads must overlap across workers (max concurrent = {max})"
    );
    // still correct: round-trips byte-identically.
    materialize(&store, &st.manifest, &d.path().join("o")).unwrap();
    assert_eq!(fs::read(d.path().join("o").join("f.bin")).unwrap(), body);
}
