//! O7 — incremental warm resume via reflink. `materialize_incremental` reflinks (CoW clone; or copy
//! fallback) every regular file whose content is unchanged from a reference materialization, so only
//! the DELTA hits the block store. Here we assert the behavior (reference reuse + fetch only the
//! changed blocks + byte-identical output + a mutated/absent reference file falls back safely). The
//! wall-clock O(1)-reflink win is measured on real XFS in the EC2 batch.

use capsule_workspace_core::cas::*;
use capsule_workspace_core::daemon::{materialize, materialize_incremental, publish};
use capsule_workspace_core::manifest::Manifest;
use std::fs;
use std::path::Path;

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}
fn write(p: &Path, b: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b).unwrap();
}
fn prng(seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_add(0x9E3779B97F4A7C15);
    let mut out = Vec::with_capacity(CHUNK);
    while out.len() < CHUNK {
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;
        out.extend_from_slice(&z.to_le_bytes());
        x = x.wrapping_add(0x9E3779B97F4A7C15);
    }
    out.truncate(CHUNK);
    out
}
fn load_index(s: &LocalBlobStore, dig: &str) -> ChunkIndex {
    Manifest::from_bytes(&s.get_manifest(dig).unwrap())
        .unwrap()
        .chunks
}

#[test]
fn incremental_reflinks_unchanged_fetches_only_delta() {
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let tree = d.path().join("t");
    write(&tree.join("a.bin"), &prng(1));
    write(&tree.join("dir/b.bin"), &prng(2));
    write(&tree.join("c.bin"), &prng(3));
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;

    // clean reference = a full materialize of gen1 (content-verified when written).
    let refdir = d.path().join("ref");
    materialize(&s, &g1, &refdir).unwrap();

    // gen2: change b.bin, ADD e.bin, keep a.bin + c.bin. Dedup vs gen1.
    write(&tree.join("dir/b.bin"), &prng(99));
    write(&tree.join("e.bin"), &prng(50));
    let g2 = publish(&tree, &s, &load_index(&s, &g1), Some(g1.clone()))
        .unwrap()
        .manifest;

    let out = d.path().join("out");
    let st = materialize_incremental(&s, &g2, &out, &g1, &refdir).unwrap();

    // a.bin + c.bin unchanged → reflinked from the reference (not fetched). b.bin (changed) + e.bin
    // (new) → materialized from the store.
    assert_eq!(st.reference_reused, 2, "a.bin + c.bin reflinked");
    // only the delta's blocks were fetched — fewer than a full materialize of gen2.
    let full = {
        let o = d.path().join("full");
        materialize(&s, &g2, &o).unwrap().blocks_fetched
    };
    assert!(
        st.blocks_fetched < full,
        "incremental fetched fewer blocks ({}) than a full materialize ({full})",
        st.blocks_fetched
    );

    // byte-identical to gen2's tree.
    assert_eq!(fs::read(out.join("a.bin")).unwrap(), prng(1)); // reflinked, old content == new
    assert_eq!(fs::read(out.join("dir/b.bin")).unwrap(), prng(99)); // changed
    assert_eq!(fs::read(out.join("c.bin")).unwrap(), prng(3)); // reflinked
    assert_eq!(fs::read(out.join("e.bin")).unwrap(), prng(50)); // new
}

// Safety: a reference whose file was MUTATED on disk (chunk list still matches, bytes differ) is not
// re-verified by reflink — the contract is "clean reference". But a reference file that is ABSENT (or
// not a regular file) must fall back to materializing from the store, never crash or produce a wrong tree.
#[test]
fn incremental_falls_back_when_reference_file_absent() {
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let tree = d.path().join("t");
    write(&tree.join("a.bin"), &prng(7));
    write(&tree.join("b.bin"), &prng(8));
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;

    // reference dir is materialized, then a.bin is DELETED from it (a partial/broken reference).
    let refdir = d.path().join("ref");
    materialize(&s, &g1, &refdir).unwrap();
    fs::remove_file(refdir.join("a.bin")).unwrap();

    // same content (gen2 == gen1); incremental must still produce a correct tree by materializing the
    // missing-from-reference file.
    let out = d.path().join("out");
    let st = materialize_incremental(&s, &g1, &out, &g1, &refdir).unwrap();
    assert_eq!(
        st.reference_reused, 1,
        "only b.bin was reflinkable (a.bin absent from ref)"
    );
    assert_eq!(fs::read(out.join("a.bin")).unwrap(), prng(7)); // materialized from the store
    assert_eq!(fs::read(out.join("b.bin")).unwrap(), prng(8)); // reflinked
}

// content SAME, mode CHANGED → reflinked (0 blocks fetched) with the NEW mode applied.
#[test]
fn incremental_applies_new_mode_on_content_match() {
    use std::os::unix::fs::PermissionsExt;
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let tree = d.path().join("t");
    write(&tree.join("run.sh"), &prng(5));
    fs::set_permissions(tree.join("run.sh"), fs::Permissions::from_mode(0o644)).unwrap();
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let refdir = d.path().join("ref");
    materialize(&s, &g1, &refdir).unwrap();

    fs::set_permissions(tree.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    let g2 = publish(&tree, &s, &load_index(&s, &g1), Some(g1.clone()))
        .unwrap()
        .manifest;
    let out = d.path().join("out");
    let st = materialize_incremental(&s, &g2, &out, &g1, &refdir).unwrap();
    assert_eq!(st.reference_reused, 1, "content unchanged → reflinked");
    assert_eq!(st.blocks_fetched, 0, "reflink only — nothing fetched");
    assert_eq!(
        fs::metadata(out.join("run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755,
        "the NEW mode is applied after reflink"
    );
}

// a tampered new manifest with a traversal path must be refused by the incremental entry point too.
#[test]
fn incremental_refuses_path_traversal() {
    use capsule_workspace_core::manifest::Manifest;
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let tree = d.path().join("t");
    write(&tree.join("f.txt"), b"x");
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let refdir = d.path().join("ref");
    materialize(&s, &g1, &refdir).unwrap();

    let mut m = Manifest::from_bytes(&s.get_manifest(&g1).unwrap()).unwrap();
    m.files[0].path = "../../ESCAPED".to_string();
    let bad = m.logical_digest();
    fs::write(
        d.path().join("s").join("manifests").join(&bad),
        m.to_bytes(),
    )
    .unwrap();
    let out = d.path().join("out");
    assert!(
        materialize_incremental(&s, &bad, &out, &g1, &refdir).is_err(),
        "traversal must be refused"
    );
    assert!(!d.path().join("ESCAPED").exists() && !d.path().join("../ESCAPED").exists());
}

// hardlinks + symlinks are recreated (never reflinked) and are correct under the incremental path,
// even when their canonical target was REFLINKED (write_links runs after reflink + delta write).
#[test]
fn incremental_recreates_links_over_reflinked_target() {
    use std::os::unix::fs::{symlink, MetadataExt};
    let d = tmp();
    let s = LocalBlobStore::new(d.path().join("s")).unwrap();
    let tree = d.path().join("t");
    write(&tree.join("canon.bin"), &prng(11));
    fs::hard_link(tree.join("canon.bin"), tree.join("hl.bin")).unwrap();
    symlink("canon.bin", tree.join("ln")).unwrap();
    write(&tree.join("x.bin"), &prng(12));
    let g1 = publish(&tree, &s, &ChunkIndex::new(), None)
        .unwrap()
        .manifest;
    let refdir = d.path().join("ref");
    materialize(&s, &g1, &refdir).unwrap();

    // gen2: change x.bin; canon/hl/ln unchanged.
    write(&tree.join("x.bin"), &prng(99));
    let g2 = publish(&tree, &s, &load_index(&s, &g1), Some(g1.clone()))
        .unwrap()
        .manifest;
    let out = d.path().join("out");
    let st = materialize_incremental(&s, &g2, &out, &g1, &refdir).unwrap();
    assert!(st.reference_reused >= 1, "canon.bin reflinked");
    // the hardlink shares the reflinked canonical target's inode
    assert_eq!(
        fs::metadata(out.join("canon.bin")).unwrap().ino(),
        fs::metadata(out.join("hl.bin")).unwrap().ino(),
        "hardlink shares the reflinked target's inode"
    );
    assert_eq!(
        fs::read_link(out.join("ln")).unwrap().to_str().unwrap(),
        "canon.bin"
    );
    assert_eq!(fs::read(out.join("x.bin")).unwrap(), prng(99));
    assert_eq!(fs::read(out.join("canon.bin")).unwrap(), prng(11));
}
