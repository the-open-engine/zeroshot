//! Daemon lifecycle + MF3 (dedup-reuse survives GC) + MF4/stale-fence + health, driven through the
//! LIBRARY helpers the `daemon` subcommand uses (`daemon_loop::{materialize_on_start, publish_cycle,
//! spawn_health_server}`) over a `LocalBlobStore` (tempdir) + a REAL Postgres (feature `pg`).
//!
//! Gating mirrors `tests/pg_lineage.rs`:
//!   - `DATABASE_URL` set   → run against that Postgres (local Docker or a gated RDS).
//!   - `DATABASE_URL` unset → each DB test PRINTS a skip notice and passes (degraded coverage is
//!     reported, not silent — repo rule). The health tests need no DB and always run.
//!
//! Run locally:
//!   docker run -d --name capsule-pg-d -p 5434:5432 -e POSTGRES_PASSWORD=pg -e POSTGRES_DB=capsule \
//!     postgres:17.10-alpine
//!   DATABASE_URL=postgres://postgres:pg@localhost:5434/capsule?sslmode=disable \
//!     cargo test --features pg --release --test daemon -- --nocapture
//!
//! ISOLATION (one shared DB, tests run in parallel): each test uses (a) a PRIVATE `LocalBlobStore`
//! tempdir (block existence is per-test), (b) a UNIQUE lineage id, and (c) UNIQUE file content →
//! UNIQUE block digests, so no two tests share a `block_ref` PK or a `lineage_head` row. The GC tests
//! sweep the GLOBAL `block_ref`, but only ASSERT on their own store's blocks — a marked (live-HEAD)
//! block is never claimed, and `materialize` never consults `block_ref` — so a foreign row swept by
//! another concurrent test is harmless.

#![cfg(feature = "pg")]

use capsule_workspace_core::cas::*;
use capsule_workspace_core::daemon::materialize;
use capsule_workspace_core::daemon_loop::{self, materialize_on_start, publish_cycle, CycleOutcome};
use capsule_workspace_core::gc_pg;
use capsule_workspace_core::ifaces::{Fence, LineageId};
use capsule_workspace_core::manifest::Manifest;
use capsule_workspace_core::pg::PgLineageStore;
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Once};
use std::time::Duration;

/// Per-process-unique prefix so re-runs against the SAME DB (rows persist) never collide with a
/// previous run, and so parallel tests within this run stay disjoint (each test adds its own label).
fn nonce() -> String {
    format!("t{:x}", std::process::id())
}

fn db_url() -> Option<String> {
    match std::env::var("DATABASE_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            eprintln!(
                "SKIPPED daemon (DB test): set DATABASE_URL (e.g. \
                 postgres://postgres:pg@localhost:5434/capsule?sslmode=disable) to run against a \
                 real Postgres. Degraded coverage."
            );
            None
        }
    }
}

/// Apply the idempotent bootstrap DDL exactly ONCE per test process (serialized via `Once` to avoid
/// the `CREATE TABLE IF NOT EXISTS` race when parallel tests each bootstrap).
static SCHEMA: Once = Once::new();
fn ensure_schema(url: &str) {
    SCHEMA.call_once(|| {
        let s = PgLineageStore::connect(url).expect("connect for schema init");
        s.init_schema().expect("apply bootstrap schema");
    });
}

/// Connect a lineage store against the test DB (or `None` + printed skip when `DATABASE_URL` unset).
fn connect() -> Option<(PgLineageStore, String)> {
    let url = db_url()?;
    ensure_schema(&url);
    Some((
        PgLineageStore::connect(&url).expect("connect PgLineageStore"),
        url,
    ))
}

/// Deterministic pseudo-random CHUNK-sized block (mirrors `tests/gc_pg.rs`). High entropy so zstd
/// can't collapse distinct content into identical compressed bytes.
fn prng(seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut out = Vec::with_capacity(CHUNK);
    while out.len() < CHUNK {
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        out.extend_from_slice(&z.to_le_bytes());
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    }
    out.truncate(CHUNK);
    out
}

/// `n` concatenated CHUNK-sized pseudo-random chunks starting at `seed` (a multi-chunk file body).
fn body(seed: u64, n: u64) -> Vec<u8> {
    let mut v = Vec::new();
    for i in 0..n {
        v.extend(prng(seed + i));
    }
    v
}

fn write(p: &Path, b: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b).unwrap();
}

/// The set of blocks a manifest references (via its chunk index).
fn block_set(store: &dyn BlobStore, digest: &str) -> HashSet<BlockId> {
    let m = Manifest::from_bytes(&store.get_manifest(digest).unwrap()).unwrap();
    m.chunks.values().map(|loc| loc.block.clone()).collect()
}

// ---------- (1) lifecycle: publish×2 (fence 1→2), then a NEW daemon materializes-on-start ----------
#[test]
fn lifecycle_publish_then_materialize_on_start() {
    let Some((ls, url)) = connect() else {
        return;
    };
    let clock = ls.ref_clock();
    let d = tempfile::tempdir().unwrap();
    let store_dir = d.path().join("store"); // the shared DURABLE store (S3 stand-in)
    let tree_a = d.path().join("wa"); // first daemon's workspace
    let s = LocalBlobStore::new(&store_dir).unwrap();
    let lin = LineageId(format!("{}-life", nonce()));

    let base = 1_000_000u64;
    let a1 = body(base, 4);
    let big = body(base + 100, 6);
    write(&tree_a.join("a.txt"), &a1);
    write(&tree_a.join("dir/big.bin"), &big);

    // materialize-on-start with no HEAD is a clean no-op (fresh lineage).
    assert!(
        !materialize_on_start(&s, &ls, &lin, &tree_a, None).unwrap(),
        "no HEAD yet → nothing materialized"
    );
    assert!(ls.head(&lin).unwrap().is_none());

    // cycle 1 → fence 1
    match publish_cycle(
        &tree_a,
        &s,
        &ls,
        &clock,
        &lin,
        0,
        None,
        &mut Default::default(),
    )
    .unwrap()
    {
        CycleOutcome::Advanced(h) => assert_eq!(h.fence, Fence(1)),
        o => panic!("cycle 1 must Advance, got {o:?}"),
    }
    assert_eq!(ls.head(&lin).unwrap().unwrap().fence, Fence(1));

    // mutate the tree; cycle 2 → fence 2
    let a2 = body(base + 500, 4);
    write(&tree_a.join("a.txt"), &a2);
    let head2 = match publish_cycle(
        &tree_a,
        &s,
        &ls,
        &clock,
        &lin,
        0,
        None,
        &mut Default::default(),
    )
    .unwrap()
    {
        CycleOutcome::Advanced(h) => {
            assert_eq!(h.fence, Fence(2));
            h
        }
        o => panic!("cycle 2 must Advance, got {o:?}"),
    };
    assert_eq!(
        ls.head(&lin).unwrap().unwrap().manifest_digest,
        head2.manifest_digest
    );

    // NEW daemon: same DURABLE store + same DB (new handles), a FRESH empty workspace tree.
    let s2 = LocalBlobStore::new(&store_dir).unwrap();
    let ls2 = PgLineageStore::connect(&url).unwrap();
    let tree_b = d.path().join("wb"); // does not exist → materialize creates it
    assert!(
        materialize_on_start(&s2, &ls2, &lin, &tree_b, None).unwrap(),
        "HEAD present → materialized on start"
    );

    // byte-for-byte match with the LAST published content of tree_a.
    assert_eq!(
        fs::read(tree_b.join("a.txt")).unwrap(),
        a2,
        "a.txt restored byte-for-byte"
    );
    assert_eq!(
        fs::read(tree_b.join("dir/big.bin")).unwrap(),
        big,
        "big.bin restored byte-for-byte"
    );
    println!(
        "[daemon-1] lifecycle ok: fence 1→2; a NEW daemon materialized HEAD {} into a fresh tree \
         byte-for-byte",
        head2.manifest_digest
    );
}

// ---------- (2) MF3: a dedup-reused block (seeded from the parent HEAD) survives a GC sweep ----------
#[test]
fn mf3_dedup_reuse_survives_gc() {
    let Some((ls, _url)) = connect() else {
        return;
    };
    let clock = ls.ref_clock();
    let d = tempfile::tempdir().unwrap();
    let store_dir = d.path().join("store");
    let tree = d.path().join("w");
    let s = LocalBlobStore::new(&store_dir).unwrap();
    let lin = LineageId(format!("{}-mf3", nonce()));

    let base = 2_000_000u64;
    // three multi-chunk files with disjoint, unique content.
    write(&tree.join("keep1.bin"), &body(base, 3));
    write(&tree.join("keep2.bin"), &body(base + 50, 3));
    write(&tree.join("change.bin"), &body(base + 100, 3));

    // cycle 1
    let h1 = match publish_cycle(
        &tree,
        &s,
        &ls,
        &clock,
        &lin,
        0,
        None,
        &mut Default::default(),
    )
    .unwrap()
    {
        CycleOutcome::Advanced(h) => h,
        o => panic!("cycle 1 must Advance, got {o:?}"),
    };
    let b1 = block_set(&s, &h1.manifest_digest);

    // change ONLY change.bin; keep1/keep2 stay → their chunks dedup-reuse via `known` seeded from the
    // parent live-HEAD chunk index (MF3, done inside `publish_cycle`).
    write(&tree.join("change.bin"), &body(base + 900, 3));
    let h2 = match publish_cycle(
        &tree,
        &s,
        &ls,
        &clock,
        &lin,
        0,
        None,
        &mut Default::default(),
    )
    .unwrap()
    {
        CycleOutcome::Advanced(h) => {
            assert_eq!(h.fence, Fence(2));
            h
        }
        o => panic!("cycle 2 must Advance, got {o:?}"),
    };
    let b2 = block_set(&s, &h2.manifest_digest);

    // MF3 proof: cycle 2's manifest reuses a PARENT-HEAD block. If the seeding were broken (known
    // empty), cycle 2 would re-pack the unchanged chunks into a fresh block and this intersection
    // would be empty.
    let reused: Vec<BlockId> = b1.intersection(&b2).cloned().collect();
    assert!(
        !reused.is_empty(),
        "MF3: cycle 2 must dedup-reuse a parent-HEAD block (known seeded from parent chunk index); \
         b1={} b2={} reused=0 — seeding broken?",
        b1.len(),
        b2.len()
    );

    // GC at grace=0: every block_ref row is 'old', so ONLY the MARK (live HEAD = h2) protects blocks.
    let st = gc_pg::collect(
        &s,
        &clock,
        std::slice::from_ref(&h2.manifest_digest),
        Duration::ZERO,
    )
    .unwrap();
    println!(
        "[daemon-2] reused={} (b1={} b2={}); gc grace=0: scanned={} deleted={} kept_marked={} \
         raced={}",
        reused.len(),
        b1.len(),
        b2.len(),
        st.scanned,
        st.deleted,
        st.kept_marked,
        st.raced_young
    );

    // the dedup-reused blocks (referenced by the live HEAD → MARKED) survived the sweep.
    for b in &reused {
        assert!(
            s.has_block(b),
            "MF3: dedup-reused block {b} (marked by live HEAD) must survive GC"
        );
    }
    // and the current HEAD still materializes byte-for-byte.
    let out = d.path().join("out");
    materialize(&s, &h2.manifest_digest, &out).unwrap();
    assert_eq!(fs::read(out.join("keep1.bin")).unwrap(), body(base, 3));
    assert_eq!(fs::read(out.join("keep2.bin")).unwrap(), body(base + 50, 3));
    assert_eq!(
        fs::read(out.join("change.bin")).unwrap(),
        body(base + 900, 3),
        "changed file materializes to its new content"
    );
}

// ---------- (3) stale-fence: two racing publish cycles → one Advanced, one non-fatal Fenced ----------
// `publish_cycle` reads-then-advances, so two racers that both read the same fence collide in the DB
// CAS: exactly one wins (Advanced), the other is a genuine StaleFence surfaced as the non-fatal,
// matchable `CycleOutcome::Fenced` (NOT an Err — the daemon must defer, not crash). Different content
// per racer → different manifests, so the loser is a real conflict (not an idempotent lost-ack). A
// rare serialized attempt (both read different fences → both Advanced) is retried with a fresh lineage.
#[test]
fn stale_fence_surfaces_as_non_fatal_fenced() {
    let Some((_probe, url)) = connect() else {
        return;
    };
    let d = tempfile::tempdir().unwrap();
    let s = Arc::new(LocalBlobStore::new(d.path().join("store")).unwrap());
    let ls = Arc::new(PgLineageStore::connect(&url).unwrap());
    let clock = Arc::new(ls.ref_clock());

    let base = 3_000_000u64;
    // distinct racer content → distinct manifests (a genuine StaleFence, not a same-digest lost-ack).
    let tree_a = d.path().join("ta");
    write(&tree_a.join("f.bin"), &body(base, 2));
    let tree_b = d.path().join("tb");
    write(&tree_b.join("f.bin"), &body(base + 500, 2));

    let mut collided = false;
    for attempt in 0..16u32 {
        let lin = LineageId(format!("{}-fence{attempt}", nonce()));

        // seed HEAD to fence 1 with its OWN manifest (must be a real published manifest — the racers'
        // publish_cycle reads it back to seed `known`, so a fake HEAD digest would NotFound).
        let seed_tree = d.path().join(format!("seed{attempt}"));
        write(
            &seed_tree.join("s.bin"),
            &body(base + 900 + attempt as u64, 1),
        );
        match publish_cycle(
            &seed_tree,
            &*s,
            &ls,
            &*clock,
            &lin,
            0,
            None,
            &mut Default::default(),
        )
        .unwrap()
        {
            CycleOutcome::Advanced(h) => assert_eq!(h.fence, Fence(1)),
            o => panic!("seed must Advance, got {o:?}"),
        }

        // two racers, released together, publishing DIFFERENT trees onto the SAME lineage.
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for t in [tree_a.clone(), tree_b.clone()] {
            let (s, ls, clock, lin, barrier) = (
                s.clone(),
                ls.clone(),
                clock.clone(),
                lin.clone(),
                barrier.clone(),
            );
            handles.push(std::thread::spawn(move || {
                barrier.wait(); // maximize overlap: both read fence 1 before either advances
                publish_cycle(
                    &t,
                    &*s,
                    &ls,
                    &*clock,
                    &lin,
                    0,
                    None,
                    &mut Default::default(),
                )
            }));
        }
        let outcomes: Vec<CycleOutcome> = handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap()
                    .expect("cycle must not error (Ok Advanced or Ok Fenced)")
            })
            .collect();

        let advanced = outcomes
            .iter()
            .filter(|o| matches!(o, CycleOutcome::Advanced(_)))
            .count();
        let fenced: Vec<(Fence, Fence)> = outcomes
            .iter()
            .filter_map(|o| match o {
                CycleOutcome::Fenced { expected, current } => Some((*expected, *current)),
                _ => None,
            })
            .collect();

        if advanced == 1 && fenced.len() == 1 {
            let (expected, current) = fenced[0];
            assert_eq!(expected, Fence(1), "loser's expected fence was 1");
            assert_eq!(current, Fence(2), "winner moved HEAD to fence 2");
            assert_eq!(
                ls.head(&lin).unwrap().unwrap().fence,
                Fence(2),
                "HEAD ends at fence 2 (one winner)"
            );
            println!(
                "[daemon-3] attempt {attempt}: 1 Advanced + 1 Fenced (expected={expected:?} \
                 current={current:?}) — StaleFence surfaced as a non-fatal outcome"
            );
            collided = true;
            break;
        }
        println!("[daemon-3] attempt {attempt}: no collision (advanced={advanced}), retrying");
    }
    assert!(
        collided,
        "expected at least one read-then-advance fence collision in 16 attempts"
    );
}

// ---------- (4) health: the readiness latch flips 503 → 200 (DB-free; always runs) ----------
#[test]
fn health_server_readiness_transition() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let ready = Arc::new(AtomicBool::new(false));
    daemon_loop::spawn_health_server(listener, ready.clone());

    // before readiness → 503
    let r1 = http_probe(addr);
    assert!(
        r1.starts_with("HTTP/1.1 503"),
        "before ready must be 503, got: {r1:?}"
    );

    // arm readiness → 200 ok
    ready.store(true, Ordering::SeqCst);
    let r2 = http_probe(addr);
    assert!(
        r2.starts_with("HTTP/1.1 200"),
        "after ready must be 200, got: {r2:?}"
    );
    assert!(r2.ends_with("ok"), "200 body is 'ok', got: {r2:?}");
    println!("[daemon-4] health readiness latch flipped 503 → 200");
}

// pure response formatting (the fallback the task allows; DB-free; always runs).
#[test]
fn health_response_formatting() {
    assert_eq!(
        daemon_loop::health_response(true),
        "HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok"
    );
    assert!(daemon_loop::health_response(false).starts_with("HTTP/1.1 503 "));
}

fn http_probe(addr: SocketAddr) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.write_all(b"GET /health HTTP/1.0\r\n\r\n").unwrap();
    let mut buf = String::new();
    stream.read_to_string(&mut buf).unwrap();
    buf
}

// ---------- F1: an UNCHANGED tree is a NoChange cycle (no fence bump, no manifest churn) ----------
#[test]
fn nochange_cycle_when_tree_unchanged() {
    let Some((ls, _url)) = connect() else {
        return;
    };
    let clock = ls.ref_clock();
    let lin = LineageId(format!("nochg-{:x}", std::process::id()));
    let d = tempfile::tempdir().unwrap();
    let s = LocalBlobStore::new(d.path().join("store")).unwrap();
    let tree = d.path().join("w");
    write(&tree.join("f.bin"), &body(9100, 3));

    // cycle 1 → fence 1
    match publish_cycle(
        &tree,
        &s,
        &ls,
        &clock,
        &lin,
        0,
        None,
        &mut Default::default(),
    )
    .unwrap()
    {
        CycleOutcome::Advanced(h) => assert_eq!(h.fence, Fence(1)),
        o => panic!("cycle 1 must Advance, got {o:?}"),
    }
    let head1 = ls.head(&lin).unwrap().unwrap();

    // cycle 2 with NO tree change → NoChange, HEAD untouched (content-only digest == HEAD).
    match publish_cycle(
        &tree,
        &s,
        &ls,
        &clock,
        &lin,
        0,
        None,
        &mut Default::default(),
    )
    .unwrap()
    {
        CycleOutcome::NoChange => {}
        o => panic!("unchanged tree must be NoChange, got {o:?}"),
    }
    let head2 = ls.head(&lin).unwrap().unwrap();
    assert_eq!(head2.fence, Fence(1), "no fence bump on an unchanged tree");
    assert_eq!(
        head2.manifest_digest, head1.manifest_digest,
        "HEAD digest unchanged"
    );

    // a real change still Advances (sanity: NoChange isn't swallowing real commits).
    write(&tree.join("f.bin"), &body(9200, 3));
    match publish_cycle(
        &tree,
        &s,
        &ls,
        &clock,
        &lin,
        0,
        None,
        &mut Default::default(),
    )
    .unwrap()
    {
        CycleOutcome::Advanced(h) => assert_eq!(h.fence, Fence(2)),
        o => panic!("changed tree must Advance, got {o:?}"),
    }
}
