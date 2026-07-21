-- Bootstrap schema for the standalone capsule workspace store (plan 0001 §4). Applied via
-- sqlx::raw_sql(include_str!(...)) — the same mechanism zeroshot-cloud uses (no sqlx::migrate!).
-- Idempotent (IF NOT EXISTS) so re-applying on daemon start / test setup is a no-op.
--
-- lineage_head: the single mutable HEAD pointer per lineage, guarded by the monotonic `fence`
-- (single-writer CAS, mirrors capsule_attempts.fencing_token). block_ref: per-block liveness clock
-- (`last_referenced_at`, refreshed via clock_timestamp() on every reference) = the F1 reuse-clock,
-- the cross-host grace clock that replaces file mtime.

CREATE TABLE IF NOT EXISTS lineage_head (
    lineage_id      TEXT PRIMARY KEY,
    manifest_digest TEXT NOT NULL,
    fence           BIGINT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    CONSTRAINT lineage_head_fence_positive  CHECK (fence > 0),
    CONSTRAINT lineage_head_digest_nonempty CHECK (length(manifest_digest) > 0)
);

CREATE TABLE IF NOT EXISTS block_ref (
    block_digest        TEXT PRIMARY KEY,
    byte_length         BIGINT NOT NULL,
    first_referenced_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    last_referenced_at  TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    CONSTRAINT block_ref_len_nonneg CHECK (byte_length >= 0)
    -- NOTE: intentionally NO `last_referenced_at >= first_referenced_at` CHECK. `touch` sets
    -- last = clock_timestamp(); on an RDS failover to a standby whose clock is a few ms behind (or
    -- an NTP step-back), clock_timestamp() can dip below the row's first_referenced_at and the CHECK
    -- would fail the touch → fail the whole publish. first_referenced_at is never read by any query,
    -- so the CHECK guarded nothing load-bearing while coupling publish liveness to wall-clock
    -- monotonicity (reviewer S1).
);

CREATE INDEX IF NOT EXISTS block_ref_gc_idx ON block_ref (last_referenced_at);
