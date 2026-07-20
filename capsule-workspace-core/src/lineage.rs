//! Lineage HEAD — the single mutable pointer per workspace, guarded by the monotonic fence.
//! Replaces physical volume fencing with a logical CAS: a second writer is REJECTED, not
//! corrupting. Prototype is file-backed; the real impl is Postgres (the row + fence already
//! exist in `capsule_control.rs` / `capsule_attempts.fencing_token`).

use crate::ifaces::{Fence, LineageId};
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Head {
    pub manifest_digest: String,
    pub fence: Fence,
}

pub trait LineageStore {
    fn get(&self, id: &LineageId) -> Option<Head>;
    /// Advance HEAD only if `expected` matches current fence. Returns the new HEAD or errors
    /// (StaleFence). This is the single-writer guarantee.
    fn advance(&mut self, id: &LineageId, digest: String, expected: Fence) -> Result<Head>;
}

/// File-backed prototype store.
pub struct FileLineageStore {
    path: PathBuf,
    map: HashMap<String, Head>,
}

impl FileLineageStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let map = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            HashMap::new()
        };
        Ok(Self { path, map })
    }
    fn flush(&self) -> Result<()> {
        // atomic write-then-rename (same discipline as cas.rs): the HEAD map is the mutable commit
        // pointer, so a crash mid-write must not leave torn/partial JSON that wedges recovery.
        let tmp = self
            .path
            .with_extension(format!("tmp.{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec(&self.map)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

impl LineageStore for FileLineageStore {
    fn get(&self, id: &LineageId) -> Option<Head> {
        self.map.get(&id.0).cloned()
    }
    fn advance(&mut self, id: &LineageId, digest: String, expected: Fence) -> Result<Head> {
        let cur = self.map.get(&id.0).map(|h| h.fence).unwrap_or(Fence(0));
        if cur != expected {
            bail!("StaleFence: expected {:?}, current {:?}", expected, cur);
        }
        let head = Head {
            manifest_digest: digest,
            fence: Fence(expected.0 + 1),
        };
        self.map.insert(id.0.clone(), head.clone());
        self.flush()?;
        Ok(head)
    }
}
