//! E9 — fixed-block vs content-defined (FastCDC) dedup across successive versions of a real
//! tree. Usage: cdcbench <avg_chunk_bytes> <version_dir_1> <version_dir_2> ...
//! Accumulates the global set of unique chunk hashes across all versions with each method and
//! reports total unique (stored) bytes — lower = better cross-version dedup.

use capsule_workspace_core::cas::hex_sha256;
use std::collections::HashSet;

fn main() {
    let mut args = std::env::args().skip(1);
    let avg: usize = args.next().unwrap().parse().unwrap();
    let (min, max) = (avg / 4, avg * 4);
    let dirs: Vec<String> = args.collect();

    let mut fixed: HashSet<String> = HashSet::new();
    let mut cdc: HashSet<String> = HashSet::new();
    let (mut fb, mut cb) = (0u64, 0u64);
    let mut raw_total = 0u64; // total incl. dups (raw material)

    for (v, dir) in dirs.iter().enumerate() {
        for e in walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !e.file_type().is_file() || e.path_is_symlink() {
                continue;
            }
            let data = match std::fs::read(e.path()) {
                Ok(d) => d,
                Err(_) => continue,
            };
            raw_total += data.len() as u64;
            // fixed
            for c in data.chunks(avg) {
                if fixed.insert(hex_sha256(c)) {
                    fb += c.len() as u64;
                }
            }
            // content-defined
            for ch in fastcdc::v2020::FastCDC::new(&data, min as u32, avg as u32, max as u32) {
                let seg = &data[ch.offset..ch.offset + ch.length];
                if cdc.insert(hex_sha256(seg)) {
                    cb += seg.len() as u64;
                }
            }
        }
        eprintln!(
            "  after v{:>2}: fixed_unique={:>7.1}MB  cdc_unique={:>7.1}MB",
            v,
            fb as f64 / 1e6,
            cb as f64 / 1e6
        );
    }
    let saved = 100.0 * (fb as f64 - cb as f64) / fb.max(1) as f64;
    println!(
        "avg={}K  raw_total={:.0}MB  FIXED_unique={:.1}MB  CDC_unique={:.1}MB  CDC_saves_vs_fixed={:.1}%",
        avg / 1024,
        raw_total as f64 / 1e6,
        fb as f64 / 1e6,
        cb as f64 / 1e6,
        saved
    );
}
