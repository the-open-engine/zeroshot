# Capsule Workspace — Production Build Log

Running log for the production build-out of the capsule workspace store (S3 `BlobStore`,
Postgres `LineageStore` + GC metadata, daemon wiring) behind the prototype's existing traits.
Companion to `design-decision-experiments.md` (the prototype/experiment phase). Notes are appended
per finding; the branch is pushed after each approved phase.

## Objective

Turn the validated prototype (`capsule-workspace-core`, 50 tests, three-invariant GC model) into a
production-shaped implementation of the two backend swap points + the daemon, **standalone** — NOT
integrated into the rest of the platform yet:

1. **`S3BlobStore`** — real S3 behind the `BlobStore` trait (`put_block`/`get_block`/`put_manifest`/
   `get_manifest`/`has_block`). S3 objects have no mutable mtime we control, so the GC liveness
   clock moves off the object and into Postgres (see #2). Content-addressed keys; multipart for
   large blocks; NotFound-vs-error mapping.
2. **`PgLineageStore`** — real Postgres behind the `LineageStore` trait (`get`/`advance` with the
   monotonic fence = single-writer CAS), matching zeroshot-cloud's `capsule_attempts.fencing_token`
   semantics. Plus a **`block_ref` table** carrying `last_referenced_at` per block, refreshed on
   EVERY reference (the F1 reuse-clock, translated to prod), so grace-period GC has a reliable
   cross-host clock instead of file mtime.
3. **Daemon wiring** — a standalone process/CLI that publishes the local tree to S3+PG and
   materializes it back, shaped (CLI/config/lifecycle: publish interval, SIGTERM, materialize-on-
   start) to drop into a capsule pod later. No pod/k8s integration yet.

## Constraints (from the user, must hold)

- **Test everything on AWS** (real S3 + real Postgres), **internal account 993939946442 ONLY**
  (dev account 794285265617 is live/shared — never use it). Same discipline as the prototype's EC2
  experiments (SSM, no keypair).
- **Do NOT integrate into the rest of the platform yet** — standalone crate only; compatibility
  seam (`ifaces.rs`) is honored but nothing is wired into zeroshot-cloud services.
- **Take notes** (this file) and **keep the branch pushed** after each approved phase.
- No `Co-Authored-By:` trailer on commits (global pref).
- Test only realistic scenarios + edge cases; no contrived threat models.
- I do not run `aws sso login` myself — the user authenticates; I drive the rest.

## Process — team-programming skill (from zeroshot-cloud `.claude/skills/team-programming`)

Flow: research → plan (planner + senior-reviewer iterate to approval) → implement phase-by-phase,
each phase gated on senior-reviewer approval → push every approved phase → full relevant test suite
before declaring complete. Adaptations for this context:
- **Not** branching fresh from `origin/main` (the prototype is on the current feature branch
  `claude/zeroshot-workspace-file-transfer-13f0be`; extend it, keep it pushed).
- **k8s local (kind) gate deferred**: the skill requires kind testing for k8s-facing deploys, but
  we are explicitly not deploying/integrating. Recorded as intentional degraded coverage for now;
  AWS S3+PG integration testing is the gate we DO run.

## Timeline / findings

### 2026-07-21 — Phase 0: kickoff
- Marked new chapter. Set up `planning/plans/`, this log.
- Spawned 3 read-only research agents against `zeroshot-cloud` (main checkout): (a) S3/object-store
  patterns, (b) Postgres/lineage/fencing schema, (c) capsule pod runtime + daemon contract.
- Next: synthesize research → write plan → planner/senior-reviewer iterate.

### 2026-07-21 — Research finding A: S3 in zeroshot-cloud
- **No existing Rust S3 code.** Backend links `aws-config =1.9.0`, `aws-sdk-kms`, `aws-sdk-rds`; NO
  `aws-sdk-s3`, no rusoto/object_store. So "drop-in" = match *conventions*, not replace code. We are
  the first S3 consumer.
- **S3 is provisioned but keyless**: IaC `iac/modules/blob` makes a per-cell bucket `${prefix}-blob`
  (SSE-KMS, versioned, TLS-only, VPCE-locked); `data-cell` binds `prefix_intents = {}` (no keys yet).
  Multipart actions are already in the IAM action sets.
- **Client idiom to match**: `aws_config::load_defaults(BehaviorVersion::latest()).await`; default
  provider chain (IRSA in EKS), region/creds/endpoint from env — no hardcoding. Feature flags on
  their AWS deps: `default-features=false, ["behavior-version-latest","default-https-client","rt-tokio"]`.
- **MinIO is the S3 test double** via env `S3_ENDPOINT_URL` / `S3_BUCKET` / `AWS_REGION` — but no Rust
  reads them yet. Our `S3BlobStore` should honor that contract: `S3_ENDPOINT_URL` present ⇒
  `force_path_style(true)` (MinIO/localstack), absent ⇒ real virtual-hosted S3. Lets us test locally
  without AWS AND on real AWS with the same code.
- **Error model to mirror** (`admin/src/kms.rs`): downcast `err.as_service_error().meta().code()`,
  fail-closed enum, secrets out of `Debug`. We must add NotFound ourselves (typed `NoSuchKey` +
  `HeadObject` 404) — the GC/republish logic depends on NotFound ≠ generic error.
- **No S3 lifecycle policy anywhere → GC is app-managed** (confirms our design). Bucket versioning is
  Enabled, so real reclamation needs `DeleteObjectVersion`, not just `DeleteObject`. (For our own
  throwaway test bucket we'll disable versioning to keep delete semantics simple.)
- **Testing**: `testcontainers 0.27.3` is in-repo (used for PG/Redis, not S3); real-AWS interface
  tests are gated behind a feature/env (template: `admin/tests/kms_provider.rs`).

**Decisions for our standalone build (informed by A):**
- Test on **our internal account 993939946442** with our **own throwaway bucket** (plain, no VPCE/KMS
  needed for a functional test) — zeroshot-cloud's real `-blob` bucket (accounts 794285265617 /
  703091483677) is for *later* integration, not this standalone phase. The account "mismatch" the
  agent flagged is expected and correct.
- `aws-sdk-s3` **1.x latest** (standalone crate = own Cargo.lock, no need to co-resolve zeroshot-cloud's
  smithy versions). Behind the existing `s3` feature.
- Key scheme `blocks/<id>`, `manifests/<digest>` (flat hex, mirrors `LocalBlobStore`).
- `has_block`→HeadObject(404=false); `put_block`→optional HeadObject dedup-skip then PutObject
  (blocks ≤64 MiB fit a single PUT; multipart is a later retry-granularity optimization, not v1).

**STRATEGIC FLAGS to raise with the user before/at plan review (do NOT silently decide):**
1. **Kopia overlap** — the zeroshot-cloud spec's *intended* S3 uploader is **Kopia** (already
   content-addressed/dedup/chunked). Our custom chunk store reimplements Kopia's job. Building the
   prototype was effectively the decision to evaluate custom-vs-Kopia; worth an explicit confirm.
2. **This data plane is the DEFERRED/experimental lane** — V1 uses plain gp3 EBS PVCs; the
   content-addressed-chunk-store-to-S3 path is the "optional performance experiment," metric-gated to
   a later version (matches the hardened spec). Standalone build is consistent with "don't integrate
   yet," but the user should know we're building the experimental lane, not the shipping V1 path.

### 2026-07-21 — Research finding B: Postgres/fencing/lineage in zeroshot-cloud
- **Stack**: `sqlx =0.9.0` (workspace-pinned) against **PostgreSQL 17.10**. 100% runtime `sqlx::query`
  (NO `query!` compile-time macros, NO `.sqlx` dir → no build-time DATABASE_URL). Native `PgPool` in a
  repository struct (api-gateway/billing) is the simplest template; admin uses bb8+RDS-IAM tokens.
- **Migrations**: single edit-in-place `migrations/0001_bootstrap.sql` applied via
  `sqlx::raw_sql(include_str!(...))`. Not `sqlx::migrate!`.
- **Fence CAS = my `advance()` almost verbatim.** Their `advance_run_binding`
  (`api-gateway/src/billing/prestart_recovery.rs`):
  `UPDATE ... SET fencing_token=$next WHERE ... AND fencing_token=$expected` then
  `require_one(rows_affected)` → `Stale` if `!= 1`. Orchestrator `capsule_control.rs` is a pure
  in-memory mirror (reject fence `< current`, require exactly `current+1`). **Our model is directly
  compatible.**
- **Conventions**: snake_case plural tables; UUID PKs (`Uuid::now_v7`); `TIMESTAMPTZ` only;
  `transaction_timestamp()` for created/updated, **`clock_timestamp()` for "now" reads inside a tx**;
  counters/fence = `BIGINT` + `CHECK (>0)` mapped to `u64` via `signed()`/`positive_u64()` helpers;
  `TEXT`+`CHECK IN (...)` enums; `BYTEA` opaque; `JSONB` structured. **Refresh-on-use precedent:
  `login_sessions.last_seen_at TIMESTAMPTZ` + `CHECK (last_seen_at >= created_at)`** — the model for
  our GC clock.
- **Testing**: `testcontainers =0.27.3` (Rust crate) spins Docker `postgres:17.10-alpine`; tests
  tagged `// test-tier: integration|interface`; testcontainers drives Docker (no CI services: block).
- **DDL recommendations** (mirroring their conventions): `lineage_head(lineage_id, manifest_digest,
  fence BIGINT CHECK>0, timestamps)`; `block_ref(block_digest PK, byte_length, first/last_referenced_at,
  CHECK last>=first, INDEX on last_referenced_at)`. Reuse `signed()`/`positive_u64()`. StaleFence on
  `rows_affected != 1`.

**Decisions for our standalone build (informed by B):**
- **Own schema, own DB, native `PgPool`** — we are NOT extending a zeroshot-cloud service's
  `0001_bootstrap.sql` (that's integration, deferred). Ship our own bootstrap SQL applied via
  `sqlx::raw_sql(include_str!(...))`, exactly their mechanism, in our crate.
- `lineage_id` = **TEXT** (our `LineageId(String)`), independent monotonic fence following the same
  rules (NOT literally sharing `capsule_attempts.fencing_token` — that transactional linearization is
  an integration-time decision). Reconcile fence base: "no row" = conceptual fence 0; first write
  `INSERT ... fence=1 ON CONFLICT DO NOTHING`; advance = guarded `UPDATE ... WHERE fence=$expected`.
- **Keep the sync trait; bridge with `block_on`** (per the async discussion) — BUT change the trait
  from `advance(&mut self)` to `advance(&self)` so a shared pool works; `FileLineageStore` gets interior
  mutability (Mutex) to match. `PgLineageStore` holds the runtime + `PgPool`.
- Map `Fence(u64) ↔ BIGINT(i64)` with guarded conversion (adopt `signed`/`positive_u64`), never raw cast.
- Use `clock_timestamp()` for `last_referenced_at` (single authoritative PG server clock — the
  cross-host GC clock, replacing file mtime; this is F1's invariant #3 in prod).

**GC reachability — the key design point (invariant #2 in prod).** `block_ref.last_referenced_at`
gives the grace CLOCK but **not reachability**: a block committed a week ago by a still-live manifest
has an old `last_referenced_at`, so grace-clock-alone would wrongly collect it. So prod GC keeps the
prototype's **mark step**: read the live manifests (HEADs within retention, from `lineage_head` →
blob store), mark their referenced blocks, and sweep `block_ref` rows that are `(not marked) AND
(last_referenced_at < now - grace)`. A `manifest_block(manifest_digest, block_digest)` edge table
would let the sweep run purely in SQL (`AND NOT EXISTS ... live manifest references`) — noted as an
**optimization**, not v1 (don't over-engineer; the prototype already marks by reading manifests).
**Delete ordering**: remove the S3 object FIRST, then the `block_ref` row (a crash leaves a re-driveable
orphan row, not a leaked object) — mirrors the prototype's NotFound-tolerant idempotent delete.

### 2026-07-21 — Research finding C: capsule pod runtime + daemon contract
- **Pod is EXACTLY two containers** (golden-tested `pod_contract.rs` + admission-enforced
  `admission.yaml`): trusted `capsule-agent` (uid 65532, mounts agent-state `/var/lib/zeroshot-capsule-agent`)
  + tenant `runtime-stub-v1` (uid 65533, mounts workspace `/workspace`), sharing a `/run/zeroshot-capsule`
  emptyDir (Unix-socket control plane). **A third sidecar cannot be added** without changing the golden
  contract + admission policy → integration home is later capsule-agent-fold OR node-level checkpoint-agent.
- **Today's durability = physical volume reattach** (NVMe TopoLVM VolumeSnapshots XOR encrypted EBS,
  admission "never both"); recovery fences the old runtime and re-binds the **same PVC UIDs in the same
  AZ** (`runtime_fencing.rs validate_replacement`). Our content store **replaces this** and removes the
  AZ-lock. The node bootstrap builds an LVM thin-pool with **chunksize 256K — matches our `CHUNK`**.
- **Lifecycle conventions to follow** (`common/src/server.rs`): SIGINT+SIGTERM via tokio, readiness
  latch (start unready), `/health`, `BIND_ADDR` env, **30s graceful drain**; deadline budget is
  `run_deadline − 61s` (materialize must fit); no built-in checkpoint interval (daemon owns its timer);
  non-root/read-only-rootfs/no-SA-token → AWS creds via **IRSA/Pod Identity**, not mounted secrets.
- **Lifecycle hook mapping**: materialize at Prepared→Activating (before the runtime arms, ≤61s);
  publish during Running on an interval (peer of the heartbeat cadence), fence-guarded; final publish on
  SIGTERM within 30s.

**RESEARCH CORRECTION (honesty — the prototype overstated its "mirror"):** `ifaces.rs` claims "every
type mirrors an existing zeroshot-cloud shape (verified 2026-07-20)." Reality from finding C:
- REAL mirrors: `Fence` ↔ `cloud_run_billing.fencing_token`/orchestrator `current_fence` ✅;
  `RecoveryStatusCompletedParts` ↔ `common::funded_run::recovery::RecoveryStatusCompletedParts` ✅
  (though our copy DROPS `run_id`, `reservation_id`, `fence` — finding B mismatch #3).
- GENERALIZATIONS (do NOT literally exist in zeroshot-cloud): `ClaimRole`, `ProviderNeutralClaim(s)`,
  `fake_claim_uid` — the real shapes are `PersistentVolumeClaimIntent`/`PersistentVolumeClaims` +
  `BoundClaim{pvc_uid, availability_zone}`. `ArtifactRef{sha256,byte_length}` is not a struct either;
  it's the shape of the sidecar `RunnerToAgent::Result` receipt.
- ACTION: keep the neutral generalizations (they're a fine abstraction seam) but **correct the ifaces
  header** to say which types are literal mirrors vs. neutral generalizations, and restore the 3 dropped
  `RecoveryStatusCompletedParts` fields for a true drop-in. (Fold into the impl phases.)

**Scope decisions for the standalone daemon (informed by C):**
- **Workspace tree ONLY** this phase. Agent-state is the trusted sidecar's live SQLite store — publishing
  it needs a consistent DB checkpoint (WAL/VACUUM), a different problem than an opaque file tree. Out of
  scope; note it.
- **CLI** (extend `main.rs`): `publish`, `materialize`, `daemon` (materialize-on-start → ready →
  interval publish → SIGTERM final publish), `gc`. Config via env+flags (`WORKSPACE_DIR`, `STORE_URI`
  `file://`→`s3://`, `LINEAGE_ID`, `FENCE`, `PUBLISH_INTERVAL_SECS`, `BIND_ADDR`), AWS creds via default
  chain / IRSA.
- **Fence authority = our `lineage_head` CAS** (PG). The daemon refuses a stale publish because
  `advance()` fails when its `expected` fence doesn't match — no external fence source needed for standalone.
- **Stays stubbed (not integrating):** no k8s API calls, no VolumeSnapshot/PVC, no control-plane
  callbacks; the daemon just reads/writes the store + PG and (optionally) exposes health.
- **Worth measuring on AWS**: cold-materialize wall-time vs the 61s activation budget for a realistic
  1–5 GB `/workspace` (ties back to R2 <5s resume).

### 2026-07-21 — Plan drafting
- All 3 research reports synthesized above. Writing `planning/plans/0001-production-backends.md`, then an
  independent senior-reviewer agent iterates it to approval (team-programming step 3) before any code.

### 2026-07-21 — Plan review round 1: REVISE-AND-RESUBMIT (high-value finding)
Independent senior-reviewer verdict on plan 0001. Endorsed the architecture (sync/`block_on` bet,
scoping discipline, two-tier testing, materialize integrity carrying to S3 for free) but found **one
genuine R1-corrupting flaw** + 3 more must-fixes. Resolutions folded into plan Revision 1:

- **MF1 (critical) — GC delete ordering races a concurrent publisher → drops live bytes.** My plan's
  "DeleteObject(S3) first, then block_ref row" is backwards: under a concurrent publish+GC, GC computes
  candidates (old, unmarked) at select time, then a racing republish touches the block young, but GC's
  delete doesn't re-check freshness → deletes a block the about-to-commit manifest needs. The prototype
  only ever tested publish and GC *serialized*, so this was unproven, not just untested (the fs GC has
  the same latent TOCTOU). **Fix:** GC claims each block via an atomic **`DELETE FROM block_ref WHERE
  block_digest=$1 AND last_referenced_at < clock_timestamp() - $grace RETURNING`** (age re-checked
  SERVER-SIDE at delete, never `SystemTime::now()` on the GC host → no cross-host skew), and only on a
  won claim does it `DeleteObject`. Operational invariant added: **grace > max(publish duration, GC
  sweep duration)**; single-flight GC via a PG advisory lock; per-block claim-then-delete (no
  batch-claim-then-loop); documented residuals (crash between row-delete and object-delete = re-driveable
  orphan object = cost leak, not live-byte loss; + a narrow straddle fully closed only by a per-publisher
  lease — out of scope) + a periodic orphan-object reconciler as follow-up. **New required test: publish
  running CONCURRENTLY with a GC sweep asserts no live block dropped** (the serialized F1 test stays green
  while this race is latent — that's the trap).
- **MF2 — pin the daemon threading model** so the sync/`block_on` bet can't panic. If the daemon is
  `#[tokio::main]`, every adapter `block_on` panics ("runtime within runtime"). Require: sync `fn main()`,
  infra (signals + `/health`) on an ISOLATED runtime, pipeline on plain std/rayon (no ambient runtime).
  `materialize` fetches via `par_iter` → concurrent `block_on` from N rayon threads → the adapter runtime
  MUST be **multi-thread** (a current-thread rt would contend/panic). Add a debug-assert
  `Handle::try_current().is_err()` tripwire in the adapter methods.
- **MF3 — enforce `known ⊆ mark-set` from a single source of truth** (hidden Phase 4↔3 coupling). The
  daemon's dedup `known` must be seeded from the SAME live-HEAD manifest(s) GC marks, else a dedup-reused
  block from an unmarked manifest gets collected at correct grace (invariant #2). Mechanical: `known` =
  the parent live-HEAD's chunk index; parent HEAD is within retention ⇒ GC marks it.
- **MF4 — pin publish commit ordering + the touch set.** Load-bearing: **touch `block_ref` for every
  referenced block (young) → upload new blocks → put_manifest → advance**. Touch fires even when the
  PutObject is skipped/idempotent, and BEFORE upload (earlier touch = smaller race window). Touch set =
  every block the manifest references (simplest safe superset).

**Rulings on the 4 open questions:** OQ1 → extract MARK only; keep two `collect` fns; local collector
takes the **concrete `&LocalBlobStore`** (so mtime-vs-S3 can't be mixed — won't compile); PG sweep behind
a `RefClock` trait with the PG impl **+ an in-memory fake** (unit-testable without testcontainers). OQ2 →
build the minimal raw-TCP `/health` + readiness latch NOW (cheapest test of the daemon threading model,
MF2). OQ3 → **RDS `db.t4g.micro`** (faithful to what zeroshot-cloud runs; provision ONCE for both the
gated `PG_IT=1` tests and Phase 5; `--skip-final-snapshot` teardown). OQ4 → **drop the HeadObject
dedup-skip; unconditional idempotent PutObject** (content-addressed ⇒ overwrite is byte-identical; the
common path is new blocks so Head just adds latency); keep `has_block` as a separate method; F1 protection
comes from the `block_ref` touch, NOT the upload — decoupled per MF4. This makes unconditional re-upload
also the thing that RESTORES a block GC deleted mid-race.

**Should-fixes captured into the plan:** `advance()` dispatch (INSERT-on-conflict for first write vs
guarded UPDATE) + treat `StaleFence`-where-HEAD-already-equals-`(my digest, expected+1)` as success
(lost-ack on the final SIGTERM publish is durable, not an R1 alarm); trait-change blast radius (2
`FlakyStore` doubles in `audit_independent.rs`/`adversarial3.rs` + `let mut ls`→`let ls` in
`experiments.rs` — must update so "50 green" holds); bound adapter runtime worker threads (2–4 vCPU pod);
**record materialize fetch concurrency** in Phase 5 (par_iter caps at num_cpus; may miss R2 budget for
multi-GB trees — batch `join_all` fix if so); no `.tmp`/partial-write concept on S3 (PutObject atomic);
**verify `sqlx 0.9` resolves** standalone (research read `=0.9.0` from zeroshot-cloud; reviewer flags
crates.io latest-known 0.8.x — reconcile at Phase 2 start; 0.8.x semantics identical for our use); land
the ifaces honesty fix as its own tiny commit (zero functional value standalone — don't let it gate).

Verdict interpretation: reviewer said "fix these four → APPROVED-WITH-CHANGES." Revising plan in place
(Revision 1) and proceeding to Phase 1 on that basis; the fixes are precisely specified, no second full
plan-review round needed.

### 2026-07-21 — Phase 1: S3BlobStore (implemented + validated on real MinIO)
- **Trait change**: added idempotent `delete_block`/`delete_manifest` (returns `bool` "did-remove";
  NotFound⇒false) to `BlobStore`; `LocalBlobStore` impl via `rm_idempotent`. Updated the 2 `FlakyStore`
  doubles (delegate to inner). **50→51 default tests green** (added `local_blobstore_conformance`).
- **Uniform NotFound**: moved `StoreError::NotFound` into `cas.rs` (was going to live in `s3.rs`); BOTH
  `LocalBlobStore` (`read_or_notfound`) and `S3BlobStore` now return the same typed NotFound, so GC /
  materialize get one signal regardless of backend. Nicer than the plan's S3-only NotFound.
- **`src/s3.rs` `S3BlobStore`** (feature `s3`): bounded **multi-thread** tokio runtime (4 workers) +
  `block_on` inside sync methods + `Handle::try_current().is_err()` debug-assert tripwire (MF2). Keys
  `blocks/<id>`/`manifests/<digest>`. `put_*` = **unconditional idempotent PutObject** (OQ4, no
  HeadObject-skip). `get_*` map `GetObjectError::NoSuchKey` → `StoreError::NotFound`. `has_block` =
  HeadObject. `delete_*` = Head-then-Delete for a faithful did-remove bool (off the hot path; GC deletes
  only after winning the PG claim). Errors redacted to the modeled service code (no creds/body leak),
  mirroring zeroshot-cloud `kms.rs`. Env contract: `S3_BUCKET` + optional `S3_ENDPOINT_URL` (⇒
  `force_path_style` for MinIO); region/creds via default chain.
- **Dep pins**: `aws-sdk-s3 v1.138.1` + `aws-config v1.9.0` (matches zeroshot-cloud's `=1.9.0`), both
  `default-features=false` + `["behavior-version-latest","default-https-client","rt-tokio"]`. Compiled
  clean first try. Default (no-feature) build stays lean.
- **Validation**: `tests/blobstore_conformance.rs` runs the SAME contract on `LocalBlobStore` (always) and
  `S3BlobStore`. Ran it against **real MinIO** (Docker `minio/minio`, bucket via `minio/mc`): **both
  pass** — 300 KB block + manifest round-trip, typed NotFound (absent + after-delete), idempotent delete
  (true then false), unconditional-overwrite-identical. Skip path (no S3 env) prints a visible degraded-
  coverage notice and passes (repo rule). clippy clean on `--features s3 --all-targets`; rustfmt clean.
- **Not yet**: real-AWS S3 (`S3_IT=1`, Phase 5); multipart (deferred); versioned-bucket
  `DeleteObjectVersion` (test buckets are versioning-off; noted as prod follow-up).

**Phase 1 senior-review: APPROVED-WITH-NITS** (no must-fix). Verified correct: MF2 runtime containment
(owned multi-thread runtime + per-call `block_on` is panic-free from rayon threads, no deadlock,
multi-thread genuinely required for concurrent `par_iter` gets), the `on()` tripwire, GetObject
`NoSuchKey`→NotFound, the HEAD-vs-GET 404 asymmetry, the `StoreError` refactor (50 green holds; `gc.rs`
raw-fs NotFound matches untouched; F1 `touch_mtime` intact), OQ4 unconditional PUT. Should-fixes applied:
- **S1** (silent orphan): `del()` HEAD now distinguishes a modeled 404 (`is_not_found()` ⇒ absent) from a
  TRANSIENT error (propagates), so GC re-drives instead of skipping DeleteObject and leaking bytes.
  `has_block` keeps transient⇒false (safe direction). Re-validated on MinIO.
- **S2** (observability): `redacted()` now includes the SDK error CATEGORY (service/timeout/dispatch/
  response/construction) + code, so a real-AWS incident isn't an undiagnosable `[unknown]`.
- **N1**: `tokio` `s3` feature now declares `net`,`time` explicitly (not relying on the SDK's transitive
  features for `enable_all()`).
- **N2**: fixed the misleading "one part" test comment.

**PHASE-5 WATCH ITEMS (real-AWS behaviors MinIO cannot exercise) — set up before/at Phase 5:**
1. **s3:ListBucket is load-bearing** — without it real S3 returns **403 (not 404 NoSuchKey)** for a missing
   key, which our code would classify as transient, breaking the NotFound-dependent GC/republish logic.
   The Phase-5 bucket policy/role MUST grant `s3:ListBucket`. (Code comment added at `s3.rs` get().)
2. **Region**: `from_env` sets no region → must export `AWS_REGION` for real AWS (MinIO+path-style masks
   this; wrong region → 301 PermanentRedirect).
3. **SSE-KMS**: if the bucket enforces `aws:kms`, PUT needs no header but the role needs
   `kms:GenerateDataKey`/`Decrypt`; GET needs `kms:Decrypt`. Provision KMS grants with the bucket (or use a
   plain SSE-S3 test bucket).
4. **Throttling/503, large-object (64 MiB) PUT/GET wall-time + single-PUT retry replaying 64 MiB, versioned-
   bucket reclamation** — none exercised at 300 KB on MinIO. Confirm the Phase-5 bucket is versioning-OFF.
- Next: push Phase 1 → Phase 2 (PgLineageStore + block_ref, testcontainers Postgres).

### 2026-07-21 — Phase 2: PgLineageStore + block_ref (implemented via worker agent, reviewed, fixed)
Implemented by a worker subagent (team-programming worker role) to a precise spec; I read every line,
reproduced the tests on real Postgres, then ran a senior-review gate. Deviation from the plan: used a
**manually-run `postgres:17.10-alpine` container gated on `DATABASE_URL`** (same pattern as the S3 MinIO
test) instead of the `testcontainers` crate — simpler, avoids a heavy dev-dep, consistent with Phase 1.
- **`LineageStore` trait**: `advance(&mut self)`→`advance(&self)` (pooled store shared); `FileLineageStore`
  wraps its map in a `Mutex` (behavior identical); added typed `LineageError::StaleFence{expected,current}`
  (matchable). Fixed the one caller (`let mut ls`→`let ls`). Default suite stays **51 green**.
- **`src/refclock.rs`**: feature-independent `RefClock` trait (`touch`/`candidates_older_than`/
  `claim_collectable`) so a Phase-3 in-memory fake needs no `pg`.
- **`src/pg.rs`**: `PgLineageStore` + `PgRefClock` over a shared `Arc<PgRuntime>` (one bounded 4-worker
  multi-thread runtime + pool built ON it + the `on()` tripwire, mirroring `S3BlobStore`). Fence CAS
  dispatched on `expected` (INSERT-on-conflict for 0, guarded UPDATE for ≥1) + MF4 idempotent-retry
  (absorb a would-be StaleFence iff HEAD already == `(digest, expected+1)`). Guarded `signed`/
  `positive_u64` (no raw `as`), `checked_add` overflow. `PgRefClock.touch` = unnest upsert
  `DO UPDATE SET last_referenced_at=clock_timestamp()`; `claim_collectable` = the MF1 atomic
  `DELETE ... WHERE last_referenced_at < clock_timestamp()-grace` (age re-checked SERVER-SIDE).
- **`migrations/0001_bootstrap.sql`**: plan §4 DDL, idempotent.
- **sqlx 0.9.0** pinned (`cargo add` resolved crates.io-latest = 0.9.0 — matches zeroshot-cloud's `=0.9.0`;
  the plan's 0.8.x hedge was stale). `["postgres","runtime-tokio"]`, no TLS backend (local = sslmode=disable).
  Default dep tree has **0 sqlx** (lean preserved).
- **Tests (`tests/pg_lineage.rs`, gated on `DATABASE_URL`)**: 7 on real PG — fence happy path; concurrent
  UPDATE-path race (one wins/one StaleFence, different digests so no false idempotence); concurrent
  INSERT-path first-writers (S2); idempotent-retry both paths + a genuine-StaleFence sanity; reuse-clock
  touch-bumps; MF1 server-side re-check (a touch-between defeats a 1h-grace claim); M1 duplicate-digest
  touch. Reproduced all green myself.

**Phase 2 senior-review: CHANGES-REQUIRED → fixed.** The reviewer PROVED the fence CAS and the atomic
claim correct under every interleaving (incl. ABA and same/different-digest first-writer races), and found:
- **M1 (must-fix, real bug the 5 original tests missed)**: `touch` fed Postgres an `ON CONFLICT DO UPDATE`
  with duplicate keys → `ERROR: cannot affect row a second time`. In prod a manifest references each block
  hundreds of times (one per packed chunk), so **every real publish's touch would fail**. Fixed: `touch`
  dedups by digest (BTreeMap) before binding (also shrinks payload ~256×). Regression test added.
- **S1 (fixed)**: dropped the `block_ref_seen_ordered CHECK (last>=first)` — it coupled publish liveness to
  wall-clock monotonicity (an RDS failover to a standby a few ms behind would fail touches) and guarded a
  column nothing reads.
- **N2 (fixed)**: dropped the dead `RETURNING` in `claim_collectable` (rows_affected drives the win).
- **N3 (fixed)**: added `PgLineageStore::head() -> Result<Option<Head>>` (fallible) — the Phase-4 daemon's
  materialize-on-start MUST use it (not the infallible trait `get`, which degrades a transient DB error to
  None and could start a pod with an empty workspace on a blip).
- Verified fine: runtime/pool binding (pool built on the held runtime; no deadlock with max_conns=8 + 4
  workers since no conn is held across a second acquire); eager `connect` fail-fast; type mapping.

**PHASE-5 WATCH (real RDS, can't surface on local PG):** (1) **TLS is the likely tripwire** — the crate has
NO sqlx tls feature; real RDS with `rds.force_ssl=1` / `sslmode=require` will fail to connect. Phase 5 must
add `tls-rustls` + the RDS CA + `sslmode=require`. (2) IAM auth not implemented (password via DATABASE_URL
only — fine for the master user). (3) Connection budget: prefer `ref_clock()` (shared pool) over two
independent pools at pod scale; mind `db.t4g.micro` max_connections. (4) `clock_timestamp()` monotonicity
across failover (relates to S1).

**PHASE-3 DESIGN FLAG (settle BEFORE coding Phase 3):** there is **no manifest liveness clock** and
`lineage_head`'s UPDATE **overwrites** `manifest_digest` (no superseded-digest history). So Phase 3's
"manifest GC (superseded HEADs)" has neither a superseded-manifest list nor a cross-host grace clock for
manifests — needs a design decision (a `manifest_ref` clock, or accept that only blocks are GC'd for now).
Also: the claim→`delete_block` straddle means Phase 3's sweep MUST keep dedup-reusable (live-HEAD-marked)
blocks OUT of the candidate set (invariant #2), since a block re-referenced after a won claim but before
DeleteObject is a live-byte loss the claim primitive alone can't prevent.
- Next: push Phase 2 → Phase 3 (PG-driven GC).

### 2026-07-21 — Phase 3: Postgres-driven GC (implemented inline — the crux)
Implemented myself (load-bearing MF1 correctness). Generic over the `BlobStore` + `RefClock` traits so
it runs with the in-memory fake (fast) AND real S3+PG (Phase 5).
- **`gc::mark_live_blocks(&dyn BlobStore, &[String]) -> HashSet<BlockId>`** (shared MARK, invariant #2):
  reads live manifests via the trait; a genuinely-absent live manifest is skipped (via the uniform
  `StoreError::NotFound` downcast), not fatal. The file-backed `gc::collect` keeps its own path-based
  mark (OQ1: no forced refactor of the 7 passing fs-GC tests).
- **`refclock::MemRefClock`**: in-memory `RefClock` fake (Mutex<HashMap<block, Instant>>) that models the
  server-side atomicity — `claim_collectable` re-checks age + removes under one lock, exactly as the PG
  `DELETE ... WHERE last_referenced_at < clock_timestamp()-grace` does. Lets GC + its concurrency tests
  run without Docker.
- **`gc_pg::collect(store, clock, live, grace)`**: MARK (skip live-referenced) → `candidates_older_than`
  (grace pre-filter) → for each unmarked candidate, **atomic `claim_collectable` (age re-checked
  server-side) → only on a won claim `delete_block`** (row-claim-FIRST, object-delete-second, MF1). A
  `delete_block` failure after a won claim → counted as `orphaned` (row gone, object remains → cost
  leak, reconciler reclaims), does NOT abort the sweep. `GcPgStats{scanned,deleted,kept_marked,
  raced_young,orphaned}`.
- **Tests (`tests/gc_pg.rs`, 4, default-feature, no Docker)**: reclaims aged orphans + keeps live-HEAD
  blocks + HEAD materializes; a marked block is never collected even at grace=0 (invariant #2); the
  MemRefClock atomic re-check (a re-touch defeats a 1h-grace claim, mirroring the PgRefClock test);
  **concurrent publisher + GC over 20 gens with a per-sweep liveness assert + final materialize** — no
  live-byte loss, orphans reclaimed. **55 default tests green** (51+4); stable across 3 runs.

**Insight the failing concurrent test surfaced (and how it was resolved).** My first cut swept at
**grace=0**, and it FAILED — correctly. At grace=0 there is NO youth window: a block a publisher just
wrote+touched but that a *stale* GC read hasn't marked yet (GC still on the previous HEAD) is
immediately "old" and unmarked → GC collects it → the about-to-be-live HEAD loses a block. That is not
a GC defect — it's the **operational invariant `grace > max(publish, sweep) duration` being violated**.
Fixed the test to use a realistic grace (50 ms) with inter-gen spacing (20 ms): a freshly-touched block
stays young (clock-protected) until the next sweep marks it, while superseded gens age past grace and
ARE reclaimed. This is a concrete demonstration of WHY grace=0 is an invalid production config — worth
carrying into the deployment docs.

- **Deferred (documented)**: manifest GC (manifests are tiny; needs a manifest liveness clock /
  superseded-digest history the schema doesn't keep — see Phase-2 design flag). Single-flight advisory
  lock (correctness is in the per-block atomic claim; the lock only avoids two sweepers wasting work —
  a deployment concern, not a correctness one). Both noted in `gc_pg.rs`.
- Next: Phase 3 senior-review gate → push → Phase 4 (daemon).

**Phase 3 senior-review: APPROVED-WITH-NITS** (no must-fix). The reviewer independently REPRODUCED the
grace=0 live-loss (5/5) and grace=50ms safety (5/5), and verified each attack is stopped by the exact
named invariant: in-flight block → grace clock + server-side re-check; dedup-reused block → MARK;
HEAD-commits-mid-sweep → grace clock; two concurrent sweepers → the per-block atomic claim (genuinely
safe, just wasteful — single-flight lock correctly deferred). Should-fixes applied:
- **S1 (honesty, R1-critical file)**: the `gc_pg.rs` header under-stated the claim→delete straddle as
  "only a cost leak." Corrected: it's a genuine NARROW live-byte-loss residual — if the GC thread STALLS
  between the won claim and `delete_block`, and a publisher resurrects that same (previously-orphan)
  block (`touch`+`put_block`) in the window, GC's delete removes the re-uploaded object. Closed fully
  only by a per-publisher lease (plan-deferred). Does NOT apply to dedup-reuse (marked → never a
  candidate). Documented both residuals precisely.
- **S2**: `mark_live_blocks` NotFound-skip now matches `StoreError::NotFound` specifically (was: any
  `StoreError`). A future non-NotFound variant treated as "skip" would drop a needed block from the mark
  set → wrongful collection. Now propagates.
- **S3**: `mark_live_blocks` returns the missing-manifest count; `GcPgStats.missing_live_manifests`
  surfaces it (a live HEAD whose manifest can't be read is an anomaly the caller must alert on).
- Nits: `grace_secs` rounds a non-zero sub-second grace UP to 1s (safe direction; grace=0 stays 0);
  documented the fake's `>=` vs PG's `>` boundary. 55 default green, clippy/fmt clean.

**PHASE-4 CONSTRAINT (must enforce mechanically, not by docs — reviewer W4):** every Phase-3 test used
`known = empty` + `live = current HEAD only`. The entire dedup-reuse safety (MF3, invariant #2) rests on
the Phase-4 daemon **seeding dedup `known` from the parent live-HEAD chunk index** AND the GC caller
passing all retained-history HEADs in `live`. Wire this as a single source of truth in Phase 4 and TEST
it (a dedup-reused block from the live HEAD survives a concurrent GC).

**PHASE-5 WATCH (real S3+PG, unshowable on the fake/local tests):** (1) grace must be ≥ publish + sweep +
**max clock step/skew** (NTP forward-step / RDS-failover lagging clock) — the `Instant` fake can't model
wall-clock anomalies; (2) **sweep duration on real S3** — `mark_live_blocks` is N manifest GETs (a
manifest is ~89 MB @ 300k files, not "tiny"), and the whole sweep must stay < grace; MEASURE it; (3) the
claim→delete straddle is exercised by NO local test (needs a precisely-injected stall) — watch the e2e;
(4) manifest growth — deferring manifest GC is correctness-fine but orphaned manifests accumulate real S3
cost over long runs; track it.
- Next: push Phase 3 → Phase 4 (daemon), enforcing the MF3 known⊆mark seeding.

### 2026-07-21 — Phase 4: the daemon (implemented via worker agent, verified)
The production loop: blobs in `--store` (`file://` LocalBlobStore | `s3://` S3BlobStore), lineage + GC
clock in `--db` (Postgres, required). I read the load-bearing core (`daemon_loop.rs`) and reproduced the
full suite.
- **`src/daemon_loop.rs`** (library, so `tests/` can drive it): `publish_cycle` + `materialize_on_start`
  (feature `pg`) + a std-only `/health` server. **`publish_cycle` is the MF3+MF4 core**: (1) `ls.head()`
  FALLIBLE read (a DB blip is Err, not "fresh lineage"); (2) **MF3** — seed dedup `known` from the parent
  live-HEAD's chunk index (so a reused block is always in a MARKED manifest); (3) `publish()` (upload +
  put_manifest); (4) **MF4** — `clock.touch()` every referenced block young BEFORE `advance` (touch-after-
  upload is safe: an untouched fresh block has no `block_ref` row → invisible to GC); (5) `advance` fence
  CAS → `Fenced` (non-fatal, a different writer) surfaced as a value, non-fence error is fatal.
- **`src/main.rs`**: `Daemon` subcommand (variant always present; handler `#[cfg(feature="pg")]`, else a
  clear "rebuild with --features pg,s3"). **MF2 threading**: plain sync `fn main`; signals (`signal-hook`,
  new dep), the health `TcpListener` thread, and the pipeline all on std threads — NO ambient tokio
  runtime, so the S3/PG adapters' internal `block_on`s never nest-panic. Lifecycle: connect PG + init
  schema → build store → materialize-on-start → ready → interruptible interval loop → SIGTERM: one final
  publish → exit 0. `--once` for tests/one-shot.
- **`src/ifaces.rs`** honesty fix: relabeled LITERAL MIRRORS (`Fence`, `RecoveryStatusCompletedParts`) vs
  NEUTRAL GENERALIZATIONS (`ClaimRole`, `ProviderNeutralClaim(s)`, `ArtifactRef`) vs OUR-OWN; restored the
  3 dropped `RecoveryStatusCompletedParts` fields (`run_id`, `reservation_id`, `fence`).
- **Tests (`tests/daemon.rs`, 5, gated on `DATABASE_URL`)**: lifecycle (fence 1→2, a fresh daemon
  materializes-on-start byte-for-byte); **MF3 dedup-reuse survives GC** (a reused block, seeded from HEAD,
  survives a grace=0 sweep); stale-fence surfaces non-fatal; health readiness 503→200 + response format.
  Plus the worker's real-binary smoke tests (`--once` ×2, `/health` over TCP 200, SIGTERM→final publish→
  exit 0, default-build rebuild message). Reproduced: **55 default green; full pg suite 67 green**;
  clippy (`pg,s3`) + fmt clean. `signal-hook 0.4.4` (default-features off), `sqlx 0.9.0`.
- **Noted for review**: MF4 `touch` uses `loc.clen` (a chunk's compressed length) as the block's
  `byte_length` — after per-block dedup that's SOME member chunk's clen, not the block's total size.
  `byte_length` is metrics-only (GC never reads it), so this is an accounting inaccuracy, not a
  correctness bug — flagged for the reviewer.
- Next: Phase 4 senior-review gate → push → Phase 5 (real AWS e2e — needs the user's `aws sso login`).

**Phase 4 senior-review: APPROVED-WITH-NITS** (no must-fix). The reviewer verified STRUCTURALLY that
neither crasher exists: **MF2** — no nested-runtime path (sync `fn main`; signals/health/pipeline on std
threads; adapters' `block_on` only ever from non-tokio threads; concurrent `block_on` from `par_iter`
rayon threads is the supported multi-thread-runtime case); **MF3/MF4** — no live-block loss (touch-after-
upload is the *correct* realization: block ids aren't known until packed, and an untouched fresh block
has no `block_ref` row → invisible to GC; continuous cover no-row→young→marked; MF3 `known` is a
fail-safe single-source-of-truth from the one `head()` read). Should-fixes applied:
- **S1**: health server now sets `set_read_timeout`/`set_write_timeout` (2s) — a half-open/slow client
  could otherwise wedge the single-threaded accept loop → block all probes → kubelet kills the pod.
- **S2**: the daemon now **backs off exponentially on a genuine fence streak** (capped 600s; reset on a
  successful advance). A fenced daemon does the expensive publish (upload delta + manifest) before the
  cheap CAS, so re-contending every interval leaked S3 cost + orphan manifests and could oscillate HEAD;
  backoff throttles it. (Prod fences-by-kill via the orchestrator; standalone has no such guard.)
- **S3**: MF4 `touch` now sends the block's REAL `byte_length` (sum of its member chunks' compressed
  lengths), not one arbitrary chunk's `clen` (~256× under-report). Metrics-only (GC never reads it) but
  fixed while fresh.
- **S4 (test gap)**: added `s3_concurrent_block_on` (blobstore_conformance) — N threads share one
  `S3BlobStore` doing concurrent `get_block` → concurrent `block_on` on the adapter's runtime, the exact
  `materialize` `par_iter` path and the whole reason the runtime must be multi-thread. **Validated on
  real MinIO** (no nested-runtime panic, correct bytes). Nits: reworded the "pre-touch" comment
  (it's after upload); documented the SIGTERM-at-cycle-boundary + first-publish-at-interval behavior.
- Verified fine: idempotent-retry-vs-StaleFence, SIGTERM drain (double-publish is harmless/idempotent),
  cross-test GC isolation (disjoint digests + per-test stores), ifaces labels. 55 default green;
  s3 conformance 3/3 on MinIO; daemon 5/5 on real PG; clippy/fmt clean.

**PHASE-5 WATCH added by this review:** (1) TLS — no sqlx tls feature; real RDS `force_ssl` fails to
connect (add `tls-rustls` + RDS CA). (2) materialize-into-non-empty-tree — a pod reusing an NVMe dir with
stale files → `materialize` hard-bails on start (fail-safe but brittle; decide: guaranteed-fresh volume
or reconcile). (3) transient-error crash loop — a non-fence cycle error kills the daemon (fail-fast); real
S3 503/throttle is common (SDK retries most; confirm or add bounded retry). (4) drain vs 30s/61s budgets —
an in-flight publish isn't interrupted by SIGTERM; cold-materialize wall-time vs 61s still unmeasured
(Phase-5 measurement). (5) manifest growth — deferred manifest GC + fenced churn leak orphan manifests.

### Phases 1–4 COMPLETE. Standalone build done; only the real-AWS e2e (Phase 5) remains — it needs the
user's `aws sso login` (internal account 993939946442). Test totals: 55 default + 3 S3 (MinIO) + 12 PG
(7 lineage + 5 daemon) + the gc_pg/e8 already in default. All green.

### 2026-07-21 — Phase 5: real-AWS end-to-end (internal account 993939946442) — PASSED, torn down
User chose RDS `db.t4g.micro`. Provisioned a throwaway S3 bucket + RDS Postgres 17.10 (public, SG scoped
to my IP:5432) in us-east-1, added the **sqlx `tls-rustls-ring`** feature (RDS requires TLS), ran the
real-AWS validation, then **tore everything down**.
- **Real S3** (`S3_IT=1`, no `S3_ENDPOINT_URL`): `blobstore_conformance` (round-trip, typed NotFound,
  idempotent delete) + `s3_concurrent_block_on` — **pass**. The NotFound test passing on real S3 confirms
  the 403-vs-404 concern is a non-issue with these creds (missing key → 404 NoSuchKey).
- **Real RDS over TLS** (`sslmode=require`): `pg_lineage` (7) + `daemon` (5) — **all pass, 12/12**.
  **`sslmode=disable` FAILS** → this RDS enforces `rds.force_ssl`, which CONFIRMS the reviewer's Phase-2
  watch item: without the sqlx TLS feature we could not connect; with `tls-rustls-ring` we can. Load-bearing
  validation, not just a nice-to-have.
- **Full-stack e2e** (`tests/e2e_aws.rs`, real S3 blobs + RDS lineage/clock together, over TLS) —
  **both pass**:
  - `e2e_publish_materialize_gc_over_s3_and_rds`: publish→**cold-materialize from real S3 in 1.06s**
    (1 block; laptop→S3 = upper bound, well under the 61s activation budget → **R2 satisfied**)→ a fully-
    superseded gen → **GC deleted 3 real S3 orphan objects** (real `DeleteObject`) via the RDS clock+mark,
    kept the marked live block, gen2 re-materialized byte-identically.
  - `e2e_fence_cas_over_rds`: two concurrent first-writers over real RDS → exactly one wins, one typed
    StaleFence.
  - (A first run surfaced a *test-scenario* bug — a partial-overwrite gen2 dedup-reused gen1's block so it
    stayed correctly live, no orphan; fixed to fully-distinct gen2. The CODE was right; the test proved
    block-level dedup keeps a shared block live under GC, which is itself reassuring.)
- **Teardown**: RDS deleted (`--skip-final-snapshot`), S3 bucket emptied + deleted, security group
  removed. No resources left running. No credentials committed (tests read `DATABASE_URL`/AWS creds from
  env only). Est. cost < $0.10 (RDS ~1 hr + a few S3 ops).

**PHASE 5 COMPLETE.** The five backend COMPONENTS — S3BlobStore + PgLineageStore + block_ref reuse-clock
+ gc_pg + the daemon — are built and **validated against the real services they target** (S3 + RDS
Postgres over TLS), behind the prototype's sync traits, standalone (not integrated into zeroshot-cloud).
Every load-bearing PER-COMPONENT invariant (R1/R2, MF1–MF4) is proven by tests on real infra + reproduced
by independent reviewers, and the seams compose for a single publish/materialize/GC path.

### 2026-07-21 — Final holistic (cross-phase) review: SHIP-WITH-FOLLOWUPS
An independent final reviewer traced the WHOLE wired system (the view no per-phase reviewer had) and
verified the seams compose (uniform fence-0↔1, uniform `StoreError::NotFound` plumbing P1→P3/P4, MF1
atomic-claim consistency, MF2 no-nested-runtime, MF4 touch-after-upload). **Honest correction to the
record: the *components* are done + real-AWS-proven; the daemon as a DEPLOYABLE WHOLE is not** — the
system-level reclamation actor was designed, built, and tested, then intentionally left unwired (the user
deferred platform integration). The critical cross-phase follow-ups (blockers before the daemon runs in a
pod — an INTEGRATION-phase job, out of scope here, but surfaced loudly):
- **F1 (manifest churn):** `parent` is part of `logical_digest` identity AND changes every cycle, so the
  daemon mints a UNIQUE manifest every interval even on a byte-identical tree → ~2,880 orphaned
  manifests/day idle, and manifest GC isn't implemented. `parent` is currently write-only (nothing reads
  it). Fix at integration: content-only change-detection to skip no-op cycles and/or drop `parent` from
  `logical_digest` (free manifest dedup), + implement manifest GC.
- **F2 (data-loss footgun — now documented in code):** `gc_pg::collect`'s `live` MUST be STORE-WIDE (all
  lineages' HEADs), not per-lineage, or it deletes other lineages' live blocks. A loud ⚠️ warning added
  to the `collect` doc. GC is a store-wide singleton actor, not a daemon responsibility.
- **F3:** MF3's `known ⊆ mark-set` is mechanical on the producer (daemon seeds `known` from parent HEAD)
  but has no wired consumer (GC unwired) — the closing half is test-scaffolding-only until GC is wired.
- **F4:** two hardcoded 4-worker runtimes (S3 + PG) on a 2-vCPU pod = oversubscription; size to
  `available_parallelism` or share one infra runtime.
- **F5 (fs vs pg GC):** `gc::collect` keeps its own inline mark (doesn't call `mark_live_blocks`) — two
  mark loops kept identical by hand (documented deviation; OQ1 "unmixable" is honored — `gc::collect`
  takes `&Path`, physically can't accept an S3 store).
- **F6:** materialize masks setuid/setgid/sticky (in `logical_digest`), so a tree with special-mode files
  isn't a publish fixed-point across restart (dominated by F1 today).
Plan deviations noted as CORRECT: `--fence` flag dropped (fence authority = `lineage_head` CAS, better);
ifaces "drop-in" labels now honest. §2 non-goals all still honored (no scope creep); all deferrals
documented.

### 2026-07-21 — Cross-node + in-region experiments (64-vCPU quota approved) — PASSED, torn down
Quota increased to 64 vCPU on the internal account → ran the two quota-gated experiments on REAL
multi-node EC2, using the COMPLETED production daemon binary (Linux arm64, built locally in Docker,
distributed via S3). Provisioned: S3 bucket + RDS `db.t4g.micro` PG17 + 2× **c7gd.2xlarge** Graviton
nodes (8 vCPU, instance-store NVMe) in **different AZs** (node A us-east-1a, node B us-east-1c), IAM
instance profile (SSM + S3), driven via SSM. Then **torn down** (both instances terminated, RDS deleted,
bucket + IAM + SGs removed).

- **2-node cross-node transfer (R1 + R4) — the core thesis, PROVEN.** Node A (us-east-1a) generated a
  **2.0 GB / 840-file** workspace on its NVMe and published it to S3+RDS (**publish 37.8s**: walk + chunk
  + sha256 + zstd + upload 2 GB incompressible + fence advance → HEAD fence 1). Node B (us-east-1c —
  **different AZ, a FRESH instance that never saw node A's disk**) ran the daemon: materialize-on-start
  reconstructed the ENTIRE workspace from the content store (S3 blocks + RDS lineage HEAD) onto its own
  empty NVMe. **Tree checksum identical on both nodes** (`f256ae0f…`) → byte-for-byte transfer with **no
  shared filesystem**. This is R1 (survive node loss → resume elsewhere) + R4 (node/AZ flexibility)
  end-to-end on real infra, using the exact S3BlobStore + PgLineageStore + daemon that were built.
- **In-region cold-materialize at scale (R2) — the open number, CLOSED.** Node B's cold-materialize of
  the 2 GB / 840-file tree from S3 to fresh NVMe, timed via the daemon's OWN health-readiness signal
  (503→200 when materialize-on-start completes — validating that endpoint on real infra): **6.60s**.
  Well under the 61s activation budget; ~6× faster than the publish direction (materialize = parallel
  block GETs + decompress + write; publish = hash + compress + upload). For a typical 1–2 GB workspace
  resume is ~3–7s cold (faster warm / with reflink). The Phase-5 laptop→S3 number (1.06s @ 5 MB) was an
  upper bound; this is the representative in-region figure at realistic scale.
- **Full daemon lifecycle on real infra**: materialize-on-start → health-ready (503→200) → publish loop
  → SIGTERM → final publish (drain) → exit 0, all exercised across both nodes. The single-writer handoff
  (node A fence 1 → node B resumes + advances) worked over real RDS/TLS.
- **Note (capacity):** c7gd.2xlarge hit `InsufficientInstanceCapacity` in us-east-1b/1d on first try —
  retried into 1a/1c (still cross-AZ). Real spot-of-the-moment AWS capacity, not a design issue.
- Est. cost: 2× c7gd.2xlarge (~$0.36/hr each) + RDS, < 1 hr → ~$1. Nothing left running.

### 2026-07-21 — Optimization pass (threads 1+2+4, still standalone; user: "optimize as much as possible before integrating")
Code done locally (O1/O2/O4), measured on real EC2 (O5), all torn down.
- **O1 — kill manifest churn (F1).** `logical_digest` now EXCLUDES `parent` → content-only manifest
  identity: a byte-identical tree always yields the same digest, so an idle re-publish is idempotent
  (`put_manifest` dedups, no orphan leak) and identical trees dedup. Daemon `publish_cycle` gained
  `CycleOutcome::NoChange` — when the content digest equals the live HEAD, skip touch+advance entirely
  (no fence bump, no churn; blocks stay MARK-protected via the unchanged HEAD). An idle daemon now does
  ZERO writes. Tests: `manifest_identity_content_only_ignores_parent`, `nochange_cycle_when_tree_unchanged`.
- **O2 — warm-cache tier (`CachedBlobStore`).** Node-local NVMe cache in front of the durable S3 backing;
  reads prefer the cache (local, no network), miss reads-through + populates; writes go to the durable
  authority FIRST then best-effort to cache (cache NEVER authoritative; content-addressing makes staleness
  harmless; deletes hit both tiers). Daemon `--cache-dir` wires it in. Tests prove a warm materialize does
  0 backing GETs.
- **O3 — daemon uses the bounded-parallel publish pipeline** (measure-first: see O5). `publish_cycle`
  switched `publish` → `publish_pipelined(workers = available_parallelism.clamp(1,16))` — same logical
  manifest, ~2× less publish CPU.
- **O4 — the store-wide GC actor (F2).** `gc` subcommand + `PgLineageStore::all_head_digests()` (marks
  against EVERY lineage's HEAD — store-wide, the F2 requirement). Test `gc_store_wide_live_set_protects_
  all_lineages` DEMONSTRATES the per-lineage footgun (deletes another lineage's blocks → materialize fails).

**O5 — real-EC2 measurements** (c6gd.8xlarge, 32 vCPU + NVMe, us-east-1a; RDS PG17/TLS; S3; via SSM; torn
down):
| Metric | Result |
|---|---|
| **Warm vs cold resume (2 GB, R2 — the O2 payoff)** | **cold 3.74s → warm 1.48s (2.5× faster); warm restore byte-identical.** 1.48s is UNDER the <5s R2 target; the cache eliminates the S3 fetch. |
| Publish 2 GB → S3 | 43.8s (upload-dominated: ~23s network + ~20s CPU) |
| **Publish pipeline scaling (2 GB local, incompressible)** | workers 0→**20.4s**, 4→15.7s, 8→13.5s, 16→**10.4s**, 32→10.4s. **~2× at 16 workers**, plateaus there → drove O3. |
| 5 GB scale (local) | publish 25.2s (16w), **materialize 3.22s** — linear, fast resume path |

**Findings:** (1) the warm cache hits the aggressive <5s resume target (1.48s/2 GB warm) and even cold in-
region is under it on a fast node (3.74s). (2) Publish CPU halves with the pipeline (now wired). (3) S3
publish is UPLOAD-bound (~23s of 43.8s for 2 GB incompressible) — the next publish lever is PARALLEL block
UPLOADS (the packer currently uploads serially); overlapping compress+upload would cut the S3 publish
further. Real workspaces are also more compressible than the random test data, so both the pipeline speedup
and the upload volume improve in practice. (4) 5 GB scales linearly and stays memory-bounded (streaming).

**Still open (optimization follow-ups):** parallel S3 uploads in the packer (the biggest remaining publish
lever); file-level reflink on warm resume (skip decompress+write for unchanged files → near-instant
incremental resume — needs a reflink-capable FS + prior-tree tracking); mtime-based skip so an idle daemon
avoids even re-hashing; the orphan-object reconciler (needs a `BlobStore.list`). LVM-thin same-node COW
fork not yet measured.

### 2026-07-21 — O6: parallel S3 block uploads (implemented, reviewed, fixed)
The biggest remaining publish lever (O5: S3 publish of 2 GB was ~43.8s, ~23s serial upload). The packer
previously called `put_block` serially as it finalized each 64 MiB block. Now `publish_pipelined` has an
UPLOAD stage: `workers` uploader threads (round-robin channels) `put_block` blocks CONCURRENTLY; the
packer finalizes a block, records its chunk locations in the index (independent of the upload landing),
then hands `(block_id, bytes)` to an uploader. Ordering preserved (index built → uploaders drain, all
blocks durable → `put_manifest`). Same logical manifest (pipeline-equivalence tests green).

**Senior-review: CHANGES-REQUIRED → fixed.** The reviewer confirmed the load-bearing properties (no
deadlock — traced every thread-death; no silent upload failure — `put_manifest` runs only after every
uploader joins `Ok`, empirically 0 manifests written on an injected upload failure; same output as
streaming) and found two real must-fixes:
- **Memory regression (real):** the upload channels reused the 256 KiB-chunk bound `cap`(=8) for
  **64 MiB BLOCK** items → up to `workers·cap` blocks buffered ≈ **2.4 GiB @4 workers / 9 GiB @16**. Fixed:
  upload channel bound = **1** (block-tier peak now ~`2·workers·64 MiB` = queued+in-flight, scales with
  workers which the daemon clamps to `available_parallelism`; ~256–512 MiB on a 2–4 vCPU pod). Comment
  corrected.
- **Test red in debug:** `parallel_uploads_overlap` (a latency-injecting store asserting max-concurrent
  `put_block` ≥ 2) passed in `--release` but FAILED 6/6 in debug — debug's sha256-bound block production
  is slower than the 80 ms simulated latency, so blocks never coexist. Fixed: `#[cfg_attr(debug_assertions,
  ignore)]` (release-only, with a reason) — `cargo test` shows it ignored, `cargo test --release` runs it.
- Reviewer's 3rd item (the standalone crate isn't wired into the JS-root CI, so its tests have no CI
  protection) is real but an INTEGRATION concern (the crate has its own `[workspace]` on purpose) —
  deferred/noted, consistent with "don't integrate yet". All testing remains manual/local + real-AWS.
59 default green + 1 ignored (the release-only overlap test); clippy/fmt clean. Real-S3 speedup to be
re-measured in the next EC2 batch.

### 2026-07-21 — O7: reflink incremental warm resume (implemented, reviewed, tested)
The biggest remaining R2 lever. `materialize`'s fetch/decompress/write + link phases were refactored
into shared helpers (`write_regular_files`, `write_links`, `load_verified_manifest`) — behavior-
preserving (all 16 existing materialize/tamper/symlink/hardlink/mode security tests still green). New
`materialize_incremental(store, digest, out, ref_manifest, ref_dir)`: for each regular file whose chunk
list is byte-identical to the reference manifest's same-path file, **reflink** it from `ref_dir`
(reflink-copy: FICLONE on XFS, clonefile on APFS, transparent copy fallback) + apply the new mode; only
CHANGED/new regulars hit the block store. So a warm node resuming to the next generation pays ~the
delta. `MaterializeStats.reference_reused`. Defensive: only reflinks a real regular file in the reference
(lstat, never follows a symlink); absent/non-regular → falls back to the store. `reflink-copy 0.1.30`.

**Senior-review (security-critical fn): APPROVED-WITH-NITS**, no must-fix. The reviewer ran 5 throwaway
adversarial probes and verified: the refactor drops NO security property (manifest integrity, empty-dir
check, size-from-actual-chunks, chunk verification, setuid masking, and the regulars→hardlinks→symlinks-
LAST phase ordering all survive; composition preserves ordering); reflink path-safety (`safe_rel_path`
before every `ref_dir.join`/`out.join`, so a `..`/absolute path in a tampered new manifest can't read
outside `ref_dir` or write outside `out`); symlink-follow refusal at the reference leaf (lstat); and the
KEY soundness link — the reference manifest is ALSO integrity-verified before its chunk lists are trusted
for the unchanged-comparison (stops a tampered stored ref-manifest from falsely marking a changed file
"unchanged"). Applied the top nit (coverage): added `tests/reflink.rs` cases for content-same/mode-changed
(reflink applies the new mode, 0 blocks fetched), traversal-refused-via-incremental, and hardlink+symlink
recreation over a reflinked target — locking in the properties the reviewer verified by hand.

Deferred (noted): (a) `materialize_incremental` has NO production caller yet — the daemon's
materialize-on-start still uses full `materialize`; wiring it needs a **pristine retained reference**
(never the agent's mutated live workspace) with "full materialize on any doubt" as the default posture —
an integration-PR obligation. (b) Stat drift: `write_secs`/`write_throughput_mbps` now cover the delta
write only (cosmetic, non-security). 65 default green; clippy/fmt clean. O(1)-reflink wall-clock win to be
measured on real XFS in the next EC2 batch.

**Remaining prod-hardening (from the per-phase watch-lists, still open):** manifest GC + the GC actor
(F1/F2), orphan-object reconciler, per-publisher lease (claim→delete straddle), single-flight advisory
lock, materialize-into-nonempty-tree on pod restart, bounded retry on transient S3 5xx (a non-fence cycle
error currently crash-loops the daemon), RDS IAM auth / `verify-full` CA, in-region cold-materialize at
1–5 GB scale.

### 2026-07-22 — O6+O7 measured on real EC2 (parallel S3 uploads; reflink on XFS)
Throwaway rig (internal acct 993939946442, us-east-1, torn down): one **c6gd.4xlarge** (16 vCPU,
up-to-10 Gbps), instance-store NVMe formatted **XFS reflink=1**, **local Postgres on the NVMe** (the
lineage store — RDS+TLS was already proven in O5, so this batch didn't re-pay for it), binary rebuilt
in Docker with `--features pg,s3`. Workload: a 2.1 GB / 256-file **incompressible** tree (so
`upload_mb == 2148` — zstd can't shrink random data, giving a clean upload-volume control). Two new CLI
surfaces made the measurement reachable: `daemon --publish-workers N` (sweep pipeline width) and
`materialize --reference <dir> --ref-manifest <digest>` (invoke `materialize_incremental`).

**O6 — parallel S3 uploads take PUTs off the critical path.** Same instance, same tree, sweeping the
pipeline width. LOCAL store = CPU+local-write baseline (isolates compute); S3 = end-to-end; the
width-dependent gap between them is the exposed upload latency:

| workers | LOCAL wall (CPU+write) | S3 wall (e2e, run1/run2) | exposed upload ≈ S3−local |
|--:|--:|--:|--:|
| 1  | 17.0s | 24.2 / 24.4s | **~7.2s** |
| 2  | 16.5s | 17.3 / 17.3s | ~0.8s |
| 4  | 15.3s | 16.2 / 16.1s | ~0.9s |
| 8  | 13.0s | 14.2 / 13.8s | ~0.9s |
| 16 | 9.8s  | 10.9 / 10.7s | ~1.0s |

- **O6's specific win:** going 1→2 uploaders collapses exposed upload latency **~7.2s → ~0.8s** — a
  single S3 PUT stream (~0.7 Gbps here) can't keep up with the packer, so at W=1 the pipeline stalls on
  uploads; ≥2 streams overlap uploads with compute and the upload essentially disappears (S3 wall tracks
  the CPU-bound local wall within ~1s at every W≥2). Aggregate upload throughput goes from ~0.7 Gbps
  (serial) to ≥1.75 Gbps (overlapped).
- End-to-end publish **24.2s → 10.8s = 2.24×** across the width sweep (the compute half of that is O3's
  parallel pipeline: local 17.0→9.8s; the upload half is O6). Cross-batch, O5's pre-O6 baseline was
  ~43.8s for 2 GB on a serial-upload daemon — not a same-instance control, but directionally consistent.

**O7 — reflink incremental resume (XFS reflink=1).** gen2 = 13/256 files rewritten (~5% churn); LOCAL
store to isolate the CoW effect from network (which makes this a *lower bound* on the real win — see
below). Full `materialize` vs `materialize --reference`:

| metric | FULL materialize | INCREMENTAL (reflink) |
|---|--:|--:|
| wall time | 1.297s | **0.506s (2.6×)** |
| blocks fetched from store | 34 | **2 (17× less)** |
| files reflinked | 0 | 243 / 256 |
| disk actually written (df delta) | ~2049 MiB | **101 MiB (20× less)** |
| apparent size | 2049 MiB | 2049 MiB |
| output vs FULL | — | **IDENTICAL** |

- True CoW confirmed: only **101 MiB** hit the disk (the changed delta) though the tree is 2049 MiB
  apparent — the 243 unchanged files share extents with the reference. Byte-identical output.
- **The local-store numbers UNDERSTATE the production win.** `blocks_fetched 34 → 2` is the real lever:
  on a network-backed store a cold full resume re-downloads+decompresses all 34 blocks (~2 GB), while the
  incremental fetches 2 (~128 MiB) — a ~17× store-I/O reduction that, over S3, dominates wall-time far
  more than the 2.6× seen against a warm local NVMe. The reflink win grows with workspace size,
  unchanged-fraction, and store latency.
- Still gated on the same integration obligation as before: needs a **pristine retained reference** (never
  the agent's mutated live workspace) + "full materialize on any doubt". `materialize_incremental` now has
  a CLI caller but still no *daemon* caller — that wiring is the next integration-PR step.

Net: both optimizations validated on real hardware. Parallel uploads make S3 publish compute-bound rather
than network-bound (uploads hidden behind hashing/compression); reflink makes warm resume pay ~the delta
in both disk and store-fetch. Rig fully torn down (instance, bucket, IAM role/profile, SG — verified
empty). 65 default tests green; clippy/fmt clean.

### 2026-07-22 — O8: reflink warm resume wired into the daemon (`resume_via_reference`)
Where the O7 reflink win lands in production. Materialize-on-start could already do a full cold
`materialize`; O8 adds an OPT-IN warm-resume path behind `daemon --ref-dir <R>` (**omit ⇒ today's behavior
byte-for-byte** — zero default change). When set, the daemon keeps a **pristine, daemon-owned reference**
under `R` (scoped per lineage) and hands the agent a reflink-CLONE of it as the live `--tree`. Layout:
`R/<lin>/<digest>/` = an immutable full materialization; `R/<lin>/<digest>.ok` = its completeness sentinel;
`R/<lin>/current` = the committed digest; `.tmp-*` = in-progress builds (atomic-rename + swept on start).

`resume_via_reference(store, target, tree, ref_root)` (new `pub fn` in `daemon.rs` — unit-testable with a
LocalBlobStore, no Postgres): (1) validate `target` is 64-hex; sweep crash residue; (2) if `<target>/` is
COMMITTED (sentinel present + real dir) reuse it, else clear any incomplete/foreign `<target>/` and build
into `.tmp-<target>` INCREMENTALLY from the current committed reference (reflink unchanged, fetch only the
delta; on any error — e.g. prior manifest GC'd — full `materialize`), atomic-rename the body, then write the
`.ok` sentinel LAST; (3) commit `current` (atomic write-tmp-rename); (4) CLONE the pristine ref into `tree`
via `materialize_incremental(target, tree, target, <target>/)` — every regular reflinks (0 blocks fetched),
links recreated; (5) GC other refs (their extents were inherited via reflink, so CoW keeps them alive —
reclaims only the delta). Returns `ResumeStats { kind: ColdFull|WarmIncremental|RefReused,
ref_blocks_fetched, ref_reflinked, workspace_files }`.

**Why safe (the whole subtlety):** `materialize_incremental` reflinks a file whenever the target & reference
manifests agree on its chunk-list, and does NOT re-hash the reflinked bytes. So the reference must be
pristine — reflinking the agent's live workspace (whose bytes can diverge from any manifest via uncommitted
edits) would transplant wrong bytes. The reference is written by `materialize` (content-verified), the agent
only mutates `tree` (a CoW clone), and a reference is trusted only once its sentinel is written LAST — so an
incomplete/foreign `<digest>/` is rebuilt, never blindly reflinked. Crash-safety = tmp→atomic-rename (a
sentineled dir is always complete) + `current` committed after. `ref_root` MUST be EPHEMERAL node-local
storage (same class as `--cache-dir`): the reflink clone does no re-hash, so it trusts the sentinel — safe
because a node power-event wipes ephemeral storage → cold resume, but NOT safe on storage that survives
power-loss with metadata durable + file DATA un-flushed (no fsync barrier yet — deferred, documented).

**Two independent adversarial reviews (design + code): DESIGN SOUND WITH FIXES / SHIP WITH FIXES — all
findings applied.** The design review confirmed no interleaving (crash, concurrent 2nd daemon, agent racing
the clone, prior-ref GC, fs-boundary) lands wrong bytes in `tree`; the code review traced every crash point
in the `rename→sentinel→current→clone→gc` sequence and found each recovers cleanly (no normal-operation or
clean-crash corruption path). Fixes implemented: (#1) a completeness **sentinel** so the reuse branch can't
trust an externally-planted/partial dir; (#2) **per-lineage scoping** (`lineage_ref_subdir` hashes the
lineage id) so lineages can share a `--ref-dir` without clobbering each other's refs/GC; (#3) validate
`target`; (#4) lstat (not `is_dir`) in GC + the sentinel check; (#5) reject `--ref-dir` nested with `--tree`
+ document cross-fs as a REGRESSION (full copy). The code review's one should-fix (power-loss durability on
NON-ephemeral `ref_root`) is closed by documenting the ephemeral-storage requirement (it matches the system's
"NVMe is a pure cache" premise) and deferring a full fsync barrier.

Tests (`tests/resume.rs`, 12, all non-pg): cold→warm-incremental (delta-only fetch + byte-identical
workspace + old ref & sentinel GC'd); **the agent-mutation safety test** (scribble garbage into a resumed
workspace, then resume the next gen — the unchanged file carries pristine content, NOT the garbage);
**incomplete dir without sentinel is rebuilt** (not trusted); **symlink + hardlink recreation through a warm
resume**; **3-generation chain keeps only current**; ref-reuse on same-HEAD restart; crash-residue sweep +
committed-ref reuse with stale `current`; GC ignores a hex-named symlink; fallback to full when the prior
manifest is missing; corrupt/traversal `current` ignored; non-hex `target` rejected; injection-proof
`lineage_ref_subdir`. 77 default tests green (was 65); clippy/fmt clean. `--ref-dir` threaded through
`materialize_on_start` (5th arg, `None` = default) and the `daemon` subcommand.

Deferred (noted): fsync barrier for a non-ephemeral `ref_root` (unnecessary for the intended ephemeral
deployment); the clone into `tree` is non-atomic (crash mid-clone ⇒ partial tree ⇒ bail on restart,
identical to today's full-materialize semantics — `tree` must be provided empty each start); reflink-vs-copy
not surfaced in stats (`reflink_or_copy` returns `Option<u64>`); no per-(lineage,ref_root) advisory lock
(single-writer is a deployment invariant — an RWO PV reattached to the next pod provides it). End-to-end
daemon `--ref-dir` timing on real XFS is the next EC2 measurement.
