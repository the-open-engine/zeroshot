//! `CachedBlobStore` — a two-tier `BlobStore`: a node-local `cache` (NVMe `LocalBlobStore`) in front
//! of a durable `backing` store (S3). This is the "node-local NVMe cache" of the design: reads prefer
//! the cache (local, fast — no network), and a miss falls back to the backing store and POPULATES the
//! cache so the next read is warm. It is the mechanism behind fast WARM resume (a node that recently
//! published/materialized this lineage already holds the blocks locally, so materialize does local
//! reads instead of S3 GETs).
//!
//! LOAD-BEARING INVARIANT: the cache is NEVER authoritative. Every write goes to the durable backing
//! FIRST, then best-effort to the cache; a cache write failure never fails the operation. Reads that
//! miss the cache are served from backing. Because blocks/manifests are CONTENT-ADDRESSED, a stale
//! cache entry (e.g. a block a central GC deleted from the backing) is byte-identical to what it
//! keys, so serving it is harmless; and a block genuinely gone from backing (and absent from cache)
//! correctly surfaces `NotFound`. Deletes hit BOTH tiers so a node doesn't keep serving a block the
//! authority reclaimed.

use crate::cas::{BlobStore, BlockId, LocalBlobStore, StoreError};
use anyhow::Result;
use std::path::Path;

pub struct CachedBlobStore {
    cache: LocalBlobStore,
    backing: Box<dyn BlobStore>,
}

fn is_not_found(e: &anyhow::Error) -> bool {
    matches!(
        e.downcast_ref::<StoreError>(),
        Some(StoreError::NotFound(_))
    )
}

impl CachedBlobStore {
    /// `cache_root` is the node-local NVMe dir (fast, disposable); `backing` is the durable authority.
    pub fn new(cache_root: impl AsRef<Path>, backing: Box<dyn BlobStore>) -> Result<Self> {
        Ok(Self {
            cache: LocalBlobStore::new(cache_root.as_ref())?,
            backing,
        })
    }

    /// Read-through: cache hit → local; miss → backing, then populate the cache (best-effort).
    fn get_through(
        &self,
        from_cache: impl Fn(&LocalBlobStore) -> Result<Vec<u8>>,
        from_backing: impl Fn(&dyn BlobStore) -> Result<Vec<u8>>,
        to_cache: impl Fn(&LocalBlobStore, &[u8]) -> Result<()>,
    ) -> Result<Vec<u8>> {
        match from_cache(&self.cache) {
            Ok(b) => Ok(b),
            Err(e) if is_not_found(&e) => {
                let b = from_backing(self.backing.as_ref())?;
                let _ = to_cache(&self.cache, &b); // best-effort; a cache write must not fail the read
                Ok(b)
            }
            Err(e) => Err(e),
        }
    }
}

impl BlobStore for CachedBlobStore {
    fn put_block(&self, id: &BlockId, bytes: &[u8]) -> Result<()> {
        self.backing.put_block(id, bytes)?; // durable authority FIRST
        let _ = self.cache.put_block(id, bytes); // populate cache best-effort
        Ok(())
    }
    fn get_block(&self, id: &BlockId) -> Result<Vec<u8>> {
        self.get_through(
            |c| c.get_block(id),
            |b| b.get_block(id),
            |c, bytes| c.put_block(id, bytes),
        )
    }
    fn put_manifest(&self, digest: &str, bytes: &[u8]) -> Result<()> {
        self.backing.put_manifest(digest, bytes)?;
        let _ = self.cache.put_manifest(digest, bytes);
        Ok(())
    }
    fn get_manifest(&self, digest: &str) -> Result<Vec<u8>> {
        self.get_through(
            |c| c.get_manifest(digest),
            |b| b.get_manifest(digest),
            |c, bytes| c.put_manifest(digest, bytes),
        )
    }
    fn has_block(&self, id: &BlockId) -> bool {
        self.cache.has_block(id) || self.backing.has_block(id)
    }
    fn delete_block(&self, id: &BlockId) -> Result<bool> {
        // authority first; also drop from the cache so a node stops serving a reclaimed block.
        let removed = self.backing.delete_block(id)?;
        let _ = self.cache.delete_block(id);
        Ok(removed)
    }
    fn delete_manifest(&self, digest: &str) -> Result<bool> {
        let removed = self.backing.delete_manifest(digest)?;
        let _ = self.cache.delete_manifest(digest);
        Ok(removed)
    }
}
