//! Content-addressed store: fixed 256 KiB chunks -> sha256 -> zstd -> ~64 MiB blocks.
//! Chunk size equals the LVM thin pool `--chunksize 256K` so block-deltas map 1:1 to chunks.

use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
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

/// Typed store error so callers distinguish a genuinely-absent object from a transient failure —
/// GC and crash-retry/republish logic depend on `NotFound` ≠ generic error. Uniform across every
/// `BlobStore` backend (local fs and S3). Carries only the key, never any secret material.
#[derive(Debug)]
pub enum StoreError {
    NotFound(String),
}
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NotFound(k) => write!(f, "object not found: {k}"),
        }
    }
}
impl std::error::Error for StoreError {}

/// Byte-storage port behind `ArtifactRef` receipts. `LocalBlobStore` is the default; an
/// `S3BlobStore` (feature `s3`) is the drop-in for the node daemon. Immutable, content-keyed.
pub trait BlobStore: Send + Sync {
    fn put_block(&self, id: &BlockId, bytes: &[u8]) -> Result<()>;
    fn get_block(&self, id: &BlockId) -> Result<Vec<u8>>;
    fn put_manifest(&self, digest: &str, bytes: &[u8]) -> Result<()>;
    fn get_manifest(&self, digest: &str) -> Result<Vec<u8>>;
    fn has_block(&self, id: &BlockId) -> bool;
    /// Idempotent delete: returns `true` if THIS call removed the object, `false` if it was
    /// already absent. Never errors on "already gone" (a concurrent GC / re-drive must be safe).
    /// GC deletes the object here AFTER winning the atomic `block_ref` claim (see plan MF1).
    fn delete_block(&self, id: &BlockId) -> Result<bool>;
    fn delete_manifest(&self, digest: &str) -> Result<bool>;
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

/// Refresh a content-addressed object's mtime to "now". Grace-period GC uses mtime as the liveness
/// clock: "a block an in-flight publish references is young, so protect it." A dedup hit reuses a
/// block already on disk WITHOUT rewriting it, so without this its mtime stays old — and a
/// crash -> identical-republish would reuse old-mtime blocks that a correctly-sized grace no longer
/// protects, letting GC delete a block the about-to-commit manifest needs (R1 recovery-path
/// corruption; reproduced by the reviewer). Touch-on-reference restores the invariant. (Prod
/// replaces mtime with a monotonic Postgres commit timestamp refreshed on EVERY reference.)
fn touch_mtime(p: &Path) -> Result<()> {
    std::fs::File::options()
        .write(true)
        .open(p)?
        .set_modified(std::time::SystemTime::now())?;
    Ok(())
}

impl BlobStore for LocalBlobStore {
    fn put_block(&self, id: &BlockId, bytes: &[u8]) -> Result<()> {
        let p = self.block_path(id);
        // Dedup hit: refresh mtime so grace-period GC still treats this reused block as young. If
        // the refresh fails (e.g. a concurrent GC just collected it), fall through and rewrite from
        // the bytes we hold — either way the block ends present AND young.
        if p.exists() && touch_mtime(&p).is_ok() {
            return Ok(());
        }
        // write-then-rename for atomicity. Temp name is per-writer UNIQUE (pid + counter) so
        // two concurrent writers persisting the SAME block id do not collide on the temp file
        // (rename is idempotent: last writer wins, both see the final block).
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = p.with_extension(format!("{}.{}.tmp", std::process::id(), n));
        std::fs::write(&tmp, bytes)?;
        // rename onto the final path; if a racer already placed it, drop our temp.
        if std::fs::rename(&tmp, &p).is_err() {
            let _ = std::fs::remove_file(&tmp);
            if !p.exists() {
                anyhow::bail!("put_block: rename failed and block absent");
            }
        }
        Ok(())
    }
    fn get_block(&self, id: &BlockId) -> Result<Vec<u8>> {
        read_or_notfound(&self.block_path(id), id)
    }
    fn put_manifest(&self, digest: &str, bytes: &[u8]) -> Result<()> {
        // atomic write-then-rename with a per-writer UNIQUE temp (same fix as put_block): two
        // writers committing the SAME content-addressed digest concurrently (two lineages with
        // an identical tree) must not collide on the temp file.
        let p = self.manifest_path(digest);
        // Same reuse-clock fix as put_block: a crash -> identical-republish re-commits an existing
        // manifest; without refreshing its mtime, GC's manifest sweep could collect it in the
        // window before it becomes the live HEAD, leaving HEAD dangling. Fall through to rewrite if
        // the refresh fails.
        if p.exists() && touch_mtime(&p).is_ok() {
            return Ok(());
        }
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = p.with_extension(format!("{}.{}.tmp", std::process::id(), n));
        std::fs::write(&tmp, bytes)?;
        if std::fs::rename(&tmp, &p).is_err() {
            let _ = std::fs::remove_file(&tmp);
            if !p.exists() {
                anyhow::bail!("put_manifest: rename failed and manifest absent");
            }
        }
        Ok(())
    }
    fn get_manifest(&self, digest: &str) -> Result<Vec<u8>> {
        read_or_notfound(&self.manifest_path(digest), digest)
    }
    fn has_block(&self, id: &BlockId) -> bool {
        self.block_path(id).exists()
    }
    fn delete_block(&self, id: &BlockId) -> Result<bool> {
        rm_idempotent(&self.block_path(id))
    }
    fn delete_manifest(&self, digest: &str) -> Result<bool> {
        rm_idempotent(&self.manifest_path(digest))
    }
}

/// Read a content-addressed object, mapping a missing file to the typed [`StoreError::NotFound`]
/// (so callers get the same NotFound signal as the S3 backend) while other IO errors propagate.
fn read_or_notfound(p: &Path, key: &str) -> Result<Vec<u8>> {
    match std::fs::read(p) {
        Ok(b) => Ok(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(StoreError::NotFound(key.to_string()).into())
        }
        Err(e) => Err(e.into()),
    }
}

/// Idempotent file removal: `true` if removed, `false` if already gone, error otherwise.
fn rm_idempotent(p: &Path) -> Result<bool> {
    match std::fs::remove_file(p) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
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

/// Bounded decompress — caps output allocation at `max` bytes so a crafted block cannot
/// blow up materialize memory (decompression-bomb defense). Legit chunks are ≤ `CHUNK`.
pub fn decompress_bounded(comp: &[u8], max: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let dec = zstd::stream::read::Decoder::new(comp)?;
    let mut out = Vec::new();
    dec.take(max as u64 + 1).read_to_end(&mut out)?;
    if out.len() > max {
        anyhow::bail!("decompressed chunk exceeds bound {} (possible bomb)", max);
    }
    Ok(out)
}

/// Reject a manifest-supplied path that would escape the workspace root (path traversal).
/// Realistic tenant/manifest defense: no absolute paths, no `..`, no NUL, no empty.
pub fn safe_rel_path(rel: &str) -> Result<()> {
    if rel.is_empty() {
        anyhow::bail!("empty path");
    }
    if rel.starts_with('/') || rel.contains('\0') {
        anyhow::bail!("unsafe path (absolute or NUL): {:?}", rel);
    }
    for comp in rel.split('/') {
        if comp == ".." {
            anyhow::bail!("path escapes workspace via '..': {:?}", rel);
        }
    }
    Ok(())
}

/// A resolved chunk index (chunk_id -> location) — the dedup set consulted on publish.
/// BTreeMap (not HashMap) so manifest serialization is deterministic: identical input must
/// produce an identical manifest digest (content addressing requires it).
pub type ChunkIndex = BTreeMap<ChunkId, ChunkLoc>;

pub fn read_file_chunks(path: &Path) -> Result<Vec<Vec<u8>>> {
    let data = std::fs::read(path)?;
    Ok(data.chunks(CHUNK).map(|c| c.to_vec()).collect())
}
