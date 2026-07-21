//! `PgLineageStore` (fence CAS) + `PgRefClock` (block liveness clock) against a REAL Postgres
//! (feature `pg`). Mirrors the S3 conformance gating:
//! - `DATABASE_URL` set  → run against that Postgres (local Docker or gated RDS).
//! - `DATABASE_URL` unset → each test PRINTS a skip notice and passes (degraded coverage is
//!   reported, not silent — repo rule).
//!
//! Run locally:
//!   docker run -d --name capsule-pg -p 5433:5432 -e POSTGRES_PASSWORD=pg -e POSTGRES_DB=capsule \
//!     postgres:17.10-alpine
//!   DATABASE_URL=postgres://postgres:pg@localhost:5433/capsule?sslmode=disable \
//!     cargo test --features pg --release --test pg_lineage -- --nocapture
//!
//! Every test uses a unique lineage_id / block_digest prefix so the parallel tests (one shared DB)
//! never collide.

#![cfg(feature = "pg")]

use capsule_workspace_core::cas::BlockId;
use capsule_workspace_core::ifaces::{Fence, LineageId};
use capsule_workspace_core::lineage::{LineageError, LineageStore};
use capsule_workspace_core::pg::{PgLineageStore, PgRefClock};
use capsule_workspace_core::refclock::RefClock;
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
                "SKIPPED pg_lineage: set DATABASE_URL (e.g. \
                 postgres://postgres:pg@localhost:5433/capsule?sslmode=disable) to run against a \
                 real Postgres. Degraded coverage."
            );
            None
        }
    }
}

/// Apply the idempotent bootstrap DDL exactly ONCE per test process. Serializing via `Once` avoids
/// the well-known `CREATE TABLE IF NOT EXISTS` race when parallel tests each try to bootstrap.
static SCHEMA: Once = Once::new();
fn ensure_schema(url: &str) {
    SCHEMA.call_once(|| {
        let s = PgLineageStore::connect(url).expect("connect for schema init");
        s.init_schema().expect("apply bootstrap schema");
    });
}

fn lineage() -> Option<PgLineageStore> {
    let url = db_url()?;
    ensure_schema(&url);
    Some(PgLineageStore::connect(&url).expect("connect PgLineageStore"))
}

fn refclock() -> Option<PgRefClock> {
    let url = db_url()?;
    ensure_schema(&url);
    Some(PgRefClock::connect(&url).expect("connect PgRefClock"))
}

// ---------- fence happy path: get→None; advance(0)→1; advance(1)→2; get→(digest2,2) ----------
#[test]
fn fence_happy_path() {
    let Some(ls) = lineage() else {
        return;
    };
    let id = LineageId(format!("{}-happy", nonce()));

    assert!(ls.get(&id).is_none(), "no HEAD before first advance");

    let h1 = ls
        .advance(&id, "digest1".into(), Fence(0))
        .expect("first advance (expected=0) inserts fence 1");
    assert_eq!(h1.fence, Fence(1));
    assert_eq!(h1.manifest_digest, "digest1");

    let h2 = ls
        .advance(&id, "digest2".into(), Fence(1))
        .expect("second advance (expected=1) updates to fence 2");
    assert_eq!(h2.fence, Fence(2));
    assert_eq!(h2.manifest_digest, "digest2");

    let cur = ls.get(&id).expect("HEAD present after advances");
    assert_eq!(cur.manifest_digest, "digest2");
    assert_eq!(cur.fence, Fence(2));
}

// ---------- concurrent advance: two threads race expected=1; exactly one wins, one StaleFence ----
#[test]
fn concurrent_advance_one_wins() {
    let Some(ls) = lineage() else {
        return;
    };
    let ls = Arc::new(ls);
    let id = LineageId(format!("{}-concurrent", nonce()));

    // seed HEAD to fence 1 so both racers share expected=1 (the UPDATE-path CAS).
    ls.advance(&id, "seed".into(), Fence(0))
        .expect("seed fence 1");

    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for i in 0..2u32 {
        let ls = Arc::clone(&ls);
        let id = id.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait(); // release both at once to maximize the race
            // DIFFERENT digests: so the loser's idempotent-retry can't masquerade as a lost-ack.
            ls.advance(&id, format!("racer{i}"), Fence(1))
        }));
    }
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    let oks = results.iter().filter(|r| r.is_ok()).count();
    let errs = results.iter().filter(|r| r.is_err()).count();
    assert_eq!(oks, 1, "exactly one advance from fence 1 wins");
    assert_eq!(errs, 1, "the other is rejected");

    let err = results
        .into_iter()
        .find_map(|r| r.err())
        .expect("one error");
    assert!(
        matches!(
            err.downcast_ref::<LineageError>(),
            Some(LineageError::StaleFence { .. })
        ),
        "loser must be a typed StaleFence, got: {err:?}"
    );

    // winner advanced HEAD to fence 2.
    assert_eq!(ls.get(&id).expect("HEAD present").fence, Fence(2));
}

// ---------- idempotent retry (MF4): re-advance to the SAME HEAD is a lost-ack = success ----------
#[test]
fn idempotent_retry_is_success() {
    let Some(ls) = lineage() else {
        return;
    };

    // UPDATE-path lost-ack: seed to fence 1, advance(exp=1)→2 with digestX, then replay exp=1 digestX.
    let id = LineageId(format!("{}-idem-update", nonce()));
    ls.advance(&id, "base".into(), Fence(0))
        .expect("seed fence 1");
    let first = ls
        .advance(&id, "digestX".into(), Fence(1))
        .expect("advance to fence 2");
    assert_eq!(first.fence, Fence(2));
    let replay = ls
        .advance(&id, "digestX".into(), Fence(1))
        .expect("re-advance exp=1 to the SAME digest is a lost-ack, must be Ok (not StaleFence)");
    assert_eq!(replay.fence, Fence(2));
    assert_eq!(replay.manifest_digest, "digestX");

    // INSERT-path lost-ack: first advance(exp=0) inserts fence 1; replaying exp=0 with the SAME
    // digest is likewise a lost-ack of our own INSERT → Ok.
    let id0 = LineageId(format!("{}-idem-insert", nonce()));
    let a = ls
        .advance(&id0, "d0".into(), Fence(0))
        .expect("first insert fence 1");
    assert_eq!(a.fence, Fence(1));
    let a_replay = ls
        .advance(&id0, "d0".into(), Fence(0))
        .expect("re-advance exp=0 to the SAME digest is a lost-ack, must be Ok");
    assert_eq!(a_replay.fence, Fence(1));
    assert_eq!(a_replay.manifest_digest, "d0");

    // A DIFFERENT digest at a stale fence is a genuine StaleFence (sanity: retry rule isn't too lax).
    let stale = ls
        .advance(&id, "someone-else".into(), Fence(1))
        .expect_err("different digest at a superseded fence must be rejected");
    assert!(
        matches!(
            stale.downcast_ref::<LineageError>(),
            Some(LineageError::StaleFence { .. })
        ),
        "expected typed StaleFence, got: {stale:?}"
    );
}

// ---------- M1 regression: touch with DUPLICATE block_digests must succeed (not error) ----------
// A real manifest references each ~64 MiB block hundreds of times; the touch list is heavily
// duplicated. Postgres rejects an ON CONFLICT DO UPDATE fed the same key twice, so touch MUST dedup.
#[test]
fn touch_tolerates_duplicate_digests() {
    let Some(rc) = refclock() else {
        return;
    };
    let b: BlockId = format!("{}-dupBlk", nonce());
    // the same block 200 times in one batch (as a chunk-iterated publisher would produce)
    let batch: Vec<(BlockId, u64)> = std::iter::repeat_with(|| (b.clone(), 4096))
        .take(200)
        .collect();
    rc.touch(&batch)
        .expect("touch with 200 duplicate digests must succeed (dedup in the primitive)");
    // and it registered exactly one row
    let cands = rc
        .candidates_older_than(Duration::from_secs(0))
        .expect("candidates");
    assert_eq!(
        cands.iter().filter(|d| **d == b).count(),
        1,
        "duplicates collapse to a single block_ref row"
    );
}

// ---------- S2: concurrent FIRST-writers (INSERT path, expected=0) — one wins, one StaleFence ----
#[test]
fn concurrent_first_writers_insert_path() {
    let Some(ls) = lineage() else {
        return;
    };
    let ls = Arc::new(ls);

    // (a) DIFFERENT digests racing expected=0 → exactly one inserts fence 1, the other StaleFence.
    let id = LineageId(format!("{}-firstrace", nonce()));
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for i in 0..2u32 {
        let ls = Arc::clone(&ls);
        let id = id.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            ls.advance(&id, format!("first{i}"), Fence(0))
        }));
    }
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        1,
        "one first-writer wins"
    );
    let err = results
        .into_iter()
        .find_map(|r| r.err())
        .expect("one error");
    assert!(
        matches!(
            err.downcast_ref::<LineageError>(),
            Some(LineageError::StaleFence { .. })
        ),
        "the losing first-writer (different digest) is a StaleFence, got: {err:?}"
    );
    assert_eq!(ls.get(&id).expect("HEAD").fence, Fence(1));

    // (b) SAME digest racing expected=0 → BOTH succeed (idempotent lost-ack), HEAD at fence 1.
    let id2 = LineageId(format!("{}-firstsame", nonce()));
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for _ in 0..2u32 {
        let ls = Arc::clone(&ls);
        let id2 = id2.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            ls.advance(&id2, "sameD".into(), Fence(0))
        }));
    }
    let oks = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .filter(|r| r.is_ok())
        .count();
    assert_eq!(
        oks, 2,
        "same-digest first-writers both succeed (content-addressed idempotence)"
    );
    assert_eq!(ls.get(&id2).expect("HEAD").fence, Fence(1));
}

// ---------- reuse-clock: touch bumps last_referenced_at (F1 invariant #3) ----------
#[test]
fn reuse_clock_touch_bumps() {
    let Some(rc) = refclock() else {
        return;
    };
    let b: BlockId = format!("{}-reuseB", nonce());

    rc.touch(&[(b.clone(), 123)]).expect("first touch");
    // long grace: a freshly-touched block is NOT older than 1h → not a candidate.
    let long = rc
        .candidates_older_than(Duration::from_secs(3600))
        .expect("candidates (1h grace)");
    assert!(!long.contains(&b), "fresh block not older than 1h grace");
    // grace 0: strictly older than clock_timestamp() → IS a candidate.
    let zero = rc
        .candidates_older_than(Duration::from_secs(0))
        .expect("candidates (0 grace)");
    assert!(zero.contains(&b), "block is older than 0 grace");

    // touch again after a beat: last_referenced_at MOVES FORWARD, so it's young again under 1h grace
    // (if touch did NOT bump the clock, the block would still be old — this is the F1 reuse-clock).
    std::thread::sleep(Duration::from_millis(50));
    rc.touch(&[(b.clone(), 123)]).expect("second touch");
    let long2 = rc
        .candidates_older_than(Duration::from_secs(3600))
        .expect("candidates (1h grace, after re-touch)");
    assert!(
        !long2.contains(&b),
        "re-touched block is young again under 1h grace (touch bumped last_referenced_at)"
    );
}

// ---------- atomic claim (MF1): age re-checked SERVER-SIDE; a touch-between defeats the claim -----
#[test]
fn atomic_claim_server_side_recheck() {
    let Some(rc) = refclock() else {
        return;
    };
    let n = nonce();

    // (a) young block: claim at a 1h grace must NOT win — the server-side re-check protects it.
    let b1: BlockId = format!("{n}-claimYoung");
    rc.touch(&[(b1.clone(), 10)]).expect("touch b1");
    assert!(
        !rc.claim_collectable(&b1, Duration::from_secs(3600))
            .expect("claim young"),
        "young block is not claimable at 1h grace"
    );

    // (b) grace 0: claim wins ONCE (deletes the row); a second claim is a no-op (false).
    let b2: BlockId = format!("{n}-claimAge");
    rc.touch(&[(b2.clone(), 10)]).expect("touch b2");
    assert!(
        rc.claim_collectable(&b2, Duration::from_secs(0))
            .expect("first claim (grace 0)"),
        "grace-0 claim wins for an aged block"
    );
    assert!(
        !rc.claim_collectable(&b2, Duration::from_secs(0))
            .expect("second claim"),
        "row already claimed → second claim is false"
    );
    let after = rc
        .candidates_older_than(Duration::from_secs(0))
        .expect("candidates after claim");
    assert!(
        !after.contains(&b2),
        "claimed block is gone from candidates"
    );

    // (c) SERVER-SIDE re-check (the MF1 point): a block is a candidate at grace 0, but a racing
    // publish touches it young BEFORE we claim → the claim at a LARGE grace re-checks age against
    // clock_timestamp() and REFUSES (it would have wrongly deleted a live block if it used a host
    // cutoff computed at select time).
    let b3: BlockId = format!("{n}-claimRecheck");
    rc.touch(&[(b3.clone(), 10)]).expect("touch b3");
    let cands = rc
        .candidates_older_than(Duration::from_secs(0))
        .expect("candidates (grace 0)");
    assert!(cands.contains(&b3), "b3 is a candidate at grace 0");
    // racing publish re-touches b3 young...
    rc.touch(&[(b3.clone(), 10)]).expect("re-touch b3");
    // ...so the claim at 1h grace re-checks server-side and does NOT win.
    assert!(
        !rc.claim_collectable(&b3, Duration::from_secs(3600))
            .expect("claim after re-touch"),
        "server-side re-check refuses a re-touched (young) block at 1h grace"
    );
    // b3's row still exists (not deleted).
    let still = rc
        .candidates_older_than(Duration::from_secs(0))
        .expect("candidates (grace 0, final)");
    assert!(
        still.contains(&b3),
        "protected block's row survived the claim"
    );
}
