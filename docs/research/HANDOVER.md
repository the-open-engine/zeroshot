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
Full numbers in the build log (2026-07-22 entry). Then O8 **wired reflink warm resume into the daemon**
(`resume_via_reference` behind `daemon --ref-dir <R>`, opt-in; a pristine daemon-owned reference + a
reflink-cloned live workspace, so resume fetches only the delta). Two independent adversarial reviews
(design + code) → DESIGN SOUND WITH FIXES / SHIP WITH FIXES, all findings applied (completeness sentinel,
per-lineage scoping, target validation, lstat GC, nesting guard, ephemeral-`ref_root` doc). 77 default tests
green (was 65). See the build log's O8 entry.

O8 was then **measured end-to-end on real EC2** (c6gd.4xlarge, XFS reflink=1, real S3, no block cache; rig
torn down): time-to-workspace-ready for a gen2 resume — **cold 4.01s → warm-incremental 1.42s (2.8×) →
ref-reused 0.23s (17.4×)**, fetching only 2 blocks and reflinking 243/256 files, all workspaces
byte-IDENTICAL to the cold one. A reflink-cloned workspace costs **0.1 MiB** of disk vs **1904 MiB** for a
full materialize (same 2049 MiB apparent). The first start with `--ref-dir` measured 6.86s vs 4.01s cold,
but **that delta's attribution was retracted** — the reflink pass is ~0.02s (measured by local A/B), and the
two EC2 runs weren't comparable (the with-ref run paid first-in-batch S3 TLS/DNS warmup). O9 added
`reflink_secs` so the next batch can attribute it. Full numbers in the build log's O8 entry.

**Optimization campaign: closed, but read this before you believe any number in it.**

Measured at realistic scale (100k files / 520 MB): a steady-state publish cycle is ~0.58s and is now
STAT-bound; a cold materialize is ~7s, per-file-syscall-bound. What landed: parallel S3 uploads (O6),
reflink warm resume + the daemon reference lifecycle (O7/O8), the publish re-hash skip (O10), manifest
memoization (O11), a bounded materialize working set (O13→O23), a bounded block cache (O16), and ARMv8
hardware SHA-256 (O18, 5.1x — it had never been enabled, so **every EC2 number in the build log predates
it and understates hash-bound paths by up to ~5x**).

**The campaign's own failure mode, stated plainly because it will bite you too.** Of the last ten commits,
six fixed defects introduced by earlier ones in the same campaign. **Four memory bounds shipped that no
test could observe.** Three safety mechanisms shipped that provably did nothing while their tests passed
(a valve that was an unused parameter; a racy-window guard whose tests moved the safety-critical field
permissively; a block pool whose claimed fix was never in its own diff). Every one was caught by an
independent review or by a test written specifically to be able to see the failure — **never by the
change's own benchmarks**. Two rules came out of that, and they are not optional:
- **Benchmark on a production-shaped fixture** (~100k small files, mixed compressibility). A 256-large-file
  fixture hid three separate defects: per-file cost (22ms at 256 files vs ~10.8s at 95k), a wall regression
  visible only at 100k, and a memory bound that only held at compression ratio ~1.
- **Mutation-test every guard.** Revert it; the test must fail. Where the guard is a constant, a runtime
  test cannot do this (asserting "groups fit the budget" passes vacuously if you raise the budget) — use a
  `const _: () = assert!(...)` so a bad value fails to COMPILE.

**Ranked remaining work, per the review gate** (this supersedes an earlier verdict here that nothing
performance-shaped was worth doing — that was wrong at 100k-file scale):
1. **Ranged/partial block reads.** The real remaining S3 win and NOT a constant factor: a whole 64 MiB
   block is currently fetched to read one 256 KiB chunk. Measured **4880 MiB → 1604 MiB (3.0x)** on a
   realistic fixture, **250x** on an eroded one. It also makes the block-size estimate exact by
   construction, dissolving the memory-bound problem rather than patching it, and removes the 4.4x
   fetch-concurrency cost the current bound imposes. A defaulted `get_block_range` on `BlobStore` keeps
   every existing impl and test double compiling.
2. **Manifest compression at rest.** 100k-file manifest **30.2 MiB JSON → 7.43 MiB zstd L1 (4.1x)**, 27ms
   encode / 11ms decode. Identity is `logical_digest` over logical content, so compression cannot perturb
   it; sniff magic bytes for back-compat.
3. **A soak test** — N cycles of an agent-like writer against a full-rehash oracle, real timestamps. The
   only shape that finds timing-dependent skips, and the one this system most needs before real work.
4. **A coarse-granularity-filesystem test in CI.** A reviewer found a live data-loss bug that way; nothing
   in CI covers it.
5. **Re-measure O5/O6/O8 on EC2** with the 100k fixture AND hardware SHA before quoting any of them.
6. Explicitly **NEVER**: `[u8;32]` ids (measured ~2% of RSS for a crate-wide, format-adjacent change) and
   the per-file syscall diet (measured and refuted: 1.04x write path, 1.01x reflink pass).

**Known and accepted** (gate-ruled coverage debt, not hazard). Carry these forward; none is a live bug:
- `publish_pipelined` dropping `blen` is untested (only `publish` is covered); the `0 ⇒ BLOCK_TARGET`
  old-manifest fallback is unobservable; deleting the grouper's byte check leaves the suite green.
- **`MATERIALIZE_WAVE_BYTES` is the largest unguarded memory lever** — mutating it to 4 GiB costs **7.1x
  peak RSS with a green suite**, and it is covered by neither `const` assertion. Pre-existing, not
  introduced by this campaign. If you touch materialize's memory, guard this the way the other two are.
- **Hoisting the per-group `held` map out of its loop reintroduces unbounded residency** (6.2x, green
  suite). No constant is involved, so no assertion can catch it;
  `materialize_holds_only_a_bounded_slice_of_blocks` prints peak concurrency but asserts nothing about it.
  Adding that assertion is the cheapest way to close it.
- Refetch amplification, as recorded across the campaign: **2.28x → 2.78x** on a realistic lineage shape,
  **3.00x** at 40 blocks, **13.00x at 200 blocks**. (An earlier draft of this line said "2.47-3.00x" — an
  unprovenanced range that also dropped the 13x worst case. Restored.) Ranged reads, item 1 above, are the
  fix; the deleted `BlockPool` never affected it.
- `cross_wave_block_refetch_is_measured_and_bounded` runs on a fixture that produces a SINGLE wave, so it
  cannot observe cross-wave refetch at all — the same vacuity as the one-block fixture it replaced, on a
  different axis. Give it a genuinely multi-wave fixture before trusting it.
- Cosmetic: `tests/materialize_bounds.rs` still mentions `BlockPool` in the present tense inside a
  paragraph about "the previous attempt". Delete it next time that file is touched — this campaign has a
  specific history of comments describing code that does not exist.

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
