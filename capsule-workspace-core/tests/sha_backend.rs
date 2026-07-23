//! Guards the sha2 `asm` feature, which selects the ARMv8 hardware SHA-256 backend.
//!
//! This test exists because losing that feature is INVISIBLE by construction: the digests are identical
//! either way, so every correctness test in this suite passes just as happily on the software path. The
//! only observable difference is throughput — and hashing is the dominant CPU cost of publish, and of the
//! full re-hash the periodic scrub performs. It was in fact shipped soft for this entire campaign, so
//! every measurement taken before this guard understated the system by ~5x.
//!
//! The floor is set well below hardware and well above software so it separates them with margin on slow
//! machines rather than being a benchmark.

use capsule_workspace_core::cas::hex_sha256;
use std::time::Instant;

#[test]
fn sha256_uses_the_hardware_backend() {
    let buf = vec![0x5au8; 1 << 20]; // 1 MiB
    // warm up (first call pays cpufeatures detection + page faults)
    let _ = hex_sha256(&buf);
    let n = 128;
    let t = Instant::now();
    for _ in 0..n {
        std::hint::black_box(hex_sha256(std::hint::black_box(&buf)));
    }
    let mbps = (n as f64) / t.elapsed().as_secs_f64();

    // Measured on aarch64: ~1765 MB/s hardware vs ~344 MB/s software (5.1x apart). 700 leaves 2x margin
    // below hardware and comfortably above software.
    const FLOOR_MBPS: f64 = 700.0;
    assert!(
        mbps > FLOOR_MBPS,
        "sha256 measured at {mbps:.0} MB/s, below the {FLOOR_MBPS:.0} MB/s floor.\n\
         The most likely cause is that the `asm` feature on the `sha2` dependency was dropped, which \
         silently selects the SOFTWARE backend on aarch64 (digests stay identical, so nothing else \
         fails). Check Cargo.toml. If this machine is genuinely just slow, say so explicitly rather \
         than lowering the floor."
    );
    println!("[sha] {mbps:.0} MB/s");
}
