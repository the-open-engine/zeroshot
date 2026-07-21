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
