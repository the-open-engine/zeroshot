# Plan 0001 — Production backends for the capsule workspace store (standalone)

Status: **APPROVED-WITH-CHANGES (Revision 1, post senior-review)**
Owner: capsule-workspace-core
Branch: `claude/zeroshot-workspace-file-transfer-13f0be` (extend; not branching from main)
Companion notes: `docs/research/production-build-log.md` (research findings A/B/C + decisions + review verdict)

## Revision 1 changelog (post senior-review)
Review verdict was REVISE-AND-RESUBMIT with 4 must-fixes; all folded in here (see build log for the full
verdict). Headlines:
- **MF1 (R1-critical):** GC delete is now an atomic **row-claim (age re-checked server-side) → then
  DeleteObject**, NOT S3-first. Adds the operational invariant grace > max(publish, sweep) duration,
  single-flight via PG advisory lock, and a required concurrent-publish-vs-GC test. (§3, Phase 3)
- **MF2:** daemon threading model pinned (sync `fn main`, isolated infra runtime, multi-thread adapter
  runtime, no ambient runtime under the pipeline). (§3, Phase 4)
- **MF3:** `known ⊆ mark-set` made mechanical (dedup `known` = parent live-HEAD's chunk index). (§3, Phase 4)
- **MF4:** publish commit ordering + touch set pinned (touch all referenced blocks young BEFORE upload →
  upload → put_manifest → advance). (§3, Phase 2/4)
- Open questions §8 resolved with rulings (OQ1 concrete-typed local GC + RefClock fake; OQ2 build health
  now; OQ3 RDS t4g.micro once; OQ4 drop HeadObject-skip). 

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
  `gc::collect` keeps its mtime clock + all 7 tests **and takes the concrete `&LocalBlobStore`** (so an S3
  store can't be passed to the mtime collector — a half-migrated clock won't compile, MF/OQ1). The new
  `gc_pg` sweep reuses the same mark helper, takes candidates from `block_ref` via a `RefClock` trait
  (PG impl + an in-memory fake for unit tests), and uses the atomic-claim delete below.
- **GC three-invariant model carried to prod** (from the design doc):
  1. grace ≥ max_publish + max_clock_skew — now a single PG `clock_timestamp()` clock (cross-host safe),
     not file mtime.
  2. mark-set covers every live manifest — GC reads live HEADs (within retention) and marks their blocks.
  3. refresh-on-every-reference — `block_ref` upsert `ON CONFLICT DO UPDATE SET last_referenced_at =
     clock_timestamp()` on every publish that references a block (the F1 fix in prod).
- **GC delete ordering (MF1 — R1-critical, corrected).** NOT S3-first (that races a concurrent publisher
  and drops live bytes — proven interleaving in the build log). GC claims each candidate with an **atomic
  guarded delete that re-checks age SERVER-SIDE**, then deletes the object only on a won claim:
  ```sql
  DELETE FROM block_ref
    WHERE block_digest = $1 AND last_referenced_at < clock_timestamp() - $grace::interval
    RETURNING block_digest;              -- won the claim iff a row is returned
  ```
  then `DeleteObject`. Grace is evaluated against **`clock_timestamp()` (PG server clock)**, never
  `SystemTime::now()` on the GC host (no cross-host skew — the whole point of the PG clock). Supporting
  rules: **single-flight GC** via a PG advisory lock; **per-block** claim→delete (no batch-claim-then-loop,
  which widens the straddle); operational invariant **grace > max(publish duration, GC sweep duration)**
  (recommend ≥ 2× expected max publish). Residuals (documented, fine for a measurement build): crash
  between row-delete and DeleteObject ⇒ re-driveable **orphan object** (cost leak, not live-byte loss) →
  periodic orphan reconciler as follow-up; a very narrow straddle closed fully only by a per-publisher
  lease (out of scope, noted).
- **Publish commit ordering + touch set (MF4 — load-bearing for F1/R1).** Per publish, in order:
  **(1) touch `block_ref` for every referenced block (young), (2) upload new blocks (unconditional
  idempotent PutObject — OQ4), (3) `put_manifest`, (4) `advance` (fence CAS).** Touch precedes upload and
  fires even when the object write is skipped/idempotent — F1 protection is the touch, not the write; the
  unconditional re-upload also RESTORES a block GC deleted mid-race. Touch set = every referenced block
  (batched `unnest` upsert). With grace > publish duration, a to-be-referenced block stays young through
  the whole publish, so GC's age-re-checked claim can't take it; after commit it's protected by the MARK
  (live-HEAD reachability).
- **`known ⊆ mark-set`, mechanically (MF3).** The daemon's dedup `known` MUST be the parent live-HEAD's
  chunk index (which GC marks), so a dedup-reused block can never come from a manifest outside the mark set
  (invariant #2). Single source of truth, not a documented hope.
- **Daemon threading (MF2) — the one way the sync/`block_on` bet panics.** Plain sync `fn main()`; infra
  (SIGTERM/SIGINT + `/health`) on an **isolated** runtime; the pipeline on plain std/rayon threads with
  **no ambient runtime**. Each adapter holds a **multi-thread** runtime (bounded 2–4 workers) because
  `materialize` calls `get_block` concurrently from `par_iter` → concurrent `block_on` (a current-thread rt
  would contend/panic). Adapter methods carry a debug-assert `Handle::try_current().is_err()` tripwire.
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

`advance()` CAS (mirrors `advance_run_binding`), **dispatched on `expected`**: `expected == 0` (first
write) → `INSERT ... fence=1 ON CONFLICT DO NOTHING`; `expected >= 1` → guarded `UPDATE ... SET
manifest_digest=$2, fence=$expected+1, updated_at=transaction_timestamp() WHERE lineage_id=$1 AND
fence=$expected`. `StaleFence` if `rows_affected != 1` — **except** the idempotent-retry case: on
StaleFence, re-`get()` HEAD; if it already equals `(my digest, expected+1)` treat as **success** (a
lost-ack on the final SIGTERM publish is durable, not an R1 alarm). A naive always-UPDATE bricks the first
publish (no row → StaleFence forever) — hence the dispatch. `Fence(u64) ↔ BIGINT(i64)` via guarded
conversion (adopt `signed`/`positive_u64`), never raw cast.

`block_ref` touch (batched, on every publish): `INSERT INTO block_ref(block_digest, byte_length) SELECT *
FROM unnest($1::text[], $2::bigint[]) ON CONFLICT (block_digest) DO UPDATE SET last_referenced_at =
clock_timestamp()`.

## 5. Phases (each: implement → senior-review gate → rustfmt/clippy/test → push)

### Phase 1 — `S3BlobStore` (feature `s3`)
- Implement all `BlobStore` methods + new `delete_block`/`delete_manifest` against `aws-sdk-s3` 1.x.
- Client builder honoring `S3_ENDPOINT_URL`/`force_path_style`/`S3_BUCKET`; hold a **multi-thread** runtime
  (bounded 2–4 workers) + the `Handle::try_current().is_err()` debug-assert tripwire (MF2).
- Keys `blocks/<id>`, `manifests/<digest>`. `has_block`→HeadObject (404⇒false, kept as its own method).
  `put_block`/`put_manifest`→ **unconditional idempotent PutObject, NO HeadObject-skip (OQ4)** —
  content-addressed ⇒ overwrite is byte-identical, common path is new blocks, and unconditional re-upload
  is what restores a block GC deleted mid-race. `get_*`→GetObject, `NoSuchKey`⇒typed `NotFound` error
  (distinct from transient). `delete_*`→DeleteObject, idempotent (404⇒`false`). Error enum mirrors
  `kms.rs` (downcast `.meta().code()`, secrets out of `Debug`).
- **Also update the 2 `FlakyStore` test doubles** (`audit_independent.rs`, `adversarial3.rs`) for the new
  `delete_block`/`delete_manifest` trait methods (delegate to `self.inner`) so "50 green" holds.
- **Tests**: (a) local, against MinIO/localstack via `S3_ENDPOINT_URL` (round-trip, NotFound, delete
  idempotency, unconditional-overwrite-is-identical); gated behind an env so `cargo test` without a bucket
  still passes. (b) AWS integration (real bucket in 993939946442) — same suite, gated behind `S3_IT=1`.
- Acceptance: `LocalBlobStore` and `S3BlobStore` pass an identical trait-conformance test module.

### Phase 2 — `PgLineageStore` + `block_ref` (feature `pg`)
- `migrations/0001_bootstrap.sql` (§4) applied via `raw_sql`. Native `PgPool` from `DATABASE_URL`; held
  multi-thread runtime + tripwire (MF2). **Pre-flight: verify `sqlx 0.9` resolves standalone** (research
  read `=0.9.0` from zeroshot-cloud; if crates.io latest is 0.8.x, use it — semantics identical for us).
  Minimal features + `uuid`,`chrono`.
- Trait change: `LineageStore::advance` `&mut self`→`&self` (`FileLineageStore` gets a `Mutex`); update the
  one caller `let mut ls`→`let ls` in `experiments.rs`. Sync trait, block_on inside. `advance` per the
  dispatched CAS + idempotent-retry above.
- A `RefClock` trait: `touch(&[BlockId],&[u64])` (batched upsert, `DO UPDATE SET last_referenced_at =
  clock_timestamp()`), `claim_collectable(&BlockId, grace) -> bool` (the atomic guarded `DELETE ...
  RETURNING`, MF1), `candidates_older_than(grace) -> Vec<BlockId>` (the pre-filter). **Two impls: PG + an
  in-memory fake** (unit-test the prod sweep without testcontainers, OQ1).
- **Tests**: `testcontainers` `postgres:17.10-alpine` — fence CAS happy path; concurrent advance (one of
  two racing `expected=N` wins, other StaleFence); idempotent retry (re-advance to the same HEAD =
  success); reuse-clock (touch bumps `last_referenced_at`); atomic claim (a touch between candidate-select
  and claim ⇒ claim returns 0 rows). AWS integration (real RDS in 993939946442) gated behind `PG_IT=1`.

### Phase 3 — Postgres-driven GC
- Extract `mark_live_blocks(&dyn BlobStore, &[String])`; refactor filesystem `gc::collect` to call it, but
  keep it taking the **concrete `&LocalBlobStore`** (unmixable clock, OQ1) — behavior + 7 tests unchanged.
- New `gc_pg::collect(store, clock: &dyn RefClock, live, grace)` under a **single-flight PG advisory lock**:
  mark = live-HEAD blocks; for each `clock.candidates_older_than(grace)` ∉ mark, **atomic
  `clock.claim_collectable(b, grace)` (age re-checked server-side) → only on a won claim
  `store.delete_block(b)`** (per-block, MF1). Manifest GC analogous (superseded HEADs). Document the orphan
  residual + reconciler.
- Port the three-invariant regressions to the PG clock (esp. crash-retry F1: republish touches the reused
  block young ⇒ grace protects it). **NEW required test (MF1): a publish running CONCURRENTLY with a GC
  sweep drops no live block** (the serialized F1 test stays green while this race is latent).
- **Tests**: in-memory `RefClock` fake + `LocalBlobStore` (fast, no Docker); testcontainers PG (full);
  gated AWS S3+PG.

### Phase 4 — `daemon` subcommand + ifaces corrections
- Extend `main.rs` with a **sync `fn main()`** (MF2): `daemon --tree --store <uri> --lineage --fence
  --publish-interval [--health-addr]`: materialize-latest-on-start (if HEAD exists) → mark ready → interval
  publish loop → SIGTERM: one final publish, exit 0. Each publish follows the MF4 ordering (touch → upload
  → put_manifest → advance) and seeds dedup `known` from the **parent live-HEAD chunk index** (MF3).
  `store <uri>` dispatches `file://`(Local) vs `s3://`(S3); lineage `postgres://` vs local file.
- **Threading (MF2)**: infra (SIGTERM/SIGINT + minimal raw-TCP `/health` + readiness latch, OQ2) on an
  **isolated** runtime; the publish/materialize pipeline runs on std/rayon with **no ambient runtime** (so
  adapter `block_on`s never nest); 30s drain mirroring `common::server`. Readiness = materialize-on-start
  done. No metrics endpoint (YAGNI).
- **ifaces.rs corrections** (honesty; separate tiny commit, non-gating): label literal zeroshot-cloud
  mirrors vs. neutral generalizations; restore the 3 dropped `RecoveryStatusCompletedParts` fields
  (`run_id`, `reservation_id`, `fence`).
- **Tests**: daemon lifecycle over `file://`+testcontainers PG (start→materialize→publish×N→SIGTERM→final
  publish; a second daemon with a stale fence is rejected; idempotent-retry final publish = success);
  `/health` responds; the daemon actually drives a publish through the S3 adapter's real threading (MF2
  smoke — proves no nested-runtime panic).

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

## 8. Open questions — RESOLVED by senior-review

1. **GC surface** → extract MARK only; keep two `collect` fns; local collector takes the concrete
   `&LocalBlobStore` (mtime-vs-S3 unmixable, won't compile); prod sweep behind a `RefClock` trait (PG impl
   + in-memory fake). (Applied: §3, Phase 2/3.)
2. **Daemon health** → build the minimal raw-TCP `/health` + readiness latch NOW — it's the cheapest test
   of the daemon threading model (MF2). No metrics endpoint. (Applied: Phase 4.)
3. **Phase 5 Postgres** → RDS `db.t4g.micro` (faithful to what zeroshot-cloud runs; provision ONCE for
   both `PG_IT=1` and the e2e; `--skip-final-snapshot` teardown). testcontainers stays the fast inner loop.
   (Applied: Phase 5.)
4. **`put_block` dedup-skip** → drop the HeadObject; unconditional idempotent PutObject (content-addressed
   ⇒ byte-identical overwrite; common path is new blocks; and it restores a block GC deleted mid-race).
   Keep `has_block` separate; F1 protection is the `block_ref` touch, decoupled from the write. (Applied:
   Phase 1.)

## 9. Definition of done (per phase gate)
Each phase: `cargo fmt` clean, `cargo clippy` no new lib warnings, all tests green (existing 50 + new),
senior-review approval, branch pushed. AWS-gated tests (`*_IT=1`) run in Phase 5 (needs user SSO). Full
build log entry + issue #744 comment per phase.
