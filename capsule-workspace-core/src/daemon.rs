//! Node-daemon core: publish (freeze->chunk->dedup->pack->upload->manifest) and
//! materialize (manifest->fetch blocks->decompress->write tree). Rayon-parallel on the
//! CPU-bound stages so this measures the REAL throughput the Python prototype could not.

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
    pub read_hash_secs: f64,
    pub compress_secs: f64,
    pub pack_upload_secs: f64,
    pub wall_secs: f64,
    pub hash_throughput_mbps: f64,
    pub compress_throughput_mbps: f64,
}

fn list_files(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && !e.path_is_symlink())
        .map(|e| e.into_path())
        .collect()
}

/// Publish a tree. `known` is the chunk index already durable (the dedup set the real
/// daemon consults from the lineage). Returns the new manifest digest + stats.
pub fn publish(
    root: &Path,
    store: &dyn BlobStore,
    known: &ChunkIndex,
    parent: Option<String>,
) -> Result<PublishStats> {
    let t0 = Instant::now();
    let files = list_files(root);

    // Stage A (parallel): read + split + hash every file. Bytes held only until dedup.
    let t_a = Instant::now();
    let per_file: Vec<(FileEntry, Vec<(ChunkId, Vec<u8>)>)> = files
        .par_iter()
        .map(|p| -> Result<_> {
            let meta = std::fs::metadata(p)?;
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().to_string();
            let raw = std::fs::read(p)?;
            let mut ids = Vec::new();
            let mut chunks = Vec::new();
            for c in raw.chunks(CHUNK) {
                let id = hex_sha256(c);
                ids.push(id.clone());
                chunks.push((id, c.to_vec()));
            }
            let entry = FileEntry {
                path: rel,
                mode: meta.permissions().mode(),
                size: meta.len(),
                chunks: ids,
            };
            Ok((entry, chunks))
        })
        .collect::<Result<Vec<_>>>()?;
    let read_hash_secs = t_a.elapsed().as_secs_f64();

    // Dedup (sequential, cheap): first occurrence of each new chunk keeps its raw bytes.
    let mut raw_bytes: u64 = 0;
    let mut total_chunks = 0usize;
    let mut new_raw: HashMap<ChunkId, Vec<u8>> = HashMap::new();
    let mut file_entries = Vec::with_capacity(per_file.len());
    for (entry, chunks) in per_file {
        for (id, bytes) in chunks {
            total_chunks += 1;
            raw_bytes += bytes.len() as u64;
            if !known.contains_key(&id) && !new_raw.contains_key(&id) {
                new_raw.insert(id, bytes);
            }
        }
        file_entries.push(entry);
    }
    let new_unique: Vec<(ChunkId, Vec<u8>)> = new_raw.into_iter().collect();
    let new_raw_bytes: u64 = new_unique.iter().map(|(_, b)| b.len() as u64).sum();

    // Stage B (parallel): compress ONLY new chunks — the expensive CPU stage.
    let t_b = Instant::now();
    let compressed: Vec<(ChunkId, Vec<u8>, u32)> = new_unique
        .par_iter()
        .map(|(id, raw)| {
            let comp = zstd::stream::encode_all(raw.as_slice(), ZSTD_LEVEL).expect("zstd");
            (id.clone(), comp, raw.len() as u32)
        })
        .collect();
    let compress_secs = t_b.elapsed().as_secs_f64();
    let comp_new: u64 = compressed.iter().map(|(_, c, _)| c.len() as u64).sum();

    // Pack into ~64 MiB blocks, then upload blocks in parallel.
    let t_c = Instant::now();
    let mut new_index: ChunkIndex = HashMap::new();
    let mut blocks: Vec<(BlockId, Vec<u8>)> = Vec::new();
    let mut cur = BlockBuilder::new();
    for (id, comp, rlen) in compressed {
        let off = cur.buf.len() as u64;
        let clen = comp.len() as u32;
        cur.buf.extend_from_slice(&comp);
        cur.members.push((id, off, clen, rlen));
        if cur.buf.len() >= BLOCK_TARGET {
            let (bid, buf, members) = std::mem::replace(&mut cur, BlockBuilder::new()).finalize();
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
            blocks.push((bid, buf));
        }
    }
    if !cur.buf.is_empty() {
        let (bid, buf, members) = cur.finalize();
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
        blocks.push((bid, buf));
    }
    let n_blocks = blocks.len();
    blocks
        .par_iter()
        .try_for_each(|(bid, buf)| store.put_block(bid, buf))?;
    let pack_upload_secs = t_c.elapsed().as_secs_f64();

    // Manifest = full resolved index (known ∪ new), so a cold node needs only this + blocks.
    let mut index = known.clone();
    index.extend(new_index);
    let manifest = Manifest {
        parent,
        files: file_entries,
        chunks: index,
    };
    let mbytes = manifest.to_bytes();
    let digest = Manifest::digest(&mbytes);
    store.put_manifest(&digest, &mbytes)?;

    let wall = t0.elapsed().as_secs_f64();
    let nc = new_unique.len();
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

    // unique blocks needed
    let mut needed: Vec<BlockId> = manifest
        .files
        .iter()
        .flat_map(|f| f.chunks.iter())
        .map(|cid| manifest.chunks[cid].block.clone())
        .collect();
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
    let chunk_bytes: HashMap<ChunkId, Vec<u8>> = uniq
        .par_iter()
        .map(|cid| {
            let loc = &manifest.chunks[cid];
            let blk = &block_bytes[&loc.block];
            let comp = &blk[loc.offset as usize..loc.offset as usize + loc.clen as usize];
            (cid.clone(), decompress(comp))
        })
        .collect();
    let decompress_secs = t_d.elapsed().as_secs_f64();

    // write files (parallel across files)
    let t_w = Instant::now();
    let total: u64 = manifest.files.iter().map(|f| f.size).sum();
    manifest.files.par_iter().try_for_each(|f| -> Result<()> {
        let p = out.join(&f.path);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut buf = Vec::with_capacity(f.size as usize);
        for cid in &f.chunks {
            buf.extend_from_slice(&chunk_bytes[cid]);
        }
        std::fs::write(&p, &buf)?;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(f.mode))?;
        Ok(())
    })?;
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
