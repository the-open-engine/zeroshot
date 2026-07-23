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
    /// `Some(canonical_path)` = a hardlink to another entry in this tree (same inode). Chunks
    /// empty. Preserves the pnpm/npm/cargo hardlink model so a linked tree doesn't materialize
    /// to N full copies (E11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardlink: Option<String>,
}

/// One publish. `chunks` is the full resolved index needed to materialize this tree (self-contained
/// so a cold node needs only this manifest + blocks). `parent` is INFORMATIONAL ONLY — it is NOT part
/// of the manifest's logical identity (see `logical_digest`), so a byte-identical tree always yields
/// the same manifest digest regardless of its lineage predecessor. That is what makes an idle daemon
/// idempotent (no churn) and lets identical trees across lineages share one manifest object. Because
/// of that content-dedup, a stored manifest's `parent` reflects whichever writer created the object
/// first; nothing in publish/materialize/GC reads it (the lineage chain lives in `lineage_head`).
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

    /// LOGICAL identity — hash over the file tree + chunk-plaintext ids ONLY, EXCLUDING both the
    /// physical chunk→block index AND `parent`. Excluding the index makes the identity independent of
    /// zstd version/level and block packing (cross-node stability). Excluding `parent` makes it purely
    /// content-addressed: a byte-identical tree ALWAYS produces the same digest, so re-publishing an
    /// unchanged workspace is idempotent (no manifest churn / orphan leak — the F1 fix) and identical
    /// trees dedup. The physical index is still carried for materialization but is authenticated
    /// per-chunk by content hashing on read, not by this digest.
    pub fn logical_digest(&self) -> String {
        #[derive(Serialize)]
        struct LF<'a> {
            path: &'a str,
            mode: u32,
            size: u64,
            symlink: &'a Option<String>,
            hardlink: &'a Option<String>,
            chunks: &'a Vec<ChunkId>,
        }
        #[derive(Serialize)]
        struct L<'a> {
            files: Vec<LF<'a>>,
        }
        let mut files: Vec<&FileEntry> = self.files.iter().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let l = L {
            files: files
                .iter()
                .map(|f| LF {
                    path: &f.path,
                    mode: f.mode,
                    size: f.size,
                    symlink: &f.symlink,
                    hardlink: &f.hardlink,
                    chunks: &f.chunks,
                })
                .collect(),
        };
        // Stream the canonical form straight into the hasher instead of materialising it. The digest is
        // byte-identical (same serializer, same bytes) but a 100k-file manifest no longer allocates a
        // ~17 MB intermediate Vec on every call — and this is called several times per resume.
        use sha2::{Digest, Sha256};
        struct HashWriter(Sha256);
        impl std::io::Write for HashWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.update(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut w = HashWriter(Sha256::new());
        serde_json::to_writer(&mut w, &l).expect("logical serialize");
        crate::cas::hex_of(&w.0.finalize())
    }
}
