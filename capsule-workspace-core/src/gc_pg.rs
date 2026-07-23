//! `gc_pg` — the production, clock-driven garbage collector (the drop-in replacement for the
//! file-backed `gc::collect`). Generic over the `BlobStore` (block bytes) and `RefClock` (liveness
//! clock) traits, so it runs against real S3 + Postgres OR `LocalBlobStore` + `MemRefClock` in a fast
//! unit test.
//!
//! Correctness = the three-invariant GC model + the MF1 concurrency fix:
//! 1. **MARK (invariant #2):** a block referenced by ANY live-HEAD manifest is NEVER collected,
//!    regardless of age — `mark_live_blocks`. This protects blocks referenced by live-but-old HEADs
//!    (which the clock alone would not, since they aren't `touch`ed until re-referenced).
//! 2. **Grace clock (invariants #1/#3):** only blocks whose `last_referenced_at` is older than `grace`
//!    are even candidates; a publish `touch`es every block it references young, so in-flight blocks are
//!    protected. The clock is a single server-side clock, never the GC host's.
//! 3. **Atomic claim → then delete (MF1):** for each unmarked candidate, `claim_collectable` deletes
//!    the `block_ref` row IFF it is STILL older than grace at the instant of the delete (re-checked
//!    server-side). Only on a WON claim do we `delete_block` the object. A racing publish's `touch`
//!    (which moved the row young) makes the claim return 0 rows → the object is never deleted. This
//!    closes the select-vs-delete TOCTOU that an S3-first ordering left open.
//!
//! Ordering is row-claim-FIRST, object-delete-second. Two residuals (documented, accepted for this
//! measurement build; both need a per-publisher lease to close fully, which the plan defers):
//!   1. **Orphan object (cost leak, NOT loss):** a crash between the won claim and `delete_block`
//!      leaves the bytes with no `block_ref` row → a periodic reconciler (list the store, drop objects
//!      with no row) reclaims it.
//!   2. **Claim→delete straddle (a NARROW live-byte loss):** if the GC thread STALLS between the won
//!      claim and `delete_block`, and in that window a publisher re-references the *same, previously-
//!      orphan* block — `touch` (row re-inserted young) then `put_block` (object re-uploaded) — then
//!      GC's `delete_block` deletes the just-re-uploaded object, and the publisher's subsequent
//!      manifest references a now-missing block. This is real work loss, closed fully only by a
//!      per-publisher lease (out of scope here). It does NOT apply to dedup-REUSE (a reused block is
//!      in a live HEAD → MARKED → never a candidate), only to a block that was a genuine orphan at
//!      claim time and is concurrently resurrected. Keep the GC-host stall window small (per-block
//!      claim→delete, no batching) to shrink it.
//!
//! Operational invariant the deployment must hold: **grace > max(publish duration, GC sweep
//! duration + max clock skew)** — so a block touched at the start of a publish stays young through
//! commit, at which point the MARK takes over. Real deployments also single-flight the sweep (a PG
//! advisory lock) to avoid two sweepers wasting work; correctness does not depend on it (each
//! per-block claim is individually atomic — two sweepers just means one wins each block).

use crate::cas::BlobStore;
use crate::gc::mark_live_blocks;
use crate::refclock::RefClock;
use anyhow::Result;
use std::time::Duration;

#[derive(Debug, Default, serde::Serialize)]
pub struct GcPgStats {
    /// candidate blocks examined (older than grace at pre-filter time)
    pub scanned: usize,
    /// blocks whose object was deleted (claim won + delete_block ok)
    pub deleted: usize,
    /// candidates kept because a live HEAD references them (invariant #2)
    pub kept_marked: usize,
    /// candidates that lost the atomic claim — a racing publish re-touched them young between the
    /// pre-filter and the claim (MF1 server-side re-check protected a live block)
    pub raced_young: usize,
    /// claims won but `delete_block` failed → orphan object (row gone, bytes remain) for the reconciler
    pub orphaned: usize,
    /// live HEADs whose manifest was ABSENT on the store — their blocks could not be marked this
    /// sweep. Non-fatal (skipped), but the caller MUST surface/alert: a live HEAD we can't read is an
    /// anomaly (replication lag, partial restore, or — the dangerous case — a lost committed manifest).
    pub missing_live_manifests: usize,
}

/// Sweep collectable blocks. Never collects a marked (live-referenced) block or one a concurrent
/// publish just touched; deletes only genuine, aged, unreferenced orphans.
///
/// ⚠️ **DATA-LOSS FOOTGUN — `live` MUST be STORE-WIDE, not per-lineage.** `block_ref` and the block
/// store are GLOBAL and content-addressed: the SAME block id can be referenced by the HEADs of
/// DIFFERENT lineages. So `live` must be **every currently-live HEAD of EVERY lineage sharing this
/// store** (within retention) — e.g. `SELECT manifest_digest FROM lineage_head` with NO `WHERE`. If a
/// caller passes only its own lineage's HEAD (the tempting per-daemon call), this will MARK one
/// lineage and then claim+delete every OTHER lineage's aged-but-live blocks → cross-lineage live-byte
/// loss. GC is therefore a **store-wide singleton actor**, NOT a per-lineage/per-daemon responsibility.
/// (This is why no per-lineage caller in this crate invokes `collect` — wiring it is a deliberate
/// integration step; see the build log's Phase-5 final-review follow-ups.)
pub fn collect(
    store: &dyn BlobStore,
    clock: &dyn RefClock,
    live: &[String],
    grace: Duration,
) -> Result<GcPgStats> {
    let (marked, missing) = mark_live_blocks(store, live)?; // invariant #2
    let candidates = clock.candidates_older_than(grace)?; // grace pre-filter (superset)
    let mut st = GcPgStats {
        missing_live_manifests: missing,
        ..Default::default()
    };
    for b in candidates {
        st.scanned += 1;
        if marked.contains(&b) {
            st.kept_marked += 1;
            continue; // referenced by a live HEAD → never collect, whatever its age
        }
        // Atomic claim: deletes the row IFF still older than grace at delete-time (server-side). A
        // publish that re-referenced this block since the pre-filter has touched it young → 0 rows.
        if clock.claim_collectable(&b, grace)? {
            // won the claim → the row is gone; now delete the object. A failure here orphans the
            // object (re-driveable only by a reconciler, since the row is already gone) — count it,
            // do NOT abort the whole sweep for one orphan.
            match store.delete_block(&b) {
                Ok(_) => st.deleted += 1,
                Err(e) => {
                    st.orphaned += 1;
                    eprintln!(
                        "gc_pg: claimed block_ref row for {b} but delete_block failed \
                         (orphan object, reconciler reclaims): {e}"
                    );
                }
            }
        } else {
            st.raced_young += 1; // a concurrent touch protected it (MF1)
        }
    }
    Ok(st)
}
