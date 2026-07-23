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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub struct CachedBlobStore {
    cache: LocalBlobStore,
    cache_root: PathBuf,
    backing: Box<dyn BlobStore>,
    /// Byte ceiling for the cache tier. `None` = unbounded (the historical behaviour).
    max_bytes: Option<u64>,
    /// Bytes written since the last sweep — sweeping on every write would be O(cache) per block.
    since_sweep: AtomicU64,
    /// One sweeper at a time; a concurrent caller just skips (the next write re-triggers).
    sweeping: AtomicBool,
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
        Self::with_limit(cache_root, backing, None)
    }

    /// Same, with a byte ceiling on the cache tier.
    ///
    /// WHY THIS EXISTS: without a bound the cache grows until the device fills — and that device is the
    /// same ephemeral NVMe that holds `--ref-dir` and, typically, the agent's workspace. Cache writes are
    /// best-effort so they fail silently; the workspace writes that fail alongside them do NOT. An
    /// unbounded cache therefore converts "this node has been busy for a while" into a workspace-write
    /// outage, which is the same failure class as the unbounded materialize buffer.
    pub fn with_limit(
        cache_root: impl AsRef<Path>,
        backing: Box<dyn BlobStore>,
        max_bytes: Option<u64>,
    ) -> Result<Self> {
        Ok(Self {
            cache: LocalBlobStore::new(cache_root.as_ref())?,
            cache_root: cache_root.as_ref().to_path_buf(),
            backing,
            max_bytes,
            since_sweep: AtomicU64::new(0),
            sweeping: AtomicBool::new(false),
        })
    }

    /// Evict oldest-by-mtime until the cache is back under ~90% of the ceiling. Safe at any moment: the
    /// cache is never authoritative, so a miss simply falls through to the durable backing store.
    /// Amortised — only runs once an eighth of the ceiling has been written since the last sweep.
    fn maybe_sweep(&self, wrote: u64) {
        let Some(max) = self.max_bytes else { return };
        if self.since_sweep.fetch_add(wrote, Ordering::Relaxed) + wrote < max / 8 {
            return;
        }
        if self.sweeping.swap(true, Ordering::SeqCst) {
            return; // another thread is already sweeping
        }
        self.since_sweep.store(0, Ordering::Relaxed);
        let blocks = self.cache_root.join("blocks");
        let mut entries: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
        let mut total: u64 = 0;
        if let Ok(rd) = std::fs::read_dir(&blocks) {
            for e in rd.flatten() {
                if let Ok(m) = e.metadata() {
                    if m.is_file() {
                        total += m.len();
                        entries.push((
                            m.modified().unwrap_or(std::time::UNIX_EPOCH),
                            m.len(),
                            e.path(),
                        ));
                    }
                }
            }
        }
        if total > max {
            entries.sort_by_key(|(t, _, _)| *t); // oldest first
            let target = max - max / 10;
            for (_, len, path) in entries {
                if total <= target {
                    break;
                }
                if std::fs::remove_file(&path).is_ok() {
                    total = total.saturating_sub(len);
                }
            }
        }
        self.sweeping.store(false, Ordering::SeqCst);
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
        self.maybe_sweep(bytes.len() as u64);
        Ok(())
    }
    fn get_block(&self, id: &BlockId) -> Result<Vec<u8>> {
        self.get_through(
            |c| c.get_block(id),
            |b| b.get_block(id),
            |c, bytes| {
                let r = c.put_block(id, bytes);
                self.maybe_sweep(bytes.len() as u64);
                r
            },
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
