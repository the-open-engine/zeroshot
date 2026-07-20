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
}

/// Walk an opaque tree, classifying entries. Regular files carry content; symlinks are
/// preserved by target (no content); anything else (fifo/socket/device) is counted and
/// skipped rather than silently misrepresented as an empty file.
fn walk_entries(root: &Path) -> Result<(Vec<PathBuf>, Vec<FileEntry>, usize)> {
    use std::os::unix::fs::MetadataExt;
    let mut regulars = Vec::new();
    let mut symlinks = Vec::new();
    let mut skipped = 0usize;
    for e in walkdir::WalkDir::new(root).into_iter() {
        let e = e?;
        let rel = e
            .path()
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .to_string();
        if rel.is_empty() {
            continue; // root itself
        }
        let ft = e.file_type();
        if ft.is_dir() {
            continue; // dirs are implied by file paths (empty dirs are a known minor gap)
        } else if ft.is_symlink() {
            let target = std::fs::read_link(e.path())?.to_string_lossy().to_string();
            let mode = e
                .path()
                .symlink_metadata()
                .map(|m| m.mode())
                .unwrap_or(0o120777);
            symlinks.push(FileEntry {
                path: rel,
                mode,
                size: 0,
                chunks: vec![],
                symlink: Some(target),
            });
        } else if ft.is_file() {
            regulars.push(e.into_path());
        } else {
            skipped += 1; // fifo / socket / block / char device — not workspace content
        }
    }
    Ok((regulars, symlinks, skipped))
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
    let n_symlinks = symlink_entries.len();

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
    Ok(PublishStats {
        manifest: digest,
        files: manifest.files.len(),
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
        compress_throughput_mbps: new_raw_bytes as f64 / 1e6 / compress_secs.max(1e-9),
    })
}

#[derive(Debug, serde::Serialize)]
pub struct MaterializeStats {
    pub files: usize,
    pub blocks_fetched: usize,
    pub fetch_secs: f64,
    pub decompress_secs: f64,
    pub write_secs: f64,
    pub wall_secs: f64,
    pub write_throughput_mbps: f64,
}

/// Materialize a manifest into `out`. Fetches each needed block once (parallel), decompresses
/// unique chunks (parallel), writes files (parallel). Mirrors the daemon's read path.
pub fn materialize(
    store: &dyn BlobStore,
    manifest_digest: &str,
    out: &Path,
) -> Result<MaterializeStats> {
    let t0 = Instant::now();
    let manifest = Manifest::from_bytes(&store.get_manifest(manifest_digest)?)?;
    // INTEGRITY: the parsed manifest must hash (logically) to the requested digest. Detects a
    // corrupt/torn/tampered manifest. Safe because the digest comes from the trusted lineage
    // HEAD (fenced), never from tenant input.
    if manifest.logical_digest() != manifest_digest {
        anyhow::bail!("manifest digest mismatch (corruption/tamper)");
    }

    // `out` must be a FRESH empty dir. Materializing into a dir that already holds a symlink
    // (a leftover/reused scratch dir) would let a regular-file write escape THROUGH it (P5).
    // Also guarantees the bind-mount target exists even for an empty manifest (P9e).
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

    // unique blocks needed (panic-safe against an adversarial/corrupt manifest)
    let mut needed: Vec<BlockId> = Vec::new();
    for f in &manifest.files {
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

    // decompress each unique chunk once (parallel)
    let mut uniq: Vec<ChunkId> = manifest
        .files
        .iter()
        .flat_map(|f| f.chunks.iter().cloned())
        .collect();
    uniq.sort();
    uniq.dedup();
    let t_d = Instant::now();
    // Decompress + VERIFY each chunk: bounded (bomb defense) and content-hash == chunk id
    // and declared rlen (detects corruption / tampering; content addressing must be checked).
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

    // Write in two phases so a malicious symlink cannot become an ANCESTOR of a file write
    // (write-through-symlink escape): (1) all regular files into real dirs, then (2) symlinks.
    // Assumes `out` starts empty (the daemon materializes into a fresh dir per pod).
    let t_w = Instant::now();
    let total: u64 = manifest.files.iter().map(|f| f.size).sum();
    manifest
        .files
        .par_iter()
        .filter(|f| f.symlink.is_none())
        .try_for_each(|f| -> Result<()> {
            safe_rel_path(&f.path)?;
            let p = out.join(&f.path);
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            // Never trust the manifest `size` for allocation (P7). Size from the ACTUAL
            // authenticated chunk bytes, and require the declared size to match it.
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
            // mask setuid/setgid/sticky — never honor those from a workspace snapshot
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(f.mode & 0o777))?;
            Ok(())
        })?;
    for f in manifest.files.iter().filter(|f| f.symlink.is_some()) {
        safe_rel_path(&f.path)?;
        let p = out.join(&f.path);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::os::unix::fs::symlink(f.symlink.as_ref().unwrap(), &p)?;
    }
    let write_secs = t_w.elapsed().as_secs_f64();

    Ok(MaterializeStats {
        files: manifest.files.len(),
        blocks_fetched: needed.len(),
        fetch_secs,
        decompress_secs,
        write_secs,
        wall_secs: t0.elapsed().as_secs_f64(),
        write_throughput_mbps: total as f64 / 1e6 / write_secs.max(1e-9),
    })
}
