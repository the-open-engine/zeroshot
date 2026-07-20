//! Immutable per-publish manifest — the commit point. References (block, offset, len) spans.

use crate::cas::{ChunkId, ChunkIndex};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub mode: u32,
    pub size: u64,
    pub chunks: Vec<ChunkId>,
    /// `Some(target)` = symlink (chunks empty); `None` = regular file. Preserves the common
    /// `node_modules/.bin` and venv symlinks that a file-only walk silently drops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink: Option<String>,
}

/// One publish. `parent` links the lineage; `chunks` is the full resolved index needed to
/// materialize this tree (self-contained so a cold node needs only this manifest + blocks).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub parent: Option<String>,
    pub files: Vec<FileEntry>,
    pub chunks: ChunkIndex,
}

impl Manifest {
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("manifest serialize")
    }
    pub fn from_bytes(b: &[u8]) -> anyhow::Result<Self> {
        Ok(serde_json::from_slice(b)?)
    }
    pub fn digest(bytes: &[u8]) -> String {
        crate::cas::hex_sha256(bytes)
    }

    /// LOGICAL identity — hash over the file tree + chunk-plaintext ids only, EXCLUDING the
    /// physical chunk→block index. Two nodes with different zstd versions/levels therefore
    /// compute the same identity for a byte-identical tree (fixes the cross-node lineage /
    /// manifest-dedup fragility). The physical index is still carried for materialization but
    /// is authenticated per-chunk by content hashing on read, not by this digest.
    pub fn logical_digest(&self) -> String {
        #[derive(Serialize)]
        struct LF<'a> {
            path: &'a str,
            mode: u32,
            size: u64,
            symlink: &'a Option<String>,
            chunks: &'a Vec<ChunkId>,
        }
        #[derive(Serialize)]
        struct L<'a> {
            parent: &'a Option<String>,
            files: Vec<LF<'a>>,
        }
        let mut files: Vec<&FileEntry> = self.files.iter().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let l = L {
            parent: &self.parent,
            files: files
                .iter()
                .map(|f| LF {
                    path: &f.path,
                    mode: f.mode,
                    size: f.size,
                    symlink: &f.symlink,
                    chunks: &f.chunks,
                })
                .collect(),
        };
        crate::cas::hex_sha256(&serde_json::to_vec(&l).expect("logical serialize"))
    }
}
