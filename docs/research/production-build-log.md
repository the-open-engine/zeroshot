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
