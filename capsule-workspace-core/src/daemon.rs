//! Node-daemon core: publish (stream->chunk->dedup->pack->upload->manifest) and
//! materialize (manifest->fetch blocks->decompress->write tree).
//! `publish` is SINGLE-THREADED STREAMING (chunk-at-a-time) so peak memory is bounded and
//! independent of file/tree apparent size — deliberately trading the earlier rayon-parallel
//! throughput for bounded RAM (the real daemon reclaims throughput with a bounded-parallel
//! producer/queue/packer pipeline). `materialize` remains rayon-parallel.

use crate::cas::*;
use crate::manifest::{FileEntry, Manifest};
use anyhow::Result;
use rayon::prelude::*;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The previous publish's outputs, enabling the O10 re-hash skip: the parent MANIFEST (the only source of
/// reused chunk lists) plus the node-local stat cache written by that publish. Construct via
/// [`PrevPublish::new`], which refuses a mismatched pair.
#[derive(Clone, Copy)]
pub struct PrevPublish<'a> {
    pub manifest: &'a Manifest,
    pub cache: &'a crate::stat_cache::StatCache,
}

impl<'a> PrevPublish<'a> {
    /// Pair a parent manifest with a stat cache ONLY if that cache was produced by the publish that
    /// created this exact manifest; otherwise `None` (⇒ no skip ⇒ full re-hash).
    ///
    /// This binding is load-bearing, not hygiene. A publish can store its manifest and write its cache and
    /// then LOSE the fence, leaving HEAD on the previous generation. Pairing that cache (which describes
    /// gen N+1's tree) with gen N's manifest would let any file changed in N+1 whose size happened to match
    /// be "skipped" back to gen N's STALE chunk list — silently discarding the agent's work. Making the
    /// unsafe pairing unconstructable is cheaper than remembering not to write it.
    pub fn new(
        manifest: &'a Manifest,
        manifest_digest: &str,
        cache: &'a crate::stat_cache::StatCache,
    ) -> Option<Self> {
        (!manifest_digest.is_empty() && cache.manifest_digest == manifest_digest)
            .then_some(Self { manifest, cache })
    }
}

#[derive(Debug, serde::Serialize)]
pub struct PublishStats {
    pub manifest: String,
    pub files: usize,
    pub total_chunks: usize,
    pub new_chunks: usize,
    pub dedup_pct: f64,
    pub raw_mb: f64,
    pub new_raw_mb: f64,
    pub upload_mb: f64,
    pub zstd_ratio_on_new: f64,
    pub blocks: usize,
    pub symlinks: usize,
    pub skipped_special: usize,
    pub read_hash_secs: f64,
    pub compress_secs: f64,
    pub pack_upload_secs: f64,
    pub wall_secs: f64,
    pub hash_throughput_mbps: f64,
    pub compress_throughput_mbps: f64,
    /// O10: regular files whose chunk list was reused from the parent manifest without being re-read.
    pub skipped_files: usize,
    pub skipped_mb: f64,
    /// Fingerprints observed during THIS publish — persist alongside the new HEAD and feed back as
    /// `PrevPublish::cache` next cycle. Not serialized (it is state, not a stat).
    #[serde(skip)]
    pub stat_cache: crate::stat_cache::StatCache,
    /// The manifest this publish just built and stored (O11). Callers need its chunk index (to refresh the
    /// GC reuse-clock) and its file entries (as the NEXT cycle's parent), and re-fetching it from the store
    /// would be a multi-MB round-trip for something we already hold. `None` only for the streaming
    /// `publish`, which callers do not chain.
    #[serde(skip)]
    pub manifest_obj: Option<Manifest>,
}

/// Walk an opaque tree, classifying entries. Regular files carry content; symlinks are
/// preserved by target (no content); anything else (fifo/socket/device) is counted and
/// skipped rather than silently misrepresented as an empty file.
/// Returns (regular files to chunk [canonical, sorted], extra entries [symlinks + hardlinks],
/// skipped-special count). Hardlinks (same dev+ino as an already-seen regular) become hardlink
/// entries pointing at the lexicographically-first path for that inode — so a pnpm/npm/cargo
/// linked tree materializes with the links intact instead of N full copies.
fn walk_entries(root: &Path) -> Result<(Vec<PathBuf>, Vec<FileEntry>, usize)> {
    use std::os::unix::fs::MetadataExt;
    let mut regs: Vec<(PathBuf, String, u32, u64, u64, u64, u64)> = Vec::new(); // path,rel,mode,size,dev,ino,nlink
    let mut extra: Vec<FileEntry> = Vec::new(); // symlinks (hardlinks appended after resolution)
    let mut skipped = 0usize;
    for e in walkdir::WalkDir::new(root).into_iter() {
        let e = e?;
        let rp = e.path().strip_prefix(root).unwrap();
        if rp.as_os_str().is_empty() {
            continue; // root itself
        }
        // FAIL FAST > silent (E12): a non-UTF8 name would be lossily decoded and could collide
        // with another name → silent data loss. Error instead. (byte-paths is the real fix.)
        let rel = rp
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("non-UTF8 path not supported (fail-fast): {:?}", rp))?
            .to_string();
        let ft = e.file_type();
        if ft.is_dir() {
            continue; // dirs are implied by file paths (empty dirs are a known minor gap)
        } else if ft.is_symlink() {
            let tgt = std::fs::read_link(e.path())?;
            let target = tgt
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("non-UTF8 symlink target (fail-fast): {:?}", tgt))?
                .to_string();
            let mode = e
                .path()
                .symlink_metadata()
                .map(|m| m.mode())
                .unwrap_or(0o120777);
            extra.push(FileEntry {
                path: rel,
                mode,
                size: 0,
                chunks: vec![],
                symlink: Some(target),
                hardlink: None,
            });
        } else if ft.is_file() {
            let m = e.metadata()?;
            regs.push((
                e.path().to_path_buf(),
                rel,
                m.mode(),
                m.len(),
                m.dev(),
                m.ino(),
                m.nlink(),
            ));
        } else {
            skipped += 1; // fifo / socket / block / char device — not workspace content
        }
    }
    // resolve hardlinks deterministically: sort by path; first path per inode is canonical.
    regs.sort_by(|a, b| a.1.cmp(&b.1));
    let mut canonical: std::collections::HashMap<(u64, u64), String> =
        std::collections::HashMap::new();
    let mut to_chunk = Vec::new();
    for (path, rel, mode, size, dev, ino, nlink) in regs {
        if nlink > 1 {
            if let Some(canon) = canonical.get(&(dev, ino)) {
                extra.push(FileEntry {
                    path: rel,
                    mode,
                    size,
                    chunks: vec![],
                    symlink: None,
                    hardlink: Some(canon.clone()),
                });
                continue;
            }
            canonical.insert((dev, ino), rel.clone());
        }
        to_chunk.push(path);
    }
    Ok((to_chunk, extra, skipped))
}

/// Publish a tree. `known` is the chunk index already durable (the dedup set the real
/// daemon consults from the lineage). Returns the new manifest digest + stats.
pub fn publish(
    root: &Path,
    store: &dyn BlobStore,
    known: &ChunkIndex,
    parent: Option<String>,
) -> Result<PublishStats> {
    use std::io::Read;
    let t0 = Instant::now();
    let (mut files, symlink_entries, skipped_special) = walk_entries(root)?;
    files.sort();
    let n_symlinks = symlink_entries
        .iter()
        .filter(|f| f.symlink.is_some())
        .count();

    // STREAMING publish: read each file CHUNK-at-a-time and pack NEW chunks straight into the
    // current ~64 MiB block, flushing as it fills. Peak memory is bounded by ~one block + the
    // seen-id set + the index — NOT by tree/file size. This closes the whole-file-read OOM
    // (sparse or large files could otherwise drive RAM to multiples of apparent size).
    let mut raw_bytes: u64 = 0;
    let mut total_chunks = 0usize;
    let mut new_chunks = 0usize;
    let mut new_raw_bytes: u64 = 0;
    let mut comp_new: u64 = 0;
    let mut file_entries: Vec<FileEntry> = Vec::with_capacity(files.len());
    let mut new_index: ChunkIndex = ChunkIndex::new();
    let mut seen: std::collections::HashSet<ChunkId> = std::collections::HashSet::new();
    let mut cur = BlockBuilder::new();
    let mut n_blocks = 0usize;
    let mut upload_secs = 0f64;
    let mut buf = vec![0u8; CHUNK];

    let flush = |cur: &mut BlockBuilder,
                 new_index: &mut ChunkIndex,
                 n_blocks: &mut usize,
                 upload_secs: &mut f64|
     -> Result<()> {
        if cur.buf.is_empty() {
            return Ok(());
        }
        let (bid, bytes, members) = std::mem::replace(cur, BlockBuilder::new()).finalize();
        for (id, off, clen, rlen) in members {
            new_index.insert(
                id,
                ChunkLoc {
                    block: bid.clone(),
                    offset: off,
                    clen,
                    rlen,
                },
            );
        }
        let tu = Instant::now();
        store.put_block(&bid, &bytes)?;
        *upload_secs += tu.elapsed().as_secs_f64();
        *n_blocks += 1;
        Ok(())
    };

    let t_stream = Instant::now();
    for p in &files {
        let meta = std::fs::metadata(p)?;
        let rel = p.strip_prefix(root).unwrap().to_string_lossy().to_string();
        let mut fh = std::fs::File::open(p)?;
        let mut ids: Vec<ChunkId> = Vec::new();
        loop {
            // fill up to CHUNK bytes, tolerating short reads
            let mut filled = 0;
            while filled < CHUNK {
                match fh.read(&mut buf[filled..])? {
                    0 => break,
                    n => filled += n,
                }
            }
            if filled == 0 {
                break;
            }
            let chunk = &buf[..filled];
            total_chunks += 1;
            raw_bytes += filled as u64;
            let id = hex_sha256(chunk);
            ids.push(id.clone());
            if known.contains_key(&id) || seen.contains(&id) {
                continue; // dedup: already durable or already packed this publish
            }
            seen.insert(id.clone());
            let comp = zstd::stream::encode_all(chunk, ZSTD_LEVEL).expect("zstd");
            let off = cur.buf.len() as u64;
            cur.buf.extend_from_slice(&comp);
            cur.members
                .push((id, off, comp.len() as u32, filled as u32));
            new_chunks += 1;
            new_raw_bytes += filled as u64;
            comp_new += comp.len() as u64;
            if cur.buf.len() >= BLOCK_TARGET {
                flush(&mut cur, &mut new_index, &mut n_blocks, &mut upload_secs)?;
            }
            if filled < CHUNK {
                break; // short final chunk
            }
        }
        file_entries.push(FileEntry {
            path: rel,
            mode: meta.permissions().mode(),
            size: meta.len(),
            chunks: ids,
            symlink: None,
            hardlink: None,
        });
    }
    flush(&mut cur, &mut new_index, &mut n_blocks, &mut upload_secs)?;
    let stream_secs = t_stream.elapsed().as_secs_f64();
    let read_hash_secs = stream_secs;
    let compress_secs = stream_secs;
    let pack_upload_secs = upload_secs;

    // Manifest index = ONLY chunks referenced by THIS manifest's files (not the whole cumulative
    // `known`). Prevents the manifest — the first object a cold node downloads (R2) — from
    // growing with lineage churn independent of live-tree size.
    file_entries.extend(symlink_entries);
    file_entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut index: ChunkIndex = ChunkIndex::new();
    for f in &file_entries {
        for cid in &f.chunks {
            if index.contains_key(cid) {
                continue;
            }
            let loc = new_index
                .get(cid)
                .or_else(|| known.get(cid))
                .ok_or_else(|| anyhow::anyhow!("referenced chunk {} missing from index", cid))?;
            index.insert(cid.clone(), loc.clone());
        }
    }
    let manifest = Manifest {
        parent,
        files: file_entries,
        chunks: index,
    };
    let mbytes = manifest.to_bytes();
    // Identity = LOGICAL digest (content only, zstd-independent) so it is stable across nodes.
    let digest = manifest.logical_digest();
    store.put_manifest(&digest, &mbytes)?;

    let wall = t0.elapsed().as_secs_f64();
    let nc = new_chunks;
    let n_files = manifest.files.len();
    Ok(PublishStats {
        manifest: digest,
        files: n_files,
        total_chunks,
        new_chunks: nc,
        dedup_pct: 100.0 * (total_chunks - nc) as f64 / total_chunks.max(1) as f64,
        raw_mb: raw_bytes as f64 / 1e6,
        new_raw_mb: new_raw_bytes as f64 / 1e6,
        upload_mb: comp_new as f64 / 1e6,
        zstd_ratio_on_new: new_raw_bytes as f64 / comp_new.max(1) as f64,
        blocks: n_blocks,
        symlinks: n_symlinks,
        skipped_special,
        read_hash_secs,
        compress_secs,
        pack_upload_secs,
        wall_secs: wall,
        hash_throughput_mbps: raw_bytes as f64 / 1e6 / read_hash_secs.max(1e-9),
        skipped_files: 0,
        skipped_mb: 0.0,
        stat_cache: crate::stat_cache::StatCache::default(),
        manifest_obj: None,
        compress_throughput_mbps: new_raw_bytes as f64 / 1e6 / compress_secs.max(1e-9),
    })
}

/// BOUNDED-PARALLEL publish (E6 + O6): a producer streams+dedups files and round-robins NEW chunks
/// through `workers` compressor threads into a single packer; the packer finalizes 64 MiB blocks and
/// round-robins them to `workers` UPLOADER threads that `put_block` concurrently (S3 publish is
/// upload-bound). Identity is the logical digest (packing/upload order irrelevant), so this is safe.
///
/// Peak memory (bounded regardless of tree size): the chunk-stage channels hold `~(workers+2)·cap`
/// 256 KiB chunks; the upload stage holds at most **~2·workers 64 MiB BLOCKS** in flight (each
/// uploader: 1 queued + 1 uploading — the upload channel bound is deliberately 1, NOT `cap`, because
/// its items are whole blocks). So block-tier peak ≈ `2·workers·64 MiB` — scales with `workers` (the
/// daemon clamps it to `available_parallelism`, so a 2–4 vCPU pod holds ~256–512 MiB, not GiBs).
pub fn publish_pipelined(
    root: &Path,
    store: &dyn BlobStore,
    known: &ChunkIndex,
    parent: Option<String>,
    workers: usize,
    cap: usize,
    prev: Option<PrevPublish<'_>>,
) -> Result<PublishStats> {
    use std::io::Read;
    use std::sync::mpsc::sync_channel;
    let workers = workers.max(1);
    let t0 = Instant::now();
    // Wall-clock BEFORE the walk — the racy-window guard for the NEXT publish compares file mtimes against
    // this, so it must pre-date every stat we are about to take.
    let scan_started_ns = crate::stat_cache::now_unix_ns();
    let mut new_cache = crate::stat_cache::StatCache::new(scan_started_ns);
    let mut skipped_files = 0usize;
    let mut skipped_bytes = 0u64;
    // Parent manifest's regular files by path — the ONLY source of reused chunk lists.
    let prev_by_path: HashMap<&str, &FileEntry> = prev
        .map(|p| {
            p.manifest
                .files
                .iter()
                .filter(|f| f.symlink.is_none() && f.hardlink.is_none())
                .map(|f| (f.path.as_str(), f))
                .collect()
        })
        .unwrap_or_default();
    let (mut files, symlink_entries, skipped_special) = walk_entries(root)?;
    files.sort();
    let n_symlinks = symlink_entries
        .iter()
        .filter(|f| f.symlink.is_some())
        .count();

    let mut file_entries: Vec<FileEntry> = Vec::with_capacity(files.len());
    let mut total_chunks = 0usize;
    let mut raw_bytes = 0u64;
    let mut new_chunks = 0usize;
    let mut new_raw_bytes = 0u64;

    let (new_index, n_blocks, comp_new) = std::thread::scope(|scope| -> Result<_> {
        let (comp_tx, comp_rx) = sync_channel::<(ChunkId, Vec<u8>, u32)>(cap.max(1) * workers);
        let mut raw_txs = Vec::with_capacity(workers);
        let mut chandles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let (rtx, rrx) = sync_channel::<(ChunkId, Vec<u8>)>(cap.max(1));
            raw_txs.push(rtx);
            let ctx = comp_tx.clone();
            chandles.push(scope.spawn(move || {
                while let Ok((id, raw)) = rrx.recv() {
                    let comp = zstd::stream::encode_all(raw.as_slice(), ZSTD_LEVEL).expect("zstd");
                    let _ = ctx.send((id, comp, raw.len() as u32));
                }
            }));
        }
        drop(comp_tx);

        // UPLOAD stage: N uploader threads pull finalized blocks and `put_block` them CONCURRENTLY
        // (S3 publish is upload-bound — the packer was previously the serial upload bottleneck). Each
        // has its own channel (mpsc rx isn't shareable); the packer round-robins blocks across them.
        // `put_block` is thread-safe (S3 adapter shares a multi-thread runtime; LocalBlobStore uses a
        // per-writer unique temp + atomic rename), so concurrent uploads are safe.
        let mut up_txs = Vec::with_capacity(workers);
        let mut uhandles = Vec::with_capacity(workers);
        for _ in 0..workers {
            // Bound = 1 (NOT `cap`): items are whole 64 MiB blocks, so a per-uploader queue of 1 caps
            // block-tier peak to ~2·workers blocks (queued + in-flight) instead of `cap`·workers.
            let (utx, urx) = sync_channel::<(BlockId, Vec<u8>)>(1);
            up_txs.push(utx);
            uhandles.push(scope.spawn(move || -> Result<()> {
                while let Ok((bid, bytes)) = urx.recv() {
                    store.put_block(&bid, &bytes)?;
                }
                Ok(())
            }));
        }

        let packer = scope.spawn(move || -> Result<(ChunkIndex, usize, u64)> {
            let mut new_index = ChunkIndex::new();
            let mut cur = BlockBuilder::new();
            let mut n_blocks = 0usize;
            let mut comp_new = 0u64;
            let mut brr = 0usize; // block round-robin cursor across uploaders
            // Finalize a full block, record its chunks in the index (what the manifest needs — does
            // NOT depend on the upload landing), then hand (id, bytes) to an uploader.
            fn flush_block(
                cur: &mut BlockBuilder,
                ni: &mut ChunkIndex,
                nb: &mut usize,
                brr: &mut usize,
                up_txs: &[std::sync::mpsc::SyncSender<(BlockId, Vec<u8>)>],
            ) -> Result<()> {
                if cur.buf.is_empty() {
                    return Ok(());
                }
                let (bid, bytes, members) = std::mem::replace(cur, BlockBuilder::new()).finalize();
                for (id, off, clen, rlen) in members {
                    ni.insert(
                        id,
                        ChunkLoc {
                            block: bid.clone(),
                            offset: off,
                            clen,
                            rlen,
                        },
                    );
                }
                up_txs[*brr % up_txs.len()]
                    .send((bid, bytes))
                    .map_err(|_| anyhow::anyhow!("uploader terminated mid-publish"))?;
                *brr += 1;
                *nb += 1;
                Ok(())
            }
            while let Ok((id, comp, rlen)) = comp_rx.recv() {
                comp_new += comp.len() as u64;
                let off = cur.buf.len() as u64;
                let clen = comp.len() as u32;
                cur.buf.extend_from_slice(&comp);
                cur.members.push((id, off, clen, rlen));
                if cur.buf.len() >= BLOCK_TARGET {
                    flush_block(&mut cur, &mut new_index, &mut n_blocks, &mut brr, &up_txs)?;
                }
            }
            flush_block(&mut cur, &mut new_index, &mut n_blocks, &mut brr, &up_txs)?;
            drop(up_txs); // close upload channels → uploaders drain + finish
            Ok((new_index, n_blocks, comp_new))
        });

        // producer (this thread): stream + dedup + round-robin NEW chunks
        let mut seen: std::collections::HashSet<ChunkId> = std::collections::HashSet::new();
        let mut buf = vec![0u8; CHUNK];
        let mut rr = 0usize;
        for p in &files {
            let meta = std::fs::metadata(p)?;
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().to_string();

            // O10 — REUSE-WITHOUT-REHASH. Record this file's fingerprint for the next cycle, then decide
            // whether we can take its chunk list from the parent manifest instead of reading + sha256ing
            // it. A wrong skip is silent non-durability, so ALL FOUR must hold:
            //   1. `may_skip`  — fingerprint identical AND the file was quiescent before the previous scan
            //                    began (racy-window guard; see stat_cache).
            //   2. the PARENT MANIFEST has a regular-file entry for this exact path (chunks come from
            //      there, never from the cache).
            //   3. that entry's size equals what we just stat'd (cheap cross-check of the two sources).
            //   4. every reused chunk is already in `known` — so we can never emit a manifest referencing a
            //      chunk that isn't durable, regardless of what the caller passed as `known`.
            let key = crate::stat_cache::StatKey::from_metadata(&meta);
            new_cache.insert(rel.clone(), key);
            if let Some(pv) = prev {
                if pv.cache.may_skip(&rel, &key) {
                    if let Some(pe) = prev_by_path.get(rel.as_str()) {
                        if pe.size == meta.len() && pe.chunks.iter().all(|c| known.contains_key(c))
                        {
                            total_chunks += pe.chunks.len();
                            raw_bytes += pe.size;
                            skipped_files += 1;
                            skipped_bytes += pe.size;
                            file_entries.push(FileEntry {
                                path: rel,
                                mode: meta.permissions().mode(),
                                size: meta.len(),
                                chunks: pe.chunks.clone(),
                                symlink: None,
                                hardlink: None,
                            });
                            continue; // not opened, not read, not hashed
                        }
                    }
                }
            }

            let mut fh = std::fs::File::open(p)?;
            let mut ids: Vec<ChunkId> = Vec::new();
            loop {
                let mut filled = 0;
                while filled < CHUNK {
                    match fh.read(&mut buf[filled..])? {
                        0 => break,
                        n => filled += n,
                    }
                }
                if filled == 0 {
                    break;
                }
                let chunk = &buf[..filled];
                total_chunks += 1;
                raw_bytes += filled as u64;
                let id = hex_sha256(chunk);
                ids.push(id.clone());
                if !known.contains_key(&id) && !seen.contains(&id) {
                    seen.insert(id.clone());
                    new_chunks += 1;
                    new_raw_bytes += filled as u64;
                    // clean error (not panic) if a compressor died; thread::scope still joins all.
                    raw_txs[rr % workers]
                        .send((id, chunk.to_vec()))
                        .map_err(|_| anyhow::anyhow!("compressor worker terminated mid-publish"))?;
                    rr += 1;
                }
                if filled < CHUNK {
                    break;
                }
            }
            file_entries.push(FileEntry {
                path: rel,
                mode: meta.permissions().mode(),
                size: meta.len(),
                chunks: ids,
                symlink: None,
                hardlink: None,
            });
        }
        drop(raw_txs); // close raw channels -> compressors finish -> packer finishes
        for h in chandles {
            h.join().ok();
        }
        // packer built the index + handed every block to the uploaders; then the uploaders drain.
        let packed = packer.join().unwrap()?;
        for h in uhandles {
            h.join().unwrap()?; // propagate any upload error — all blocks are durable once these return
        }
        Ok(packed)
    })?;

    file_entries.extend(symlink_entries);
    file_entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut index: ChunkIndex = ChunkIndex::new();
    for f in &file_entries {
        for cid in &f.chunks {
            if index.contains_key(cid) {
                continue;
            }
            let loc = new_index
                .get(cid)
                .or_else(|| known.get(cid))
                .ok_or_else(|| anyhow::anyhow!("referenced chunk {} missing", cid))?;
            index.insert(cid.clone(), loc.clone());
        }
    }
    let manifest = Manifest {
        parent,
        files: file_entries,
        chunks: index,
    };
    let digest = manifest.logical_digest();
    store.put_manifest(&digest, &manifest.to_bytes())?;
    // Bind the fingerprints to the manifest they describe — `PrevPublish::new` refuses any other pairing.
    new_cache.manifest_digest = digest.clone();
    let wall = t0.elapsed().as_secs_f64();
    Ok(PublishStats {
        manifest: digest,
        files: manifest.files.len(),
        total_chunks,
        new_chunks,
        dedup_pct: 100.0 * (total_chunks - new_chunks) as f64 / total_chunks.max(1) as f64,
        raw_mb: raw_bytes as f64 / 1e6,
        new_raw_mb: new_raw_bytes as f64 / 1e6,
        upload_mb: comp_new as f64 / 1e6,
        zstd_ratio_on_new: new_raw_bytes as f64 / comp_new.max(1) as f64,
        blocks: n_blocks,
        symlinks: n_symlinks,
        skipped_special,
        read_hash_secs: wall,
        compress_secs: wall,
        pack_upload_secs: 0.0,
        wall_secs: wall,
        hash_throughput_mbps: raw_bytes as f64 / 1e6 / wall.max(1e-9),
        skipped_files,
        skipped_mb: skipped_bytes as f64 / 1e6,
        stat_cache: new_cache,
        manifest_obj: Some(manifest),
        compress_throughput_mbps: new_raw_bytes as f64 / 1e6 / wall.max(1e-9),
    })
}

#[derive(Debug, Default, serde::Serialize)]
pub struct MaterializeStats {
    pub files: usize,
    pub blocks_fetched: usize,
    /// files served from a reference tree via reflink/copy (incremental resume) instead of
    /// fetch+decompress+write — 0 for a full materialize.
    pub reference_reused: usize,
    pub fetch_secs: f64,
    pub decompress_secs: f64,
    pub write_secs: f64,
    /// wall time of the (parallel) reflink pass — 0 for a full materialize. Separated from `write_secs`
    /// so a reference-backed resume shows where its time actually goes (CoW clone vs store fetch).
    pub reflink_secs: f64,
    pub wall_secs: f64,
    pub write_throughput_mbps: f64,
}

/// Fetch (parallel) the blocks that `regulars` need, decompress+VERIFY their chunks (content hash ==
/// id, bounded, rlen), and write the files (parallel, phase-1 regular-file write with size validation
/// + setuid/setgid/sticky masking). Shared by `materialize` (all regulars) and `materialize_incremental`
/// (only the changed regulars). Returns (blocks_fetched, fetch_secs, decompress_secs, write_secs).
fn write_regular_files(
    store: &dyn BlobStore,
    manifest: &Manifest,
    out: &Path,
    regulars: &[&FileEntry],
) -> Result<(usize, f64, f64, f64)> {
    let mut needed: Vec<BlockId> = Vec::new();
    for f in regulars {
        for cid in &f.chunks {
            let loc = manifest
                .chunks
                .get(cid)
                .ok_or_else(|| anyhow::anyhow!("manifest missing chunk index for {}", cid))?;
            needed.push(loc.block.clone());
        }
    }
    needed.sort();
    needed.dedup();

    let t_f = Instant::now();
    let block_bytes: HashMap<BlockId, Vec<u8>> = needed
        .par_iter()
        .map(|b| -> Result<_> { Ok((b.clone(), store.get_block(b)?)) })
        .collect::<Result<HashMap<_, _>>>()?;
    let fetch_secs = t_f.elapsed().as_secs_f64();

    let mut uniq: Vec<ChunkId> = regulars
        .iter()
        .flat_map(|f| f.chunks.iter().cloned())
        .collect();
    uniq.sort();
    uniq.dedup();
    let t_d = Instant::now();
    let chunk_bytes: HashMap<ChunkId, Vec<u8>> = uniq
        .par_iter()
        .map(|cid| -> Result<(ChunkId, Vec<u8>)> {
            let loc = manifest
                .chunks
                .get(cid)
                .ok_or_else(|| anyhow::anyhow!("missing chunk index"))?;
            let blk = block_bytes
                .get(&loc.block)
                .ok_or_else(|| anyhow::anyhow!("missing block"))?;
            let end = loc.offset as usize + loc.clen as usize;
            if end > blk.len() {
                anyhow::bail!("chunk span out of block bounds");
            }
            let raw = decompress_bounded(&blk[loc.offset as usize..end], CHUNK)?;
            if raw.len() != loc.rlen as usize {
                anyhow::bail!("chunk rlen mismatch (corruption)");
            }
            if hex_sha256(&raw) != *cid {
                anyhow::bail!("chunk hash != id (corruption/tamper)");
            }
            Ok((cid.clone(), raw))
        })
        .collect::<Result<HashMap<_, _>>>()?;
    let decompress_secs = t_d.elapsed().as_secs_f64();

    let t_w = Instant::now();
    regulars.par_iter().try_for_each(|f| -> Result<()> {
        safe_rel_path(&f.path)?;
        let p = out.join(&f.path);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let actual: usize = f
            .chunks
            .iter()
            .map(|cid| chunk_bytes.get(cid).map(|b| b.len()).unwrap_or(0))
            .sum();
        if f.size as usize != actual {
            anyhow::bail!(
                "file size field does not match chunk content ({} vs {})",
                f.size,
                actual
            );
        }
        let mut buf = Vec::with_capacity(actual);
        for cid in &f.chunks {
            buf.extend_from_slice(
                chunk_bytes
                    .get(cid)
                    .ok_or_else(|| anyhow::anyhow!("missing chunk"))?,
            );
        }
        std::fs::write(&p, &buf)?;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(f.mode & 0o777))?;
        Ok(())
    })?;
    Ok((
        needed.len(),
        fetch_secs,
        decompress_secs,
        t_w.elapsed().as_secs_f64(),
    ))
}

/// Recreate the hardlinks (phase 2) then symlinks (phase 3) of `manifest` into `out`. Symlinks LAST so
/// a malicious symlink can never become an ancestor of a regular-file write (write-through-escape).
fn write_links(manifest: &Manifest, out: &Path) -> Result<()> {
    for f in manifest.files.iter().filter(|f| f.hardlink.is_some()) {
        safe_rel_path(&f.path)?;
        let target = f.hardlink.as_ref().unwrap();
        safe_rel_path(target)?;
        let p = out.join(&f.path);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::hard_link(out.join(target), &p)?;
    }
    for f in manifest.files.iter().filter(|f| f.symlink.is_some()) {
        safe_rel_path(&f.path)?;
        let p = out.join(&f.path);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::os::unix::fs::symlink(f.symlink.as_ref().unwrap(), &p)?;
    }
    Ok(())
}

/// Verify a manifest's integrity + that `out` is a fresh empty dir (or create it). Shared preamble.
fn load_verified_manifest(store: &dyn BlobStore, digest: &str, out: &Path) -> Result<Manifest> {
    let manifest = Manifest::from_bytes(&store.get_manifest(digest)?)?;
    if manifest.logical_digest() != digest {
        anyhow::bail!("manifest digest mismatch (corruption/tamper)");
    }
    if out.exists() {
        let nonempty = std::fs::read_dir(out)
            .map(|mut it| it.next().is_some())
            .unwrap_or(true);
        if nonempty {
            anyhow::bail!("materialize target is not empty: {}", out.display());
        }
    } else {
        std::fs::create_dir_all(out)?;
    }
    Ok(manifest)
}

/// Materialize a manifest into `out` (a FRESH empty dir). Fetches each needed block once (parallel),
/// decompresses+verifies chunks (parallel), writes regulars (parallel), then hardlinks, then symlinks
/// LAST (write-through-symlink-escape safety). The daemon's COLD read path.
pub fn materialize(
    store: &dyn BlobStore,
    manifest_digest: &str,
    out: &Path,
) -> Result<MaterializeStats> {
    let t0 = Instant::now();
    let manifest = load_verified_manifest(store, manifest_digest, out)?;
    let regulars: Vec<&FileEntry> = manifest
        .files
        .iter()
        .filter(|f| f.symlink.is_none() && f.hardlink.is_none())
        .collect();
    let (blocks, fetch_secs, decompress_secs, write_secs) =
        write_regular_files(store, &manifest, out, &regulars)?;
    write_links(&manifest, out)?;
    let total: u64 = manifest.files.iter().map(|f| f.size).sum();
    Ok(MaterializeStats {
        files: manifest.files.len(),
        blocks_fetched: blocks,
        reference_reused: 0,
        fetch_secs,
        decompress_secs,
        write_secs,
        reflink_secs: 0.0,
        wall_secs: t0.elapsed().as_secs_f64(),
        write_throughput_mbps: total as f64 / 1e6 / write_secs.max(1e-9),
    })
}

/// INCREMENTAL materialize (warm resume): like `materialize`, but for every regular file whose content
/// is UNCHANGED from a reference materialization (same path, byte-identical chunk list), **reflink** it
/// from `ref_dir` (O(1) CoW clone on XFS/APFS; transparent copy fallback elsewhere) instead of
/// fetch+decompress+write. Only the CHANGED/new regular files hit the block store — so a warm node
/// resuming to the next generation pays ~the delta, not the whole tree.
///
/// SAFETY CONTRACT: `ref_dir` must be an UNMODIFIED prior `materialize`/`materialize_incremental` output
/// of `ref_manifest_digest` (it was content-verified when written). The caller owns keeping it clean —
/// if the agent may have mutated it, do a full `materialize` instead. `out` must be a fresh empty dir
/// (distinct from `ref_dir`).
pub fn materialize_incremental(
    store: &dyn BlobStore,
    manifest_digest: &str,
    out: &Path,
    ref_manifest_digest: &str,
    ref_dir: &Path,
) -> Result<MaterializeStats> {
    let t0 = Instant::now();
    let manifest = load_verified_manifest(store, manifest_digest, out)?;
    // The reference manifest is verified too (so its chunk lists are trustworthy for comparison).
    let ref_manifest = Manifest::from_bytes(&store.get_manifest(ref_manifest_digest)?)?;
    if ref_manifest.logical_digest() != ref_manifest_digest {
        anyhow::bail!("reference manifest digest mismatch");
    }
    let ref_by_path: HashMap<&str, &FileEntry> = ref_manifest
        .files
        .iter()
        .filter(|f| f.symlink.is_none() && f.hardlink.is_none())
        .map(|f| (f.path.as_str(), f))
        .collect();

    // Partition new regular files by MANIFEST ONLY (pure comparison, no I/O): `candidates` are unchanged
    // vs the reference and can be reflinked; the rest must come from the store.
    let mut to_write: Vec<&FileEntry> = Vec::new();
    let mut candidates: Vec<&FileEntry> = Vec::new();
    for f in manifest
        .files
        .iter()
        .filter(|f| f.symlink.is_none() && f.hardlink.is_none())
    {
        if ref_by_path
            .get(f.path.as_str())
            .is_some_and(|rf| rf.chunks == f.chunks)
        {
            candidates.push(f);
        } else {
            to_write.push(f); // changed / new → materialize from the store
        }
    }
    // Validate every candidate path BEFORE any join (fail fast, cheap, sequential).
    for f in &candidates {
        safe_rel_path(&f.path)?;
    }

    // Reflink the unchanged files IN PARALLEL — the per-file work is lstat + create_dir_all + FICLONE +
    // chmod, which is syscall/IO-bound, so a sequential loop leaves the device idle (this pass dominated
    // the cold `--ref-dir` start before O9). Mirrors the rayon-parallel fetch/write path below. A file
    // whose reference leaf is missing or not a REAL regular file (defensive: never follow a symlink)
    // yields `Some(f)` and falls back to the store; a genuine IO error propagates rather than silently
    // degrading to a fetch.
    let t_reflink = Instant::now();
    let fallbacks: Vec<&FileEntry> = candidates
        .par_iter()
        .map(|f| -> Result<Option<&FileEntry>> {
            let src = ref_dir.join(&f.path);
            let src_ok = src
                .symlink_metadata()
                .map(|m| m.file_type().is_file())
                .unwrap_or(false);
            if !src_ok {
                return Ok(Some(*f));
            }
            let dst = out.join(&f.path);
            if let Some(d) = dst.parent() {
                std::fs::create_dir_all(d)?;
            }
            // reflink (CoW) if supported, else a full copy — either way, no network + no decompress.
            reflink_copy::reflink_or_copy(&src, &dst)?;
            std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(f.mode & 0o777))?;
            Ok(None)
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    let reflink_secs = t_reflink.elapsed().as_secs_f64();
    let reflinked = candidates.len() - fallbacks.len();
    to_write.extend(fallbacks); // reference leaf unusable → materialize from the store instead

    let (blocks, fetch_secs, decompress_secs, write_secs) =
        write_regular_files(store, &manifest, out, &to_write)?;
    // hardlinks + symlinks are always recreated (reflink N/A); still LAST for symlink-escape safety.
    write_links(&manifest, out)?;

    let total: u64 = manifest.files.iter().map(|f| f.size).sum();
    Ok(MaterializeStats {
        files: manifest.files.len(),
        blocks_fetched: blocks,
        reference_reused: reflinked,
        fetch_secs,
        decompress_secs,
        write_secs,
        reflink_secs,
        wall_secs: t0.elapsed().as_secs_f64(),
        write_throughput_mbps: total as f64 / 1e6 / write_secs.max(1e-9),
    })
}

/// How a reference-accelerated warm resume (`resume_via_reference`) built its pristine reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeKind {
    /// No usable prior reference — the pristine ref was built by a FULL materialize (cold node).
    ColdFull,
    /// Built INCREMENTALLY from a prior reference (reflink unchanged, fetch only the delta).
    WarmIncremental,
    /// A pristine ref for this exact digest already existed (restart at the same HEAD) — reused, 0 fetch.
    RefReused,
}

/// Stats from `resume_via_reference`, for the daemon log + tests.
#[derive(Debug, Clone, Copy)]
pub struct ResumeStats {
    pub kind: ResumeKind,
    /// Blocks fetched from the store to (re)build the pristine reference (0 when the ref was reused).
    pub ref_blocks_fetched: usize,
    /// Regular files reflinked (CoW, or copy-fallback) from the prior reference while building the new one.
    pub ref_reflinked: usize,
    /// Total files in the cloned live workspace (all regulars reflinked from the pristine ref).
    pub workspace_files: usize,
}

/// True iff `s` is a 64-char lowercase-hex sha256 digest — the ONLY shape we will `join` onto `ref_root`
/// or `remove_dir_all`. Guards path-injection via a corrupt `current` file or a stray dir name.
fn is_hex_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Read the committed current-reference digest from `ref_root/current` (None if absent/corrupt/non-hex).
fn read_current_ref(ref_root: &Path) -> Option<String> {
    let d = std::fs::read_to_string(ref_root.join("current")).ok()?;
    let d = d.trim().to_string();
    is_hex_digest(&d).then_some(d)
}

/// Commit `ref_root/current = digest` atomically (write staging file, then rename).
fn write_current_ref(ref_root: &Path, digest: &str) -> Result<()> {
    let tmp = ref_root.join(".current.tmp");
    std::fs::write(&tmp, digest)?;
    std::fs::rename(&tmp, ref_root.join("current"))?;
    Ok(())
}

/// Sweep crash-residue staging entries (`.tmp-*` dirs, `.current.tmp`) under `ref_root`. Best-effort.
fn sweep_ref_staging(ref_root: &Path) {
    if let Ok(rd) = std::fs::read_dir(ref_root) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with(".tmp-") {
                let _ = std::fs::remove_dir_all(e.path());
            } else if n == ".current.tmp" {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Filesystem-safe, injection-proof subdirectory name for a lineage's reference tree, so multiple
/// lineages can safely share one `--ref-dir` without clobbering each other's refs or GC. `LineageId` is a
/// free-form String (it may contain `/` or `..`), so we HASH it rather than trust it as a path component.
pub fn lineage_ref_subdir(lineage: &str) -> String {
    // 16 hex of sha256 — ample against collision, and clearly not a 64-hex manifest digest.
    format!("lineage-{}", &hex_sha256(lineage.as_bytes())[..16])
}

/// A committed reference is trusted ONLY when its completeness sentinel `<digest>.ok` is a real FILE AND
/// `<digest>/` is a real DIRECTORY (both lstat'd — never a symlink). The sentinel is written as the LAST
/// step of a build, so a crash-partial or externally-planted `<digest>/` (no sentinel) is never reflinked
/// un-rehashed: this converts "a visible dir is complete" from an assumption into an enforced invariant.
fn ref_committed(ref_root: &Path, digest: &str) -> bool {
    let sentinel_ok = ref_root
        .join(format!("{digest}.ok"))
        .symlink_metadata()
        .map(|m| m.is_file())
        .unwrap_or(false);
    sentinel_ok
        && ref_root
            .join(digest)
            .symlink_metadata()
            .map(|m| m.is_dir())
            .unwrap_or(false)
}

/// Best-effort lstat-based removal of a path whatever it is (dir tree / file / symlink) — never follows a
/// symlink into another tree. Clears an incomplete/foreign `<digest>/` before a rebuild.
fn remove_any(p: &Path) {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => {
            let _ = std::fs::remove_dir_all(p);
        }
        Ok(_) => {
            let _ = std::fs::remove_file(p);
        }
        Err(_) => {}
    }
}

/// Remove every committed reference (dir + its `.ok` sentinel) under `ref_root` except `keep`
/// (best-effort, lstat-based). Safe: the new ref already inherited the shared extents via reflink, so CoW
/// keeps them alive — this reclaims only the delta.
fn gc_other_refs(ref_root: &Path, keep: &str) {
    if let Ok(rd) = std::fs::read_dir(ref_root) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if let Some(dig) = n.strip_suffix(".ok") {
                if is_hex_digest(dig) && dig != keep {
                    let _ = std::fs::remove_file(e.path());
                }
            } else if is_hex_digest(&n)
                && n != keep
                && e.path()
                    .symlink_metadata()
                    .map(|m| m.is_dir())
                    .unwrap_or(false)
            {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

/// Reflink-accelerated WARM RESUME for the daemon's materialize-on-start.
///
/// Maintains a daemon-owned **pristine reference** under `ref_root` (never touched by the agent) and hands
/// the agent a reflink-CLONE of it as the live `tree`. Layout under `ref_root`: `<digest>/` = an immutable
/// full materialization of that manifest; `<digest>.ok` = its completeness sentinel; `current` = the
/// committed current digest; `.tmp-*` = in-progress builds (atomically renamed on success, swept on start).
/// The next generation's reference is built INCREMENTALLY from the current one (reflink unchanged files,
/// fetch only the delta), so a warm node pays ~the delta; the clone into `tree` is all-reflink (0 fetched).
///
/// SAFETY: the reference is written by `materialize` (content-verified) and the agent only ever mutates
/// `tree` (a CoW clone) — so the reference's bytes always match its manifest and stay trustworthy for the
/// NEXT generation's chunk-list diff. A reference is trusted only once its sentinel is written LAST, so an
/// incomplete/foreign `<digest>/` is rebuilt, never reflinked blindly. `ref_root` must be single-writer
/// (per (lineage, node) — see `lineage_ref_subdir`); `tree` must be provided EMPTY/absent each start (same
/// contract as `materialize` — the daemon never clears the agent's workspace itself).
///
/// `ref_root` MUST be EPHEMERAL node-local storage (the same class as the NVMe block cache): the reflink
/// clone does NO content re-hash, so it trusts the sentinel. That is safe because a node power-event wipes
/// ephemeral storage → the reference is gone → cold resume from the durable store. It is NOT safe on
/// storage that can survive a power-loss with metadata (rename+sentinel) durable but file DATA un-flushed —
/// there is no fsync barrier yet (deferred). For the reflink win, `ref_root` must also be on the SAME
/// filesystem as `tree`; cross-fs falls back to a full copy (correct, but SLOWER than no reference at all).
pub fn resume_via_reference(
    store: &dyn BlobStore,
    target: &str,
    tree: &Path,
    ref_root: &Path,
) -> Result<ResumeStats> {
    // The target is the DB HEAD digest; validate it before it becomes a path component or the `current`
    // file (defense in depth — a non-hex value would also fail the manifest verification below).
    if !is_hex_digest(target) {
        anyhow::bail!("resume target is not a 64-hex digest: {target:?}");
    }
    std::fs::create_dir_all(ref_root)?;
    sweep_ref_staging(ref_root);

    let target_ref = ref_root.join(target);
    let (kind, ref_blocks_fetched, ref_reflinked) = if ref_committed(ref_root, target) {
        // Restart at a HEAD whose pristine ref is already COMMITTED (sentinel present) — reuse, no fetch.
        (ResumeKind::RefReused, 0, 0)
    } else {
        // A `<target>/` without a sentinel is incomplete/foreign — clear it (and any stale sentinel) first.
        remove_any(&target_ref);
        let _ = std::fs::remove_file(ref_root.join(format!("{target}.ok")));
        // Only a COMMITTED prior can seed the incremental build.
        let prior = read_current_ref(ref_root)
            .filter(|p| p.as_str() != target && ref_committed(ref_root, p))
            .map(|p| {
                let d = ref_root.join(&p);
                (p, d)
            });
        let tmp = ref_root.join(format!(".tmp-{target}"));
        let _ = std::fs::remove_dir_all(&tmp);
        let (kind, st) = match prior {
            Some((pdig, pdir)) => {
                match materialize_incremental(store, target, &tmp, &pdig, &pdir) {
                    Ok(st) => (ResumeKind::WarmIncremental, st),
                    Err(e) => {
                        // Prior ref unusable (e.g. its manifest was GC'd) → wipe partial tmp, full rebuild.
                        eprintln!(
                            "[daemon] incremental reference build failed ({e}); full materialize"
                        );
                        let _ = std::fs::remove_dir_all(&tmp);
                        (ResumeKind::ColdFull, materialize(store, target, &tmp)?)
                    }
                }
            }
            None => (ResumeKind::ColdFull, materialize(store, target, &tmp)?),
        };
        std::fs::rename(&tmp, &target_ref)?; // atomic commit of the reference BODY
        // Completeness sentinel LAST — only now will `ref_committed` trust `<target>/` on a later start.
        std::fs::write(ref_root.join(format!("{target}.ok")), b"")?;
        (kind, st.blocks_fetched, st.reference_reused)
    };

    write_current_ref(ref_root, target)?;

    // Clone the pristine reference into the agent's live workspace: ref_manifest == target ⇒ every regular
    // reflinks from the ref (0 blocks fetched), links recreated from the manifest. `tree` must be empty.
    let clone = materialize_incremental(store, target, tree, target, &target_ref)?;

    gc_other_refs(ref_root, target);
    Ok(ResumeStats {
        kind,
        ref_blocks_fetched,
        ref_reflinked,
        workspace_files: clone.files,
    })
}
