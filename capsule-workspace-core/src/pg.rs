//! `PgLineageStore` (fence CAS) + `PgRefClock` (block liveness clock) — the production Postgres
//! backends behind the SYNC `LineageStore` / `RefClock` traits (feature `pg`).
//!
//! Design (see `planning/plans/0001-production-backends.md` §3/§4, research finding B):
//! - The traits are SYNC and the pipeline is CPU-bound rayon batch work, so async (sqlx) is
//!   CONTAINED here exactly like `S3BlobStore`: a shared [`PgRuntime`] holds ONE bounded
//!   **multi-thread** tokio runtime + the `PgPool` and `block_on`s the sqlx futures inside each sync
//!   method. Multi-thread (not current-thread) because `advance`/`touch` can be called concurrently
//!   from N rayon/std threads → concurrent `block_on` (MF2). A `debug_assert` tripwire catches an
//!   accidental call from inside an async context ("runtime within a runtime").
//! - `advance` is the fence CAS, **dispatched on `expected`** (mirrors zeroshot-cloud's
//!   `advance_run_binding`): `expected == 0` first-write `INSERT ... ON CONFLICT DO NOTHING`;
//!   `expected >= 1` guarded `UPDATE ... WHERE fence = expected`. A would-be StaleFence is absorbed
//!   as success iff HEAD already equals `(my digest, expected+1)` — a lost-ack on the final publish
//!   is durable, not a real conflict (MF4). `Fence(u64) ↔ BIGINT(i64)` via guarded conversion,
//!   never a raw `as`.
//! - `PgRefClock` is the F1 reuse-clock in prod: `touch` upserts `last_referenced_at =
//!   clock_timestamp()`; `claim_collectable` is the atomic guarded `DELETE ... RETURNING` that
//!   re-checks age SERVER-SIDE (MF1) — grace is NEVER computed from the GC host clock.

use crate::cas::BlockId;
use crate::ifaces::{Fence, LineageId};
use crate::lineage::{Head, LineageError, LineageStore};
use crate::refclock::RefClock;
use anyhow::{anyhow, Result};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::{Builder, Handle, Runtime};

/// Guarded `u64 -> i64` (Postgres BIGINT). Never a raw `as` cast (which silently wraps a huge fence
/// to a negative and violates the `CHECK (fence > 0)`).
fn signed(v: u64) -> Result<i64> {
    i64::try_from(v).map_err(|_| anyhow!("value {v} exceeds i64/BIGINT range"))
}
/// Guarded `i64 (BIGINT) -> u64`. Rejects a negative value (a corrupt/underflowed row) instead of
/// wrapping it to a colossal u64.
fn positive_u64(v: i64) -> Result<u64> {
    u64::try_from(v).map_err(|_| anyhow!("negative BIGINT {v} where a u64 was expected"))
}

/// Render a `Duration` grace as a Postgres interval literal bound to `$n::interval`. Whole seconds
/// is the GC granularity (grace is on the order of minutes); sub-second precision is irrelevant.
fn grace_secs(grace: Duration) -> String {
    format!("{} seconds", grace.as_secs())
}

/// Shared backend: the pool + the one owned runtime + the `block_on` tripwire. Both `PgLineageStore`
/// and `PgRefClock` hold an `Arc<PgRuntime>` so they share a single pool/runtime against the same DB.
struct PgRuntime {
    pool: PgPool,
    rt: Runtime,
}

impl PgRuntime {
    fn connect(database_url: &str) -> Result<Arc<Self>> {
        // bounded so a small (2–4 vCPU) pod doesn't spin excess tokio workers against rayon.
        let rt = Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?;
        // Build the pool ON this runtime; it is only ever used from `self.rt.block_on`, so its
        // background tasks live on the same runtime (a sqlx pool bound to a different runtime breaks).
        let pool = rt.block_on(async {
            PgPoolOptions::new()
                .max_connections(8)
                .connect(database_url)
                .await
        })?;
        Ok(Arc::new(Self { pool, rt }))
    }

    /// `block_on` with the nested-runtime tripwire (MF2). Called only from sync (rayon/std) threads.
    fn on<F: Future>(&self, fut: F) -> F::Output {
        debug_assert!(
            Handle::try_current().is_err(),
            "PgLineageStore/PgRefClock method called from inside an async context — would nest-panic block_on"
        );
        self.rt.block_on(fut)
    }

    fn init_schema(&self) -> Result<()> {
        self.on(async {
            sqlx::raw_sql(include_str!("migrations/0001_bootstrap.sql"))
                .execute(&self.pool)
                .await?;
            Ok::<(), anyhow::Error>(())
        })
    }

    /// Precise HEAD read (`Result`, unlike the infallible trait `get`) — used internally by
    /// `advance`'s idempotent-retry so a transient DB error there is a real error, not "no HEAD".
    fn get_head(&self, id: &LineageId) -> Result<Option<Head>> {
        self.on(async {
            let row = sqlx::query_as::<_, (String, i64)>(
                "SELECT manifest_digest, fence FROM lineage_head WHERE lineage_id = $1",
            )
            .bind(id.0.as_str())
            .fetch_optional(&self.pool)
            .await?;
            match row {
                Some((digest, fence)) => Ok(Some(Head {
                    manifest_digest: digest,
                    fence: Fence(positive_u64(fence)?),
                })),
                None => Ok(None),
            }
        })
    }

    fn advance(&self, id: &LineageId, digest: String, expected: Fence) -> Result<Head> {
        if expected.0 == 0 {
            // First write: INSERT the row at fence 1. ON CONFLICT DO NOTHING so a concurrent
            // first-writer can't corrupt — the loser gets rows_affected == 0 and falls to the
            // stale/idempotent path.
            let res = self.on(async {
                sqlx::query(
                    "INSERT INTO lineage_head(lineage_id, manifest_digest, fence) \
                     VALUES($1, $2, 1) ON CONFLICT (lineage_id) DO NOTHING",
                )
                .bind(id.0.as_str())
                .bind(digest.as_str())
                .execute(&self.pool)
                .await
            })?;
            if res.rows_affected() == 1 {
                return Ok(Head {
                    manifest_digest: digest,
                    fence: Fence(1),
                });
            }
            return self.stale_or_idempotent(id, &digest, expected);
        }

        // expected >= 1: guarded UPDATE, succeeds iff the stored fence is still exactly `expected`.
        let next = expected
            .0
            .checked_add(1)
            .ok_or_else(|| anyhow!("fence overflow at u64::MAX"))?;
        let next_i = signed(next)?;
        let exp_i = signed(expected.0)?;
        let res = self.on(async {
            sqlx::query(
                "UPDATE lineage_head SET manifest_digest = $2, fence = $3, \
                 updated_at = transaction_timestamp() WHERE lineage_id = $1 AND fence = $4",
            )
            .bind(id.0.as_str())
            .bind(digest.as_str())
            .bind(next_i)
            .bind(exp_i)
            .execute(&self.pool)
            .await
        })?;
        if res.rows_affected() == 1 {
            return Ok(Head {
                manifest_digest: digest,
                fence: Fence(next),
            });
        }
        self.stale_or_idempotent(id, &digest, expected)
    }

    /// A CAS that affected 0 rows is a StaleFence — EXCEPT the idempotent-retry case (MF4): if HEAD
    /// already equals `(my digest, expected+1)`, this is a lost-ack of our OWN successful write (a
    /// SIGTERM-final publish whose ack we never saw), which is durable → success, not an alarm.
    fn stale_or_idempotent(&self, id: &LineageId, digest: &str, expected: Fence) -> Result<Head> {
        let next = Fence(expected.0.saturating_add(1));
        match self.get_head(id)? {
            Some(h) if h.manifest_digest == digest && h.fence == next => Ok(h),
            Some(h) => Err(LineageError::StaleFence {
                expected,
                current: h.fence,
            }
            .into()),
            None => Err(LineageError::StaleFence {
                expected,
                current: Fence(0),
            }
            .into()),
        }
    }

    fn touch(&self, blocks: &[(BlockId, u64)]) -> Result<()> {
        if blocks.is_empty() {
            return Ok(());
        }
        // DEDUP by block_digest first: a manifest references each ~64 MiB block hundreds of times
        // (one per packed 256 KiB chunk), so the natural touch list has heavy duplicates. Postgres
        // rejects an `ON CONFLICT DO UPDATE` whose source feeds the same key twice ("cannot affect
        // row a second time") — so an un-deduped touch would fail EVERY real publish. Dedup here
        // (also shrinks the payload ~256×). Same content-addressed digest ⇒ same byte_length, so
        // collapsing duplicates is lossless. BTreeMap → deterministic bind order.
        let mut uniq: std::collections::BTreeMap<&str, i64> = std::collections::BTreeMap::new();
        for (b, len) in blocks {
            uniq.insert(b.as_str(), signed(*len)?);
        }
        let digests: Vec<String> = uniq.keys().map(|s| s.to_string()).collect();
        let lengths: Vec<i64> = uniq.values().copied().collect();
        self.on(async {
            sqlx::query(
                "INSERT INTO block_ref(block_digest, byte_length) \
                 SELECT * FROM unnest($1::text[], $2::bigint[]) \
                 ON CONFLICT (block_digest) DO UPDATE SET last_referenced_at = clock_timestamp()",
            )
            .bind(&digests)
            .bind(&lengths)
            .execute(&self.pool)
            .await?;
            Ok::<(), anyhow::Error>(())
        })
    }

    fn candidates_older_than(&self, grace: Duration) -> Result<Vec<BlockId>> {
        let grace = grace_secs(grace);
        self.on(async {
            let rows = sqlx::query_scalar::<_, String>(
                "SELECT block_digest FROM block_ref \
                 WHERE last_referenced_at < clock_timestamp() - $1::interval \
                 ORDER BY last_referenced_at",
            )
            .bind(grace)
            .fetch_all(&self.pool)
            .await?;
            Ok(rows)
        })
    }

    fn claim_collectable(&self, block: &BlockId, grace: Duration) -> Result<bool> {
        let grace = grace_secs(grace);
        // Atomic guarded delete: the age is re-checked SERVER-SIDE against clock_timestamp() at the
        // instant of the DELETE, so a racing publish's touch (which moved last_referenced_at young)
        // makes the row no longer match → 0 rows → we did NOT win the claim (MF1). Never compute a
        // cutoff from the GC host clock.
        let res = self.on(async {
            sqlx::query(
                "DELETE FROM block_ref \
                 WHERE block_digest = $1 AND last_referenced_at < clock_timestamp() - $2::interval",
            )
            .bind(block.as_str())
            .bind(grace)
            .execute(&self.pool)
            .await
        })?;
        // block_digest is the PK ⇒ this DELETE affects 0 or 1 rows; 1 = we won the claim.
        Ok(res.rows_affected() == 1)
    }
}

/// Postgres-backed `LineageStore` (fence CAS). Cheap to `clone`-share via `ref_clock`.
pub struct PgLineageStore {
    inner: Arc<PgRuntime>,
}

impl PgLineageStore {
    /// Connect a native `PgPool` from a `DATABASE_URL` and build the owned runtime. Establishes at
    /// least one connection eagerly so a bad URL fails fast here rather than on first `advance`.
    pub fn connect(database_url: &str) -> Result<Self> {
        Ok(Self {
            inner: PgRuntime::connect(database_url)?,
        })
    }

    /// Apply the bootstrap DDL (idempotent). The standalone build owns its schema — this is the
    /// `sqlx::raw_sql(include_str!(...))` mechanism zeroshot-cloud uses, not `sqlx::migrate!`.
    pub fn init_schema(&self) -> Result<()> {
        self.inner.init_schema()
    }

    /// A `PgRefClock` sharing THIS store's pool + runtime (same DB backend).
    pub fn ref_clock(&self) -> PgRefClock {
        PgRefClock {
            inner: Arc::clone(&self.inner),
        }
    }

    /// FALLIBLE HEAD read (unlike the infallible trait `get`, which degrades a transient DB error to
    /// `None`). The Phase-4 daemon's materialize-on-start MUST use this so it can distinguish "no HEAD
    /// yet" (`Ok(None)`) from "DB transiently unreachable" (`Err`) — otherwise it might skip restoring
    /// a workspace on a blip and start empty (reviewer N3).
    pub fn head(&self, id: &LineageId) -> Result<Option<Head>> {
        self.inner.get_head(id)
    }
}

impl LineageStore for PgLineageStore {
    fn get(&self, id: &LineageId) -> Option<Head> {
        // The trait `get` is infallible (`Option`), so a transient DB error can't propagate here.
        // Report it loudly and degrade to None (conceptual fence 0). A caller acting on this None
        // will attempt `advance(expected=0)`, whose CAS then surfaces the conflict as a StaleFence —
        // i.e. a transient error degrades to a retryable rejection, never a silent overwrite.
        match self.inner.get_head(id) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("PgLineageStore::get transient error (degrading to None): {e}");
                None
            }
        }
    }
    fn advance(&self, id: &LineageId, digest: String, expected: Fence) -> Result<Head> {
        self.inner.advance(id, digest, expected)
    }
}

/// Postgres-backed `RefClock` (the F1 block liveness clock) over the same pool.
pub struct PgRefClock {
    inner: Arc<PgRuntime>,
}

impl PgRefClock {
    /// Connect an independent `PgRefClock` (own pool + runtime). Prefer `PgLineageStore::ref_clock`
    /// when you already hold a lineage store against the same DB.
    pub fn connect(database_url: &str) -> Result<Self> {
        Ok(Self {
            inner: PgRuntime::connect(database_url)?,
        })
    }

    /// Apply the bootstrap DDL (idempotent) — so a ref-clock-only caller can bootstrap without a
    /// lineage store.
    pub fn init_schema(&self) -> Result<()> {
        self.inner.init_schema()
    }
}

impl RefClock for PgRefClock {
    fn touch(&self, blocks: &[(BlockId, u64)]) -> Result<()> {
        self.inner.touch(blocks)
    }
    fn candidates_older_than(&self, grace: Duration) -> Result<Vec<BlockId>> {
        self.inner.candidates_older_than(grace)
    }
    fn claim_collectable(&self, block: &BlockId, grace: Duration) -> Result<bool> {
        self.inner.claim_collectable(block, grace)
    }
}
