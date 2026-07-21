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
//! Ordering is row-claim-FIRST, object-delete-second. Residual (documented, acceptable for a
//! measurement build): a crash between the won claim and `delete_block` leaves an **orphan object**
//! (row gone, bytes remain) — a cost leak, never live-byte loss; a periodic orphan-object reconciler
//! (list the store, drop objects with no `block_ref` row) reclaims it. Operational invariant the
//! deployment must hold: **grace > max(publish duration, GC sweep duration)** — so a block touched at
//! the start of a publish stays young through commit, at which point the MARK takes over. Real
//! deployments also single-flight the sweep (a PG advisory lock) to avoid two sweepers wasting work;
//! correctness does not depend on it (each per-block claim is individually atomic — two sweepers just
//! means one wins each block).

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
}

/// Sweep collectable blocks. `live` = the manifest digests of every currently-live HEAD (within
/// retention). Never collects a marked (live-referenced) block or one a concurrent publish just
/// touched; deletes only genuine, aged, unreferenced orphans.
pub fn collect(
    store: &dyn BlobStore,
    clock: &dyn RefClock,
    live: &[String],
    grace: Duration,
) -> Result<GcPgStats> {
    let marked = mark_live_blocks(store, live)?; // invariant #2
    let candidates = clock.candidates_older_than(grace)?; // grace pre-filter (superset)
    let mut st = GcPgStats::default();
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
