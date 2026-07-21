//! E8 — grace-period mark-sweep GC. A block/manifest is collectable only when it is (a) not
//! referenced by any LIVE manifest AND (b) older than `grace`. The grace period is the whole
//! safety mechanism: an in-flight publish writes its blocks BEFORE committing the manifest
//! that references them, so those blocks are momentarily orphan-but-young. `grace` must exceed
//! the longest possible publish duration, or GC can delete a block the about-to-commit manifest
//! needs → silent corruption. This is the file-backed prototype of the real (Postgres-lineage-
//! driven) collector; the invariant is identical.

use crate::cas::{BlobStore, BlockId, StoreError};
use crate::manifest::Manifest;
use anyhow::Result;
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// MARK step (shared by the Postgres-driven `gc_pg` sweep): the set of blocks referenced by ANY live
/// manifest — invariant #2, the blocks GC must NEVER collect regardless of age — plus the count of
/// live manifests that were ABSENT on the store. Reads manifests through the `BlobStore` trait, so it
/// works over S3 or local fs.
///
/// A live manifest that is momentarily absent is SKIPPED (not fatal): it references no blocks we can
/// see, and wedging the whole sweep on one dangling ref would let the store grow unbounded (matches
/// the file-backed collector's `missing_live_manifests` tolerance). But a live HEAD whose manifest we
/// CANNOT read means we could not protect that HEAD's blocks this sweep — a genuine anomaly — so the
/// count is returned for the caller to surface/alert. Only a `StoreError::NotFound` counts as absent;
/// any OTHER error (corruption, throttling, auth) PROPAGATES — treating those as "skip" could drop a
/// still-needed block from the mark set and wrongly collect it (reviewer S2).
pub fn mark_live_blocks(
    store: &dyn BlobStore,
    live: &[String],
) -> Result<(HashSet<BlockId>, usize)> {
    let mut marked = HashSet::new();
    let mut missing = 0usize;
    for dig in live {
        let bytes = match store.get_manifest(dig) {
            Ok(b) => b,
            Err(e)
                if matches!(
                    e.downcast_ref::<StoreError>(),
                    Some(StoreError::NotFound(_))
                ) =>
            {
                missing += 1;
                continue;
            }
            Err(e) => return Err(e),
        };
        let m = Manifest::from_bytes(&bytes)?;
        for f in &m.files {
            for cid in &f.chunks {
                if let Some(loc) = m.chunks.get(cid) {
                    marked.insert(loc.block.clone());
                }
            }
        }
    }
    Ok((marked, missing))
}

#[derive(Debug, Default, serde::Serialize)]
pub struct GcStats {
    pub blocks_deleted: usize,
    pub blocks_kept: usize,
    pub bytes_deleted: u64,
    pub bytes_kept: u64,
    pub manifests_deleted: usize,
    pub blocks_young_orphans_protected: usize,
    pub tmp_deleted: usize,
    /// live manifests that were absent on disk — MARK skipped them and CONTINUED (never wedge
    /// the whole sweep on one dangling ref). A genuinely-absent manifest references no blocks,
    /// so skipping is safe; but this MUST be surfaced/alerted by the caller (a live HEAD that
    /// vanished is an anomaly).
    pub missing_live_manifests: usize,
}

/// Collect orphan blocks + manifests in a `LocalBlobStore` rooted at `store_root`.
/// `live` = the manifest digests currently within retention (their referenced blocks are kept).
/// Safe under concurrent GC (removes tolerate "already gone") but a real deployment should
/// single-flight GC with a lease (concurrent sweeps waste work).
pub fn collect(store_root: &Path, live: &[String], grace: Duration) -> Result<GcStats> {
    let now = SystemTime::now();
    let mut st = GcStats::default();

    // MARK: referenced blocks across all live manifests. A MISSING live manifest is skipped +
    // counted, never a hard error — one dangling ref must not wedge all GC (unbounded growth).
    let mut live_blocks: HashSet<BlockId> = HashSet::new();
    for dig in live {
        let mbytes = match std::fs::read(store_root.join("manifests").join(dig)) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                st.missing_live_manifests += 1;
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        let m = Manifest::from_bytes(&mbytes)?;
        for f in &m.files {
            for cid in &f.chunks {
                if let Some(loc) = m.chunks.get(cid) {
                    live_blocks.insert(loc.block.clone());
                }
            }
        }
    }
    let live_manifests: HashSet<&str> = live.iter().map(|s| s.as_str()).collect();

    // idempotent delete: tolerate "already gone" so a concurrent GC can't error mid-sweep.
    let rm = |p: &Path| -> Result<bool> {
        match std::fs::remove_file(p) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false), // another sweeper got it
            Err(e) => Err(e.into()),
        }
    };

    // SWEEP blocks
    for e in std::fs::read_dir(store_root.join("blocks"))? {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_string();
        let meta = match e.metadata() {
            Ok(m) => m,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue, // raced away
            Err(err) => return Err(err.into()),
        };
        let age = now
            .duration_since(meta.modified()?)
            .unwrap_or(Duration::ZERO);
        if name.contains(".tmp") {
            // a crashed publish's leftover temp; reclaim it only once older than grace (a temp
            // younger than max-publish could belong to an in-flight write).
            if age >= grace && rm(&e.path())? {
                st.tmp_deleted += 1;
                st.bytes_deleted += meta.len();
            }
            continue;
        }
        if live_blocks.contains(&name) {
            st.blocks_kept += 1;
            st.bytes_kept += meta.len();
            continue;
        }
        // orphan: collectable ONLY if older than grace
        if age < grace {
            st.blocks_kept += 1;
            st.bytes_kept += meta.len();
            st.blocks_young_orphans_protected += 1;
            continue;
        }
        let sz = meta.len();
        if rm(&e.path())? {
            st.blocks_deleted += 1;
            st.bytes_deleted += sz;
        }
    }

    // SWEEP manifests (superseded + old)
    for e in std::fs::read_dir(store_root.join("manifests"))? {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_string();
        if name.contains(".tmp") || live_manifests.contains(name.as_str()) {
            continue;
        }
        let meta = match e.metadata() {
            Ok(m) => m,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
        };
        let age = now
            .duration_since(meta.modified()?)
            .unwrap_or(Duration::ZERO);
        if age < grace {
            continue;
        }
        if rm(&e.path())? {
            st.manifests_deleted += 1;
        }
    }
    Ok(st)
}
