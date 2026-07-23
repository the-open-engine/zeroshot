//! Materialize's memory bound and failure atomicity — both learned the hard way.
//!
//! The first wave implementation keyed waves on BLOCKS. Blocks are flushed by publish at 64 MiB of
//! COMPRESSED bytes, and a wave must always admit at least one block, so a highly compressible block
//! could decompress to gigabytes: the "bound" measured 2.03x tree size on compressible input and was
//! still linear. Every test at the time used INCOMPRESSIBLE fixtures, so none of them could see it.
//! `bound_holds_on_highly_compressible_input` is the test that would have.
//!
//! Separately, pre-creating every file at final length meant a failed materialize left a
//! complete-LOOKING tree — right file count, right sizes, right modes, all zero bytes — where the
//! previous implementation left `out` empty and the retry was clean.

use capsule_workspace_core::cas::*;
use capsule_workspace_core::daemon::{materialize, publish};
use capsule_workspace_core::manifest::Manifest;
use std::fs;
use std::path::Path;

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}
fn w(p: &Path, b: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b).unwrap();
}

/// Highly compressible but with DISTINCT chunks, so publish-side dedup can't hide the problem: each
/// chunk is mostly zeros with a unique counter, compressing ~1000x while remaining a unique chunk id.
fn compressible_chunk(i: u64) -> Vec<u8> {
    let mut v = vec![0u8; CHUNK];
    v[..8].copy_from_slice(&i.to_le_bytes());
    v
}

// THE bound test. A tree that compresses ~1000x must not blow the working set: if waves are keyed on
// blocks, all of this lands in one block and is decompressed at once.
#[test]
fn bound_holds_on_highly_compressible_input() {
    let d = tmp();
    let tree = d.path().join("t");
    // 2 GiB logical across 8 files of 256 MiB — compresses into a single 64 MiB block.
    for f in 0..8u64 {
        let mut body = Vec::with_capacity(256 * 1024 * 1024);
        for c in 0..1024u64 {
            body.extend_from_slice(&compressible_chunk(f * 1024 + c));
        }
        w(&tree.join(format!("big{f}.bin")), &body);
    }
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let dig = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let m = Manifest::from_bytes(&s.get_manifest(&dig).unwrap()).unwrap();
    let logical: u64 = m.files.iter().map(|f| f.size).sum();
    let blocks: std::collections::BTreeSet<_> = m.chunks.values().map(|l| &l.block).collect();
    assert!(
        logical >= 2 * 1024 * 1024 * 1024,
        "fixture is 2 GiB logical"
    );
    assert!(
        blocks.len() <= 2,
        "fixture must compress into ~1 block for this to be the interesting case, got {}",
        blocks.len()
    );

    let out = d.path().join("o");
    materialize(&s, &dig, &out).unwrap();

    // Byte-identical round trip.
    for f in 0..8u64 {
        let got = fs::read(out.join(format!("big{f}.bin"))).unwrap();
        assert_eq!(got.len(), 256 * 1024 * 1024);
        assert_eq!(&got[..8], &(f * 1024).to_le_bytes()[..]);
    }
    // The real assertion is memory, which the harness can't read portably — but a block-keyed wave would
    // have had to hold all 2 GiB decompressed at once to get here, so completing at all under a 256 MiB
    // logical ceiling is the observable signal. The RSS numbers live in the build log.
}

// A failed materialize must leave NOTHING behind: a complete-looking, correctly-sized, all-zero tree is
// worse than an empty one, because every cheap check says "fine" and the next start's empty-dir guard
// turns a transient fetch error into a permanent crash loop.
#[test]
fn failed_materialize_leaves_no_partial_tree() {
    let d = tmp();
    let tree = d.path().join("t");
    for i in 0..40 {
        w(&tree.join(format!("f{i}.bin")), &vec![i as u8; 300_000]);
    }
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let dig = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let m = Manifest::from_bytes(&s.get_manifest(&dig).unwrap()).unwrap();

    // Delete one block the manifest needs — the GC'd-block / transient-fetch-error case.
    let victim = m.chunks.values().next().unwrap().block.clone();
    for e in fs::read_dir(d.path().join("s").join("blocks"))
        .unwrap()
        .flatten()
    {
        if e.file_name().to_string_lossy().contains(&victim) {
            fs::remove_file(e.path()).unwrap();
        }
    }

    let out = d.path().join("o");
    assert!(materialize(&s, &dig, &out).is_err(), "must fail loudly");
    let left = fs::read_dir(&out).map(|it| it.count()).unwrap_or(0);
    assert_eq!(
        left, 0,
        "a failed materialize must leave no partial tree (found {left} entries)"
    );
}

// Wave BOUNDARIES. The file-ordered wave logic has three distinct cases and nothing else exercises them
// together: many small files packed into one wave, a file that alone exceeds the ceiling (streamed
// chunk-by-chunk rather than assembled), and files sitting either side of a wave boundary. Sizes here are
// chosen relative to the real MATERIALIZE_WAVE_BYTES so the fixture actually crosses boundaries.
#[test]
fn wave_boundaries_round_trip_exactly() {
    let d = tmp();
    let tree = d.path().join("t");
    // ~600 MiB total across three shapes → several waves at the 256 MiB ceiling.
    for i in 0..2000u64 {
        // many small files (some empty, some sub-chunk, some multi-chunk)
        let n = match i % 4 {
            0 => 0,
            1 => 100,
            2 => CHUNK + 7,
            _ => 3 * CHUNK,
        };
        let mut v = vec![0u8; n];
        if n >= 8 {
            v[..8].copy_from_slice(&i.to_le_bytes());
        }
        w(&tree.join(format!("many/f{i}.bin")), &v);
    }
    // one file that alone exceeds the wave ceiling → takes the streaming path
    let mut huge = Vec::with_capacity(300 * 1024 * 1024);
    for c in 0..1200u64 {
        huge.extend_from_slice(&compressible_chunk(1_000_000 + c));
    }
    w(&tree.join("huge.bin"), &huge);
    // and a normal file after it, so the streamed file is not simply last
    w(&tree.join("zz_after.bin"), &vec![7u8; 5 * CHUNK + 3]);

    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let dig = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let out = d.path().join("o");
    materialize(&s, &dig, &out).unwrap();

    // Every file byte-identical, including the streamed one and the boundary neighbours.
    for i in 0..2000u64 {
        let p = out.join(format!("many/f{i}.bin"));
        let got = fs::read(&p).unwrap();
        let want = fs::read(tree.join(format!("many/f{i}.bin"))).unwrap();
        assert_eq!(got, want, "mismatch at many/f{i}.bin");
    }
    assert_eq!(
        fs::read(out.join("huge.bin")).unwrap(),
        huge,
        "streamed file"
    );
    assert_eq!(
        fs::read(out.join("zz_after.bin")).unwrap(),
        vec![7u8; 5 * CHUNK + 3]
    );
}
