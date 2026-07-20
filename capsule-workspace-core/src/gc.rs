//! E8 — grace-period mark-sweep GC. A block/manifest is collectable only when it is (a) not
//! referenced by any LIVE manifest AND (b) older than `grace`. The grace period is the whole
//! safety mechanism: an in-flight publish writes its blocks BEFORE committing the manifest
//! that references them, so those blocks are momentarily orphan-but-young. `grace` must exceed
//! the longest possible publish duration, or GC can delete a block the about-to-commit manifest
//! needs → silent corruption. This is the file-backed prototype of the real (Postgres-lineage-
//! driven) collector; the invariant is identical.

use crate::cas::BlockId;
use crate::manifest::Manifest;
use anyhow::Result;
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

#[derive(Debug, Default, serde::Serialize)]
pub struct GcStats {
    pub blocks_deleted: usize,
    pub blocks_kept: usize,
    pub bytes_deleted: u64,
    pub bytes_kept: u64,
    pub manifests_deleted: usize,
    pub blocks_young_orphans_protected: usize,
}

/// Collect orphan blocks + manifests in a `LocalBlobStore` rooted at `store_root`.
/// `live` = the manifest digests currently within retention (their referenced blocks are kept).
pub fn collect(store_root: &Path, live: &[String], grace: Duration) -> Result<GcStats> {
    let now = SystemTime::now();
    let mut st = GcStats::default();

    // MARK: referenced blocks across all live manifests
    let mut live_blocks: HashSet<BlockId> = HashSet::new();
    for dig in live {
        let mbytes = std::fs::read(store_root.join("manifests").join(dig))?;
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

    // SWEEP blocks
    for e in std::fs::read_dir(store_root.join("blocks"))? {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_string();
        if name.contains(".tmp") {
            continue; // an in-flight write's temp file — never a collectable block
        }
        let meta = e.metadata()?;
        if live_blocks.contains(&name) {
            st.blocks_kept += 1;
            st.bytes_kept += meta.len();
            continue;
        }
        // orphan: collectable ONLY if older than grace
        let age = now
            .duration_since(meta.modified()?)
            .unwrap_or(Duration::ZERO);
        if age < grace {
            st.blocks_kept += 1;
            st.bytes_kept += meta.len();
            st.blocks_young_orphans_protected += 1;
            continue;
        }
        let sz = meta.len();
        std::fs::remove_file(e.path())?;
        st.blocks_deleted += 1;
        st.bytes_deleted += sz;
    }

    // SWEEP manifests (superseded + old)
    for e in std::fs::read_dir(store_root.join("manifests"))? {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_string();
        if name.contains(".tmp") || live_manifests.contains(name.as_str()) {
            continue;
        }
        let age = now
            .duration_since(e.metadata()?.modified()?)
            .unwrap_or(Duration::ZERO);
        if age < grace {
            continue;
        }
        std::fs::remove_file(e.path())?;
        st.manifests_deleted += 1;
    }
    Ok(st)
}
