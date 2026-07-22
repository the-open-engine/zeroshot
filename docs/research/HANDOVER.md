# Handover — Capsule workspace store (standalone, pre-integration)

You are picking up an in-flight project: a **content-addressed workspace storage data plane** for
Zeroshot's multi-agent orchestrator running agent "capsule" pods on AWS EKS. The premise (already
proven): a workspace survives pod+node death and resumes on **any node/AZ** purely via a durable
content store (S3) + a lineage DB (Postgres), with **no shared POSIX filesystem** (EFS/FSx were rejected
as too slow). Node-local NVMe is only ever a cache.

Everything is built and validated as a **standalone Rust crate**, behind sync traits designed to
drop into the `zeroshot-cloud` platform later. **It is NOT integrated into the platform, and you must
not integrate it** — this is still the experimentation/optimization phase.

## 0. READ THESE FIRST (authoritative, in order)
1. `docs/research/production-build-log.md` — **THE running log.** Every phase, every review verdict,
   every fix, all the AWS measurements. This is your primary source of truth; read it fully.
2. `planning/plans/0001-production-backends.md` — the approved plan (architecture decisions MF1–MF4,
   the 5 phases, the open questions and their rulings).
3. `docs/research/design-decision-experiments.md` — the prototype experiments + the **three-invariant
   GC safety model** (the E8 section) and F1 (the reuse clock). Load-bearing for anything touching GC.
4. The code (crate `capsule-workspace-core/`): `src/{cas,s3,cache,lineage,pg,refclock,gc,gc_pg,daemon,
   daemon_loop,manifest,ifaces}.rs`, `src/main.rs`, and the `tests/`.

## 1. Where things are
- Repo: `zeroshot` (the-open-engine/zeroshot). **This is a git worktree**; cwd is the worktree root.
- Branch: `claude/zeroshot-workspace-file-transfer-13f0be` (pushed; work goes here, PRs later).
- Crate: `capsule-workspace-core/` (its own `[workspace]`, so `cargo` commands run from that dir).
- Tracker/spec + all progress comments: GitHub issue **the-open-engine/zeroshot#744**.

## 2. Current state (all done, validated, pushed)
- **Prototype** (chunk/hash/zstd → 64 MiB blocks + per-publish manifest; grace-period mark-sweep GC),
  hardened through an independent audit + 2 adversarial rounds + a final review. Three-invariant GC.
- **Production backends (Phases 1–5)**, each independently senior-reviewed and pushed:
  - `S3BlobStore` (real S3) — validated on MinIO + real AWS S3.
  - `PgLineageStore` (fence CAS) + `block_ref` reuse-clock (`PgRefClock`) — validated on real RDS/TLS.
  - `gc_pg` (clock-driven GC, MF1 atomic claim) — grace=0 live-loss reproduced then fixed.
  - The `daemon` (materialize-on-start → interval publish → SIGTERM drain; MF2 threading; `/health`).
- **Real-AWS validation**: full-stack e2e (S3 + RDS/TLS) AND a **2-node cross-node transfer** (node A
  publishes → node B in another AZ materializes byte-identically, no shared FS) — R1/R4/R2 proven.
- **Optimization pass O1–O5** (this is the phase you're continuing):
  - O1: content-only manifest identity (`parent` out of `logical_digest`) + daemon `CycleOutcome::NoChange`
    → an idle daemon does zero writes (killed the F1 manifest-churn leak).
  - O2: `CachedBlobStore` (node-local NVMe cache tier) + daemon `--cache-dir`.
  - O3: daemon uses `publish_pipelined` (`workers = available_parallelism.clamp(1,16)`).
  - O4: store-wide `gc` subcommand + `PgLineageStore::all_head_digests()` (the F2 fix).
  - O5 real-EC2 numbers: **warm resume 1.48s vs cold 3.74s for 2 GB (under the <5s R2 target)**;
    publish pipeline ~2× at 16 workers; 5 GB materialize 3.22s; **S3 publish is upload-bound**.
- **Tests: 59 default green** + gated suites (S3 via MinIO/real; pg/daemon/gc via Docker-PG/real RDS;
  `e2e_aws` on real S3+RDS). clippy + rustfmt clean. All AWS torn down.

## 3. HARD CONSTRAINTS (non-negotiable — violating these is the main failure mode)
- **DO NOT integrate into `zeroshot-cloud` / the platform.** Standalone only. The user will say when to
  integrate. (When that happens, `src/ifaces.rs` is the compatibility seam; it mirrors the real
  zeroshot-cloud shapes — labels say which are literal mirrors vs. neutral generalizations.)
- **AWS = internal account `993939946442` ONLY.** Account `794285265617` (dev) is LIVE/shared — never
  touch it. Always **tear down** every resource you create (instances, RDS, S3 buckets, SGs, IAM). The
  user has authorized "test on AWS as before" (small spend, torn down); each run is ~$1.
- **You do NOT run `aws sso login`** — that authenticates as the user; it's their action. When the SSO
  token expires (`Token has expired`), STOP and ask the user to run:
  `aws sso login --profile covibes-933` (refreshes the `toec` sso-session the `internal` profile assumes
  from). Then drive with `AWS_PROFILE=internal AWS_REGION=us-east-1`.
- **Never run Zeroshot clusters or spend API credits without explicit user permission** (this repo's
  CLAUDE.md rule — unrelated to the above, but it applies).
- Push after each phase/finding so work isn't lost. Use **`git push --no-verify`**: the pre-push hook
  fails on ~14 PRE-EXISTING unrelated ESLint errors in repo JS test files (NOT ours; the hook's real
  target main/dev is untouched). Pre-commit runs rustfmt → `cargo fmt` before committing.
- **No `Co-Authored-By:` trailer** on commits (user's global pref). Avoid backticks/`>`/`(` quirks in
  `-m` bodies under zsh — use `git commit -F <file>` for multi-line messages.
- Test only **realistic scenarios + edge cases** — no contrived/"post-quantum" threat models.
- Do NOT edit the repo's `CLAUDE.md`.

## 4. Architecture invariants you must not break
- **Sync traits, async CONTAINED in adapters.** `BlobStore`/`LineageStore`/`RefClock` are sync; the
  whole publish/materialize/GC pipeline is CPU-bound rayon batch work. `S3BlobStore` and `PgLineageStore`
  each hold their OWN bounded multi-thread tokio runtime and `block_on` inside the sync methods (there's
  a `debug_assert!(Handle::try_current().is_err())` tripwire). The daemon is a **plain sync `fn main`**;
  signals (`signal-hook`), the `/health` `TcpListener`, and the pipeline all run on std/rayon threads
  with **no ambient tokio runtime** — else the adapters' `block_on` nest-panics ("runtime within a
  runtime"). This is MF2; keep it.
- **GC three-invariant safety** (see the E8 doc): (1) grace > max(publish, sweep) duration + clock skew;
  (2) MARK — a block referenced by any live-HEAD manifest is never collected regardless of age;
  (3) refresh-on-every-reference — every referenced block is `touch`ed young before commit. GC deletes
  are **row-claim-first** (atomic `DELETE ... WHERE last_referenced_at < clock_timestamp()-grace
  RETURNING`, re-checked server-side), then the S3 object (MF1).
- **F2 (data-loss footgun):** `gc_pg::collect`'s `live` set MUST be **store-wide** (ALL lineages' HEADs,
  via `all_head_digests()`), never per-lineage — a per-lineage sweep deletes other lineages' live blocks.
  GC is a **store-wide singleton actor** (the `gc` subcommand), separate from the per-lineage daemons.
  There's a loud warning on `collect` and a test that demonstrates the footgun; respect it.
- **Cache is never authoritative:** `CachedBlobStore` writes to the durable backing FIRST, then cache
  best-effort; content-addressing makes staleness harmless; deletes hit both tiers.
- **Manifest identity is content-only** (O1): `logical_digest` excludes `parent` and the physical block
  index. Don't reintroduce either into the digest (it caused the F1 churn).

## 5. How to build / test / run
From `capsule-workspace-core/`:
- Default (lean, no AWS/PG): `cargo test --release` (59 tests). `cargo clippy --all-targets`, `cargo fmt`.
- S3 tests: `cargo build --features s3`. Local via MinIO:
  `docker run -d -p 9010:9000 minio/minio server /data`, make a bucket, then
  `S3_ENDPOINT_URL=http://localhost:9010 S3_BUCKET=... AWS_ACCESS_KEY_ID=minioadmin
  AWS_SECRET_ACCESS_KEY=minioadmin AWS_REGION=us-east-1 cargo test --features s3 --test blobstore_conformance`.
  Real S3: set `S3_IT=1` + `S3_BUCKET` + creds (no `S3_ENDPOINT_URL`).
- PG tests: `docker run -d -p 5433:5432 -e POSTGRES_PASSWORD=pg -e POSTGRES_DB=capsule postgres:17.10-alpine`,
  then `DATABASE_URL='postgres://postgres:pg@localhost:5433/capsule?sslmode=disable' cargo test --features pg`.
  All pg/daemon/e2e tests SKIP (with a printed notice) if `DATABASE_URL` is unset — never silently.
- Full stack e2e (real AWS): `tests/e2e_aws.rs`, gated on `S3_IT=1 + S3_BUCKET + DATABASE_URL` (RDS,
  `sslmode=require` — RDS enforces force_ssl; the crate's sqlx has `tls-rustls-ring`).
- Daemon CLI: `cargo run --features pg,s3 -- daemon --tree <dir> --store s3://<bucket> --db
  <postgres-url> --lineage <id> [--cache-dir <nvme>] [--publish-interval N] [--health-addr ip:port]
  [--once]`. GC actor: `... gc --store <uri> --db <url> --grace-secs N`.
- **AWS experiment rig pattern** (from the build log): build the Linux binary in Docker (native arm64 on
  a Mac: `docker run --platform linux/arm64 -v $PWD:/work -e CARGO_TARGET_DIR=/tmp/target -w /work
  rust:1-bookworm cargo build --release --features pg,s3`), upload to S3, launch Graviton **`c*gd`**
  instances (NVMe instance store; user-data `mkfs.xfs /dev/nvme1n1 → /mnt/nvme`), IAM instance profile
  (`AmazonSSMManagedInstanceCore` + `AmazonS3FullAccess`), drive via **SSM `send-command`** (base64 the
  script to dodge CLI quoting), RDS `db.t4g.micro` PG17 with an SG scoped to the EC2 SG + your IP.
  `c7gd` capacity is flaky per-AZ — fall back across AZs/types. Throwaway RDS password; tear everything down.

## 6. Open work — pick up here (prioritized)
**DONE since the last handover:** O6 **parallel S3 block uploads** (packer round-robins finalized blocks
to `workers` uploader threads; reviewed — fixed a GiB-scale channel-buffering regression + a debug-only
test) and O7 **reflink incremental resume** (`materialize_incremental` reflinks unchanged files from a
reference; reviewed as security-critical — no security property dropped). Both pushed. Then **measured on
real EC2** (c6gd.4xlarge, XFS reflink=1, local PG; rig torn down): O6 — 1→2 uploaders collapses exposed
upload latency ~7.2s→~0.8s (uploads hidden behind compute; S3 wall tracks CPU-bound local wall within
~1s at every W≥2), end-to-end 2.24× across the width sweep; O7 — reflink writes only the ~5% delta
(101 MiB of 2049) and fetches 2 blocks vs 34 (17× less store I/O), byte-identical output. Two CLI knobs
added for this: `daemon --publish-workers N` and `materialize --reference <dir> --ref-manifest <digest>`.
Full numbers in the build log (2026-07-22 entry).

**Optimization follow-ups (biggest headroom first):**
1. **Wire `materialize_incremental` into the daemon** — O7 built the mechanism + a CLI caller, but there's
   still NO *daemon* caller (`daemon_loop::materialize_on_start` does a full `materialize`). This is where
   the reflink R2 win actually lands in production, and the EC2 batch quantified how big it is (17× less
   store fetch on the delta). REQUIRES a **pristine retained reference** (a prior materialize output kept
   immutable, NEVER the agent's mutated live workspace) + "full materialize on any doubt" as the default.
   Workspace-lifecycle work (keep the old gen as a read-only reference, materialize the new gen with
   reflink, swap).
2. **mtime-skip** so an idle daemon avoids even re-hashing the tree (today `NoChange` still re-hashes).
3. **Orphan-object reconciler** (needs adding `list` to `BlobStore`): list the store, drop objects with
   no `block_ref` row older than grace — closes the documented GC crash/straddle orphan residual.
4. **LVM-thin same-node COW fork** — not yet measured (writer→reader fork; from the original research).

**Deferred integration/hardening (do NOT start integration without the user's go):**
- Manifest GC (blocks are GC'd; manifests aren't — tiny, but they accumulate).
- Per-publisher lease to fully close the narrow claim→delete straddle (documented residual).
- Single-flight GC advisory lock (correctness is already in the per-block atomic claim; this avoids
  wasted work).
- materialize-into-nonempty-tree on pod restart (today `materialize` fail-safes on a non-empty dir).
- Bounded retry on transient S3 5xx in `publish_cycle` (today a non-fence error crashes the daemon).
- RDS IAM auth + `sslmode=verify-full` with the RDS CA (today password + `sslmode=require`).

## 7. Working discipline (what the user expects)
Research first → write/adjust a plan → implement in reviewable phases → **spawn an independent
senior-review agent as a gate on each meaningful change** (they have caught real bugs every time:
an R1-corrupting GC ordering, a touch() that would fail every publish, a grace=0 live-loss) → apply
must-fixes → `cargo fmt`/clippy/test → push (`--no-verify`) → note it in `production-build-log.md` and
comment on issue #744. Be honest about status (the user values "components proven" vs. "deployable
whole" precision). There's a `team-programming` skill in the **zeroshot-cloud** repo's `.claude/skills/`
(not this repo) describing this flow. When you finish a chunk, report the result and the numbers, and
ask before large new spend or scope.
