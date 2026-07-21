//! `CachedBlobStore` — warm-cache tier over a durable backing. Proves: a warm read is served locally
//! (no backing GET), a miss reads-through + populates, deletes hit both tiers, and materialize works
//! over the cached store. The wall-clock warm-vs-cold speedup is measured on real S3 in the EC2 batch;
//! here we assert the CALL behavior that produces it (a counting backing).

use capsule_workspace_core::cache::CachedBlobStore;
use capsule_workspace_core::cas::*;
use capsule_workspace_core::daemon::{materialize, publish};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A `BlobStore` that counts `get_block` calls (the "network" hits we want the cache to eliminate).
/// The counter is an `Arc` so the test can read it after the store is moved into the cache.
struct Counting {
    inner: LocalBlobStore,
    get_blocks: Arc<AtomicUsize>,
}
impl BlobStore for Counting {
    fn put_block(&self, id: &BlockId, b: &[u8]) -> anyhow::Result<()> {
        self.inner.put_block(id, b)
    }
    fn get_block(&self, id: &BlockId) -> anyhow::Result<Vec<u8>> {
        self.get_blocks.fetch_add(1, Ordering::SeqCst);
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

fn write(p: &Path, b: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b).unwrap();
}

#[test]
fn warm_read_skips_backing_and_materialize_works() {
    let d = tempfile::tempdir().unwrap();
    let backing_dir = d.path().join("backing");
    let cache_dir = d.path().join("cache");

    // publish a multi-block tree DIRECTLY to the backing (as another node / a prior generation did) —
    // the cache starts COLD (empty).
    let tree = d.path().join("t");
    write(&tree.join("a.bin"), &vec![3u8; 300_000]);
    write(&tree.join("b.bin"), &vec![9u8; 300_000]);
    let backing0 = LocalBlobStore::new(&backing_dir).unwrap();
    let m = publish(&tree, &backing0, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;

    let counter = Arc::new(AtomicUsize::new(0));
    let counting = Box::new(Counting {
        inner: LocalBlobStore::new(&backing_dir).unwrap(),
        get_blocks: Arc::clone(&counter),
    });
    let cached = CachedBlobStore::new(&cache_dir, counting).unwrap();

    // COLD materialize: every block is a cache miss → read-through to backing (counted) + populate.
    let out1 = d.path().join("o1");
    materialize(&cached, &m, &out1).unwrap();
    assert_eq!(fs::read(out1.join("a.bin")).unwrap(), vec![3u8; 300_000]);
    let cold_gets = counter.load(Ordering::SeqCst);
    assert!(
        cold_gets >= 1,
        "cold materialize read blocks from backing ({cold_gets})"
    );

    // WARM materialize (fresh out dir): every block is now in the local cache → ZERO further backing
    // GETs. This is the warm-resume win (local reads, no network).
    let before = counter.load(Ordering::SeqCst);
    let out2 = d.path().join("o2");
    materialize(&cached, &m, &out2).unwrap();
    assert_eq!(fs::read(out2.join("b.bin")).unwrap(), vec![9u8; 300_000]);
    let warm_gets = counter.load(Ordering::SeqCst) - before;
    assert_eq!(
        warm_gets, 0,
        "warm materialize served every block from the cache (no backing GET)"
    );
}

#[test]
fn write_through_and_delete_hit_both_tiers() {
    let d = tempfile::tempdir().unwrap();
    let backing_dir = d.path().join("backing");
    let cache_dir = d.path().join("cache");
    let cached = CachedBlobStore::new(
        &cache_dir,
        Box::new(LocalBlobStore::new(&backing_dir).unwrap()),
    )
    .unwrap();

    let id = "b".repeat(64);
    cached.put_block(&id, &vec![1u8; 1000]).unwrap();
    // present in BOTH tiers after a write-through
    assert!(
        LocalBlobStore::new(&backing_dir).unwrap().has_block(&id),
        "durable in backing"
    );
    assert!(
        LocalBlobStore::new(&cache_dir).unwrap().has_block(&id),
        "populated in cache"
    );

    // delete drops from both tiers (a node stops serving a reclaimed block)
    assert!(cached.delete_block(&id).unwrap());
    assert!(!LocalBlobStore::new(&backing_dir).unwrap().has_block(&id));
    assert!(!LocalBlobStore::new(&cache_dir).unwrap().has_block(&id));
}
