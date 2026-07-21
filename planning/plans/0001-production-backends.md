# Plan 0001 — Production backends for the capsule workspace store (standalone)

Status: DRAFT (awaiting senior-reviewer approval)
Owner: capsule-workspace-core
Branch: `claude/zeroshot-workspace-file-transfer-13f0be` (extend; not branching from main)
Companion notes: `docs/research/production-build-log.md` (research findings A/B/C + decisions)

## 1. Objective

Turn the validated prototype into production-shaped implementations of the two backend swap points
plus a daemon, **standalone** (no zeroshot-cloud service integration), tested on **real AWS** (S3 +
Postgres) in the **internal account 993939946442**:

1. `S3BlobStore` — real S3 behind the `BlobStore` trait.
2. `PgLineageStore` (fence CAS) + a `block_ref` liveness table = the F1 reuse-clock in prod.
3. Postgres-driven GC (grace clock from `block_ref`, reachability from live manifests).
4. `daemon` subcommand shaped to drop into a capsule pod later.

## 2. Non-goals (explicit, this phase)

- No wiring into zeroshot-cloud services / control-plane callbacks / k8s API / VolumeSnapshots.
- No agent-state (`/var/lib/...` SQLite) publish — needs a consistent DB checkpoint; separate problem.
- No client-side encryption (SSE-KMS on the bucket suffices given the trust boundary); note as future.
- No multipart upload v1 (64 MiB blocks fit a single PUT; multipart is a later retry-granularity opt).
- No `manifest_block` edge table v1 (GC marks by reading live manifests, as the prototype does).

## 3. Load-bearing architecture decisions (rationale in the build log)

- **Keep the `BlobStore`/`LineageStore` traits SYNC; contain async in the adapters.** The pipeline is a
  CPU-bound rayon batch job; async buys it nothing and would force a rewrite of 50 green tests. Each
  adapter (`S3BlobStore`, `PgLineageStore`) holds one `tokio::runtime::Runtime` and `block_on`s the
  aws-sdk / sqlx futures inside the sync methods. Concurrency we DO want (parallel block uploads) lives
  inside the adapter via `join_all`. Called only from sync (rayon) threads ⇒ no nested-runtime panic.
- **`LineageStore::advance` changes `&mut self` → `&self`** (a pooled store is shared/interior-concurrent);
  `FileLineageStore` gets a `Mutex`. This is the only trait-signature change.
- **Add `delete_block`/`delete_manifest` to `BlobStore`** (idempotent, returns `bool` "did-delete";
  NotFound ⇒ `false`). Needed by GC; `LocalBlobStore` = `fs::remove_file`, `S3BlobStore` = `DeleteObject`.
- **Extract the GC MARK step** `mark_live_blocks(store: &dyn BlobStore, live: &[String]) -> HashSet<BlockId>`
  (read each live manifest, union referenced blocks) into a shared helper. Existing filesystem
  `gc::collect` keeps its mtime clock + all 7 tests; a new `gc_pg` sweep reuses the same mark helper but
  takes candidates from `block_ref` and deletes S3-then-row.
- **GC three-invariant model carried to prod** (from the design doc):
  1. grace ≥ max_publish + max_clock_skew — now a single PG `clock_timestamp()` clock (cross-host safe),
     not file mtime.
  2. mark-set covers every live manifest — GC reads live HEADs (within retention) and marks their blocks.
  3. refresh-on-every-reference — `block_ref` upsert `ON CONFLICT DO UPDATE SET last_referenced_at =
     clock_timestamp()` on every publish that references a block (the F1 fix in prod).
- **GC delete ordering**: delete the S3 object FIRST, then the `block_ref` row (crash ⇒ re-driveable
  orphan row, never a leaked-forever object). Mirrors the prototype's NotFound-tolerant idempotent rm.
- **Env/config contract** mirrors zeroshot-cloud: S3 via `aws_config::load_defaults(BehaviorVersion::
  latest())` + default cred chain; honor `S3_ENDPOINT_URL` (present ⇒ `force_path_style(true)` for
  MinIO/localstack) + `S3_BUCKET` + `AWS_REGION`. PG via `sqlx =0.9` native `PgPool` from `DATABASE_URL`.
  Schema applied via `sqlx::raw_sql(include_str!("migrations/0001_bootstrap.sql"))` (their mechanism).

## 4. Schema (our own bootstrap; standalone, not extending a zeroshot-cloud service)

```sql
CREATE TABLE lineage_head (
    lineage_id      TEXT PRIMARY KEY,
    manifest_digest TEXT NOT NULL,
    fence           BIGINT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    CONSTRAINT lineage_head_fence_positive  CHECK (fence > 0),
    CONSTRAINT lineage_head_digest_nonempty CHECK (length(manifest_digest) > 0)
);
CREATE TABLE block_ref (
    block_digest        TEXT PRIMARY KEY,
    byte_length         BIGINT NOT NULL,
    first_referenced_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    last_referenced_at  TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    CONSTRAINT block_ref_len_nonneg   CHECK (byte_length >= 0),
    CONSTRAINT block_ref_seen_ordered CHECK (last_referenced_at >= first_referenced_at)
);
CREATE INDEX block_ref_gc_idx ON block_ref (last_referenced_at);
```

`advance()` CAS (mirrors `advance_run_binding`): first write `INSERT ... fence=1 ON CONFLICT DO NOTHING`;
advance `UPDATE ... SET manifest_digest=$2, fence=$expected+1, updated_at=transaction_timestamp() WHERE
lineage_id=$1 AND fence=$expected` → `StaleFence` if `rows_affected != 1`. `Fence(u64) ↔ BIGINT(i64)` via
guarded conversion (adopt `signed`/`positive_u64`), never raw cast.

`block_ref` touch (batched, on every publish): `INSERT INTO block_ref(block_digest, byte_length) SELECT *
FROM unnest($1::text[], $2::bigint[]) ON CONFLICT (block_digest) DO UPDATE SET last_referenced_at =
clock_timestamp()`.

## 5. Phases (each: implement → senior-review gate → rustfmt/clippy/test → push)

### Phase 1 — `S3BlobStore` (feature `s3`)
- Implement all `BlobStore` methods + new `delete_block`/`delete_manifest` against `aws-sdk-s3` 1.x.
- Client builder honoring `S3_ENDPOINT_URL`/`force_path_style`/`S3_BUCKET`; one held `Runtime`.
- Keys `blocks/<id>`, `manifests/<digest>`. `has_block`→HeadObject (404⇒false). `put_block`→optional
  HeadObject dedup-skip then PutObject. `get_*`→GetObject, `NoSuchKey`⇒typed `NotFound` error (distinct
  from transient). Error enum mirrors `kms.rs` (downcast `.meta().code()`, secrets out of `Debug`).
- **Tests**: (a) local, against MinIO/localstack via `S3_ENDPOINT_URL` (round-trip, dedup-skip, NotFound,
  delete idempotency); gated behind an env so `cargo test` without a bucket still passes. (b) AWS
  integration test (real bucket in 993939946442) — same suite, gated behind `S3_IT=1` + creds.
- Acceptance: `LocalBlobStore` and `S3BlobStore` pass an identical trait-conformance test module.

### Phase 2 — `PgLineageStore` + `block_ref` (feature `pg`)
- `migrations/0001_bootstrap.sql` (§4) applied via `raw_sql`. Native `PgPool` from `DATABASE_URL`; one
  held `Runtime`. `sqlx =0.9` minimal features + `uuid`,`chrono`.
- `LineageStore` (sync trait, `&self`, block_on): `get`, `advance` (CAS above). `StaleFence` on `!=1`.
- A `RefClock`/liveness surface: `touch(&[BlockId],&[u64])`, `candidates_older_than(grace) -> Vec<BlockId>`,
  `forget(&BlockId)`. Postgres impl = the `block_ref` upsert/select/delete above.
- **Tests**: `testcontainers` `postgres:17.10-alpine` — fence CAS happy path; concurrent advance (only one
  of two racing `expected=N` wins, other gets StaleFence); reuse-clock (touch bumps `last_referenced_at`);
  candidates honor grace. AWS integration (real PG/RDS in 993939946442) gated behind `PG_IT=1`.

### Phase 3 — Postgres-driven GC
- Extract `mark_live_blocks(&dyn BlobStore, &[String])`; refactor filesystem `gc::collect` to call it
  (behavior + 7 tests unchanged).
- New `gc_pg::collect(&dyn BlobStore, &RefClock, live: &[String], grace)`: candidates = `RefClock.
  candidates_older_than(grace)`; keep = mark set; for each candidate ∉ keep: `blobstore.delete_block`
  (S3) then `RefClock.forget` (row). Manifest GC analogous (superseded HEADs).
- Port the three-invariant regression tests (esp. crash-retry F1) to the PG clock: a reused block's
  `last_referenced_at` is refreshed on republish ⇒ grace protects it. Also concurrency-safe sweep.
- **Tests**: testcontainers PG + `LocalBlobStore` (fast) and gated AWS S3+PG (full).

### Phase 4 — `daemon` subcommand + ifaces corrections
- Extend `main.rs`: `daemon --tree --store <uri> --lineage --fence --publish-interval [--health-addr]`:
  materialize-latest-on-start (if HEAD exists) → mark ready → interval publish loop (fence-guarded via
  `advance`) → SIGTERM: one final publish, exit 0. `store <uri>` dispatches `file://` (LocalBlobStore) vs
  `s3://` (S3BlobStore); lineage store `postgres://` vs local file. tokio signals + 30s drain +
  minimal `/health` (raw TcpListener, no heavy http dep) mirroring `common::server` conventions.
- **ifaces.rs corrections** (honesty): mark which types are literal zeroshot-cloud mirrors vs. neutral
  generalizations; restore the 3 dropped `RecoveryStatusCompletedParts` fields (`run_id`,
  `reservation_id`, `fence`) for a true drop-in.
- **Tests**: daemon lifecycle over `file://`+testcontainers PG (start→materialize→publish×N→SIGTERM→final
  publish; a second daemon with a stale fence is rejected); health responds.

### Phase 5 — AWS end-to-end integration + measurement
- Provision throwaway S3 bucket + Postgres in **993939946442** (bucket: plain, versioning off; PG: RDS
  `db.t4g.micro` OR containerized PG on an EC2 via SSM — pick cheapest that works; torn down after).
- End-to-end on real AWS: publish → GC(grace) → materialize round-trip; crash-retry F1 with the PG
  reuse-clock; fence CAS under two concurrent publishers; **cold-materialize wall-time for a realistic
  1–5 GB tree vs the 61s activation budget** (ties to R2). Record numbers in the build log + issue #744.
- Tear down all AWS resources; confirm nothing left running (cost hygiene, as the prototype experiments).

## 6. Dependencies / cost / interactive gates

- Phases 1–4 need no AWS for their fast tests (MinIO + testcontainers). AWS integration (`*_IT=1` tests +
  Phase 5) needs the user to run `aws sso login` (I don't authenticate as them) and a throwaway
  bucket+DB in 993939946442 (est. < $1, torn down). Flag before each AWS step.
- New deps behind features: `aws-sdk-s3`/`aws-config`/`tokio` (`s3`), `sqlx`/`tokio` (`pg`),
  `testcontainers` (dev). Default build stays lean (no AWS/PG) so the CPU-rate measurement path is intact.

## 7. Risks

- **Nested-runtime panic** if a trait method is ever called from an async context. Mitigation: adapters
  are only used from the sync pipeline / CLI; add a doc-comment guard + a debug-assert.
- **testcontainers needs Docker** in the dev env. If absent, PG unit tests degrade to AWS-only (report it,
  don't silently skip — repo rule).
- **S3 eventual consistency**: S3 is strongly read-after-write for new objects now, so `put`→`head`→`get`
  is safe; still map 404 explicitly.
- **Clock authority**: mixing file-mtime (local) and PG-clock (prod) GC — keep them as two separate
  `collect` entry points to avoid a half-migrated clock (documented invariant).

## 8. Open questions for the reviewer to rule on

1. GC surface: one `RefClock` trait with fs + pg impls (unify local & prod GC), or keep filesystem GC
   entirely separate and only add pg GC? (Plan leans: extract MARK only; keep two `collect` fns.)
2. Daemon health: minimal raw-TCP `/health` now, or stub health entirely until integration?
3. Phase 5 Postgres: RDS `db.t4g.micro` vs containerized PG on EC2/SSM — which is the cheaper/faster
   throwaway for a faithful "real AWS" test?
4. Should `put_block`'s S3 dedup-skip HeadObject be kept (one extra round-trip per block) or dropped in
   favor of unconditional idempotent PutObject (content-addressed ⇒ safe to overwrite)?
