//! Lineage HEAD — the single mutable pointer per workspace, guarded by the monotonic fence.
//! Replaces physical volume fencing with a logical CAS: a second writer is REJECTED, not
//! corrupting. Prototype is file-backed; the real impl is Postgres (`PgLineageStore`, feature
//! `pg`) — the row + fence mirror `capsule_control.rs` / `capsule_attempts.fencing_token`.

use crate::ifaces::{Fence, LineageId};
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Head {
    pub manifest_digest: String,
    pub fence: Fence,
}

/// Typed CAS failure so a caller can distinguish a lost fence race (matchable, sometimes
/// absorbable as a lost-ack) from an IO/DB error. Matchable via `downcast_ref::<LineageError>()`.
/// `FileLineageStore` keeps its legacy `bail!` string (behavior unchanged); `PgLineageStore`
/// returns this typed variant so the daemon/tests can react to a genuine stale-fence rejection.
#[derive(Debug)]
pub enum LineageError {
    /// The `expected` fence did not match the stored `current` fence — a second writer rejected.
    StaleFence { expected: Fence, current: Fence },
}

impl std::fmt::Display for LineageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LineageError::StaleFence { expected, current } => {
                write!(f, "StaleFence: expected {expected:?}, current {current:?}")
            }
        }
    }
}
impl std::error::Error for LineageError {}

pub trait LineageStore {
    fn get(&self, id: &LineageId) -> Option<Head>;
    /// Advance HEAD only if `expected` matches current fence. Returns the new HEAD or errors
    /// (StaleFence). This is the single-writer guarantee. Takes `&self` (not `&mut self`) so a
    /// pooled/interior-concurrent store can be shared across threads (Postgres pool, or the local
    /// store's `Mutex`).
    fn advance(&self, id: &LineageId, digest: String, expected: Fence) -> Result<Head>;
}

/// File-backed prototype store. The HEAD map is wrapped in a `Mutex` so `advance(&self, ...)` can
/// mutate through a shared reference (matching the pooled `PgLineageStore`) without changing the
/// on-disk format or CAS behavior.
pub struct FileLineageStore {
    path: PathBuf,
    map: Mutex<HashMap<String, Head>>,
}

impl FileLineageStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let map = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            HashMap::new()
        };
        Ok(Self {
            path,
            map: Mutex::new(map),
        })
    }
    fn flush(&self, map: &HashMap<String, Head>) -> Result<()> {
        // atomic write-then-rename (same discipline as cas.rs): the HEAD map is the mutable commit
        // pointer, so a crash mid-write must not leave torn/partial JSON that wedges recovery.
        let tmp = self
            .path
            .with_extension(format!("tmp.{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec(map)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

impl LineageStore for FileLineageStore {
    fn get(&self, id: &LineageId) -> Option<Head> {
        self.map.lock().unwrap().get(&id.0).cloned()
    }
    fn advance(&self, id: &LineageId, digest: String, expected: Fence) -> Result<Head> {
        let mut map = self.map.lock().unwrap();
        let cur = map.get(&id.0).map(|h| h.fence).unwrap_or(Fence(0));
        if cur != expected {
            bail!("StaleFence: expected {:?}, current {:?}", expected, cur);
        }
        let head = Head {
            manifest_digest: digest,
            fence: Fence(expected.0 + 1),
        };
        map.insert(id.0.clone(), head.clone());
        self.flush(&map)?;
        Ok(head)
    }
}
