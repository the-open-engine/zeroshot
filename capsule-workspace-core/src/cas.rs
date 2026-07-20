//! Content-addressed store: fixed 256 KiB chunks -> sha256 -> zstd -> ~64 MiB blocks.
//! Chunk size equals the LVM thin pool `--chunksize 256K` so block-deltas map 1:1 to chunks.

use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const CHUNK: usize = 256 * 1024;
pub const BLOCK_TARGET: usize = 64 * 1024 * 1024;
pub const ZSTD_LEVEL: i32 = 3;

pub type ChunkId = String;
pub type BlockId = String;

/// Where a chunk's compressed bytes live: (block, offset, compressed_len, raw_len).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChunkLoc {
    pub block: BlockId,
    pub offset: u64,
    pub clen: u32,
    pub rlen: u32,
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex(&h.finalize())
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{:02x}", x));
    }
    s
}

/// Byte-storage port behind `ArtifactRef` receipts. `LocalBlobStore` is the default; an
/// `S3BlobStore` (feature `s3`) is the drop-in for the node daemon. Immutable, content-keyed.
pub trait BlobStore: Send + Sync {
    fn put_block(&self, id: &BlockId, bytes: &[u8]) -> Result<()>;
    fn get_block(&self, id: &BlockId) -> Result<Vec<u8>>;
    fn put_manifest(&self, digest: &str, bytes: &[u8]) -> Result<()>;
    fn get_manifest(&self, digest: &str) -> Result<Vec<u8>>;
    fn has_block(&self, id: &BlockId) -> bool;
}

/// Filesystem CAS (also serves as the node-local block cache tier).
pub struct LocalBlobStore {
    root: PathBuf,
}

impl LocalBlobStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(root.join("blocks"))?;
        std::fs::create_dir_all(root.join("manifests"))?;
        Ok(Self { root })
    }
    fn block_path(&self, id: &str) -> PathBuf {
        self.root.join("blocks").join(id)
    }
    fn manifest_path(&self, d: &str) -> PathBuf {
        self.root.join("manifests").join(d)
    }
}

impl BlobStore for LocalBlobStore {
    fn put_block(&self, id: &BlockId, bytes: &[u8]) -> Result<()> {
        let p = self.block_path(id);
        if p.exists() {
            return Ok(());
        }
        // write-then-rename for atomicity (torn write => wrong-named temp, never a bad block)
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }
    fn get_block(&self, id: &BlockId) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.block_path(id))?)
    }
    fn put_manifest(&self, digest: &str, bytes: &[u8]) -> Result<()> {
        std::fs::write(self.manifest_path(digest), bytes)?;
        Ok(())
    }
    fn get_manifest(&self, digest: &str) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.manifest_path(digest))?)
    }
    fn has_block(&self, id: &BlockId) -> bool {
        self.block_path(id).exists()
    }
}

/// A block being assembled in memory: compressed chunk bytes concatenated.
pub struct BlockBuilder {
    pub buf: Vec<u8>,
    /// (chunk_id, offset, clen, rlen) for chunks packed into THIS block.
    pub members: Vec<(ChunkId, u64, u32, u32)>,
}

impl BlockBuilder {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(BLOCK_TARGET),
            members: Vec::new(),
        }
    }
    pub fn finalize(self) -> (BlockId, Vec<u8>, Vec<(ChunkId, u64, u32, u32)>) {
        let id = hex_sha256(&self.buf);
        (id, self.buf, self.members)
    }
}

/// Compress one chunk (pure CPU) — parallelized by callers via rayon.
pub fn compress_chunk(raw: &[u8]) -> (ChunkId, Vec<u8>, usize) {
    let id = hex_sha256(raw);
    let comp = zstd::stream::encode_all(raw, ZSTD_LEVEL).expect("zstd");
    (id, comp, raw.len())
}

pub fn decompress(comp: &[u8]) -> Vec<u8> {
    zstd::stream::decode_all(comp).expect("zstd-d")
}

/// A resolved chunk index (chunk_id -> location) — the dedup set consulted on publish.
pub type ChunkIndex = HashMap<ChunkId, ChunkLoc>;

pub fn read_file_chunks(path: &Path) -> Result<Vec<Vec<u8>>> {
    let data = std::fs::read(path)?;
    Ok(data.chunks(CHUNK).map(|c| c.to_vec()).collect())
}
