//! `RefClock` — the block liveness clock behind grace-period GC, abstracted so the production
//! Postgres clock (`PgRefClock`, feature `pg`) and an in-memory unit-test fake (Phase 3) share one
//! contract. Feature-independent on purpose: the fake must exist without pulling in `pg`/sqlx.
//!
//! The clock is the F1 reuse-invariant translated to prod: every block a publish references is
//! `touch`ed young BEFORE upload, so a block an in-flight (or crash-retry idempotent) publish needs
//! stays protected by grace. The authority is a single server-side clock (`clock_timestamp()` in
//! the PG impl), never the GC host's `SystemTime::now()` — that removes cross-host skew (plan MF1).

use crate::cas::BlockId;
use std::time::Duration;

pub trait RefClock {
    /// Refresh `last_referenced_at` to "now" for each `(block, byte_length)` (batched upsert). Called
    /// for EVERY block a publish references, BEFORE upload — the F1 protection is the touch, not the
    /// object write.
    fn touch(&self, blocks: &[(BlockId, u64)]) -> anyhow::Result<()>;
    /// Blocks whose `last_referenced_at` is older than `grace` — the GC sweep pre-filter (a superset
    /// of the truly-collectable; each survivor is re-checked by `claim_collectable`).
    fn candidates_older_than(&self, grace: Duration) -> anyhow::Result<Vec<BlockId>>;
    /// Atomic claim: delete the row IFF still older than grace (re-checked server-side). Returns
    /// true iff THIS caller won the claim (MF1). Caller deletes the S3 object only on true.
    fn claim_collectable(&self, block: &BlockId, grace: Duration) -> anyhow::Result<bool>;
}
