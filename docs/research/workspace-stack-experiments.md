# Workspace stack — experiment log

Running notes from validating the proposed capsule workspace data plane
([spec](../specs/capsule-workspace-storage.md)) with real experiments on AWS.
Account `993939946442` (internal/research), us-east-1. Updated as experiments land.

## Method & honest scope

- **What is exercised:** the full **data plane** as a faithful single-node prototype
  against **real S3** — content-addressed chunking (256 KiB) → zstd → ~64 MiB blocks →
  manifest, incremental dedup publish, cold/warm materialize, LVM thin-snapshot freeze,
  git plane, S3 throughput, gVisor boot floor.
- **What is NOT exercised (and why):** the control plane — capsule-operator, node
  DaemonSet, Karpenter scheduling — is **unbuilt greenfield code**, and the account's
  **4-vCPU on-demand quota** cannot host a real EKS+gVisor cluster. Its floor (gVisor
  boot, image pull) is measured as a proxy; the rest needs the built stack + a quota bump.
- **Instance:** `c6id.xlarge` (4 vCPU, 8 GiB, 1×~237 GB Nitro instance-store NVMe),
  not the pool's `c6id.4xlarge` (16 vCPU) — blocked by the vCPU quota. **Consequence:**
  absolute throughput on 4 cores is a **conservative floor**; the real 16-core node is
  faster with parallelism. Ratios (native-vs-gVisor, dedup %) transfer across size.
- Every number is measured on real hardware/S3, drop_caches between timed runs.

## Prior result (materialization microbench, earlier run)

100k-file / 2 GB tree, XFS reflink on instance-store NVMe:

| Op                       | Native host | gVisor gofer | gVisor directfs | runc  |
| ------------------------ | ----------- | ------------ | --------------- | ----- |
| hardlink farm (`cp -al`) | **1.61s**   | 8.13s        | 7.59s           | 1.80s |
| reflink / full copy      | 3.75s       | 32.2s        | 32.4s           | 3.02s |
| stat every file          | 0.41s       | 0.31s        | 0.30s           | 0.04s |

**Takeaways already banked:** (1) in-sandbox materialization is 5–8× over budget →
materialize on the **host** (daemon), which the spec mandates; (2) `--directfs` does not
rescue it; (3) prefer **hardlink** (1.6s) over reflink (3.75s) for read-only fan-out.

## Results (measured on real hardware/S3, this run)

> **Read the REAL vs ARTIFACT column carefully.** The prototype CAS is single-threaded
> Python (the real plan is Rust), so wall-clock chunk/compress/decompress/write times are
> **prototype artifacts** and stated as _upper bounds_. The trustworthy numbers are the
> I/O rates, S3 request counts, structural results (block counts, O(1) snapshot), and the
> ratios. The synthetic corpus (urandom) also makes dedup/compression untrustworthy — a
> real-code corpus run (§ "Real corpus" below) supplies those.

### The results that decide the architecture (trustworthy)

| Finding                                                        | Number                                                       | Why it matters                                                                                                                                                                             |
| -------------------------------------------------------------- | ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Materializing 102k files needed only 16 S3 GETs**            | 16 blocks                                                    | The chunks→64 MB-blocks packing works. A cold node fetches a whole workspace in ~dozen requests, not 100k — kills the S3 small-object cost/latency problem outright.                       |
| **Cold block fetch from S3**                                   | ~1 GB in **2.87s ≈ 350 MB/s**                                | Real cold-node download rate on the c6id NIC (single instance, 32 parallel GETs). This is the cold-start binding constraint.                                                               |
| **Parallel block upload to S3**                                | **1.09 GB/s** (1007 MB / 0.92s, 32 threads)                  | The publish path saturates the NIC far better than a single CRT `cp` (382 MB/s) — publish is not upload-bound.                                                                             |
| **Incremental publish after a small edit**                     | **26 MB uploaded, 1 block, 0.28s** (of a 1.86 GB workspace)  | A publish barrier ships only the delta. This is the whole economic case for the design. (Absolute dedup % here is corpus-inflated; mechanism is proven.)                                   |
| **LVM thin snapshot = O(1) freeze**                            | **0.59s @ 1k files vs 0.66s @ 100k files**                   | 100× the files, same ~0.6s. The publish-barrier "freeze" is independent of tree size, as the spec claims.                                                                                  |
| **git WIP commit + push (fast durability tier)**               | commit **0.28s** + push **0.36s** = ~0.6s for a 20-file edit | The seconds-cadence RPO≈0-for-committed tier is cheap and real.                                                                                                                            |
| **gVisor sandbox boot floor**                                  | runsc **~1.7s** vs runc ~1.3s (docker run→exit ×3)           | gVisor adds ~0.4s; the sandbox boot is a large fixed chunk of any <5s budget — confirms the spec's "boot dominates, not storage."                                                          |
| **S3 small-object first-byte latency (Standard, same region)** | p50 **26 ms**, p99 **63 ms**                                 | Far better than the 100–200 ms guideline (warm connection, same region). Manifest fetch is cheap; **S3 Express One Zone's ~5 ms buys nothing here** — confirms the spec's rejection of it. |

### Prototype-artifact numbers (upper bounds only — real Rust impl is far faster)

| Op                                       | Prototype (Python, 4 vCPU)                         | Note                                                                   |
| ---------------------------------------- | -------------------------------------------------- | ---------------------------------------------------------------------- |
| Publish v1 wall (chunk+hash+zstd 1.8 GB) | 20.9s                                              | single-threaded Python sha256+zstd; Rust + N cores → seconds           |
| Cold materialize wall (102k files)       | 19.6s (fetch 2.9s **real** + write 16.2s artifact) | write is single-threaded Python decompress+write                       |
| Warm materialize wall                    | 14.9s (fetch 0.65s + write 13.6s artifact)         | the earlier **host hardlink microbench (1.6s)** is the real warm floor |

### Corpus-artifact numbers (do NOT quote — synthetic urandom corpus)

- v1 "dedup" 74.6% and compression ~1.0× are **artifacts**: all files were slices of one
  random buffer (fake chunk collisions), and random data is incompressible. Real numbers
  come from the real-corpus run below.

### Environment artifacts (not real limits)

- "GET 2GB `aws s3 cp` rc=1" — `/tmp` is a 3.9 GB **tmpfs**; it ran out of room, not an S3
  fault. Real GET rate is the 350 MB/s block-fetch above.
- "git blobless clone rc=128" — partial clone over a `file://` local bare repo needs
  `uploadpack.allowFilter`; works against real GitHub. The WIP commit+push path (the one
  that matters) succeeded.

## Real corpus (trustworthy dedup + compression)

Real Python `site-packages` (numpy/pandas/scipy/flask/sqlalchemy/… = 13,698 files, 374 MB
— the `node_modules` analog). A = 10 base packages; B = A + httpx/typer/tenacity (fresh
independent venvs).

| Metric                               | Value                                     | Note                                                                          |
| ------------------------------------ | ----------------------------------------- | ----------------------------------------------------------------------------- |
| **zstd compression on real code**    | **2.89×** (356 MB raw → 122 MB uploaded)  | The trustworthy compression ratio (vs ~1.0× on the synthetic urandom corpus). |
| Within-corpus dedup (one install)    | 7.7%                                      | Modest — real distinct files.                                                 |
| Re-publish an **unchanged** tree     | **0 bytes, 0 blocks**                     | Correct CAS property (found via a venv bug — see below).                      |
| Incremental publish (A → B, +3 pkgs) | **25.7 MB uploaded** (of 359 MB), 1 block | ~7% of the workspace shipped for a dependency add.                            |
| Chunks marked "new" for that add     | 4,946 (34% of tree)                       | **Far more than 3 packages** — the important finding below.                   |

### Key finding: derived artifacts are non-reproducible and degrade cross-install dedup

Adding 3 small packages perturbed **34% of the chunk set** while `du` grew only ~4 MB.
Probing why:

- **16% of the tree is `.pyc` bytecode** (4,602 files, 60.9 MB of 374 MB).
- `numpy/__init__.py` (source) is **byte-identical** across two installs; `numpy/__pycache__`
  (the compiled `.pyc`) **differs**.
- **962 of 2,999 nominally-identical shared-path files (32%) differ byte-for-byte** between
  two independent installs — `.pyc` (embedded compile timestamps/paths) plus `.dist-info`
  `RECORD`/`direct_url.json` churn.

**Implication for the architecture — this sharpens the dedup story rather than breaking it:**

- **Temporal dedup (one lineage evolving on one node) is the STRONG case.** When the writer
  edits its own tree and republishes, unchanged files keep identical bytes → the synthetic
  in-place edit measured **97.7% dedup / 26 MB uploaded** for a 1.86 GB tree.
- **Cross-install / cross-project dedup is the WEAK case** (~66% even for near-identical
  trees) because of `.pyc`/metadata non-determinism.
- The stated product priority — _"users hammer ONE project for weeks"_ — is exactly the
  **temporal/same-lineage case**, i.e. the strong one. So the design's dedup benefit lands
  where the priority is. Cross-org dedup (which the spec already forfeits for tenant
  isolation) was the weak case anyway — little is lost.
- Optional future optimization: excluding `__pycache__`/`.pyc` from snapshots would recover
  a chunk of cross-install dedup, but it violates R3 (no structure assumptions) and isn't
  needed for the priority workload. **Do not do it in v1.**

## Rust core (real implementation, replaces the Python prototype's wall-times)

The minimal core ([`capsule-workspace-core/`](../../capsule-workspace-core)) was built,
round-trip-verified, and measured on a `c6id.2xlarge` (8 vCPU) against the real 374 MB
dependency corpus. This closes the "real CAS throughput" open item — the Python prototype's
slow wall-times were purely single-threaded-Python artifacts.

| Stage                                   | Rust core (8 vCPU, rayon)                 | Python prototype (4 vCPU) |
| --------------------------------------- | ----------------------------------------- | ------------------------- |
| Publish 374 MB dep tree (cold, all new) | **0.90s** wall                            | ~5.8s                     |
| — hash throughput                       | **1.11 GB/s**                             | ~88 MB/s (1 core)         |
| — zstd compress throughput              | **0.98 GB/s**                             | —                         |
| Incremental publish (+3 pkgs)           | **0.48s** wall, 25.9 MB uploaded          | ~26 MB                    |
| Cold materialize 14,063 files           | **0.28s** wall (3 blocks, write 3.3 GB/s) | ~16s (Python write)       |
| Round-trip identity                     | ✅ verified                               | ✅                        |
| zstd ratio / dedup %                    | 2.87× / 65.8% incremental                 | 2.89× / 65.8% (matches)   |

`cargo build --release` on the instance: **20s**. Compiles clean, no warnings of note.
Ratios (compression, dedup) match the Python run exactly; only the throughput is now real.

## Correctness & adversarial suite (Rust integration tests, `capsule-workspace-core/tests/experiments.rs`)

Realistic scenarios only (untrusted tenant tree, corrupt/tampered store, weird-but-legal
filenames) — not contrived. Each is an auditable assertion; run `cargo test`. **7 real gaps
found; 6 fixed in the same pass, 1 is a design item.**

| ID     | Scenario                                                          | Before                                                                                                                                          | Now                                                                                                                                                                                                                          |
| ------ | ----------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| G1     | Symlink fidelity                                                  | ⚠️ silently dropped                                                                                                                             | ✅ preserved (`symlinks_preserved`)                                                                                                                                                                                          |
| G2     | Path traversal via tampered manifest                              | ⚠️ `out.join(..)` escaped                                                                                                                       | ✅ refused (`safe_rel_path` + `tampered_manifest_path_traversal_refused`)                                                                                                                                                    |
| **G4** | **Manifest digest determinism**                                   | ⚠️ **non-deterministic** — HashMap chunk index serialized in random order + HashMap-order block packing → identical tree gave different digests | ✅ fixed (BTreeMap index, sorted packing, sorted files; `idempotent_and_deterministic`). **This was a genuine correctness bug for a content-addressed system** — unstable digests break manifest dedup and lineage identity. |
| G5     | Chunk integrity on read                                           | ⚠️ none (trusted blocks blindly)                                                                                                                | ✅ verify sha256==id + rlen (`corrupt_block_detected`)                                                                                                                                                                       |
| G6     | Decompression bomb                                                | ⚠️ unbounded decompress                                                                                                                         | ✅ bounded to CHUNK (`decompression_bomb_bounded`)                                                                                                                                                                           |
| G7     | Special files (fifo/socket/device)                                | ⚠️ silently dropped                                                                                                                             | ✅ counted + skipped (`special_files_counted_not_silently_dropped`)                                                                                                                                                          |
| C2     | Concurrent-writer fence                                           | ✅ already correct                                                                                                                              | ✅ verified (`concurrent_fence_rejects_stale`)                                                                                                                                                                               |
| C7     | Idempotent republish                                              | ✅                                                                                                                                              | ✅ verified (0 new chunks)                                                                                                                                                                                                   |
| —      | Mode preservation, mixed-tree round-trip, unicode/space filenames | —                                                                                                                                               | ✅ verified                                                                                                                                                                                                                  |

**Still open as a design item (not a quick fix):**

- **G3 manifest index scaling** — the manifest inlines the full chunk index (307 B/file →
  307 MB @ 1M files), which breaks the "fetch a small manifest first" premise at scale. Fix
  is structural (shard the index into CAS objects à la Modal/Xet), tracked separately.
- **Memory footprint** — publish holds all new chunk bytes in RAM; a large first publish
  would OOM. Needs streaming. (Measured next / on EC2.)

## Bottom line

Every load-bearing assumption in the spec that could be tested on one node **held**:
content-addressed blocks collapse 100k files into ~16 S3 GETs; the publish barrier is O(1);
incremental publish ships only the delta; the git fast-tier is ~0.6s; gVisor boot (~1.7s),
not storage, dominates the start budget. Real compression is ~2.9×. The one nuance the
experiments _added_ is that strong dedup is temporal (same project over time) — which is
precisely the workload. Remaining unmeasured pieces need the built control plane and a
16-vCPU quota: real pod scheduling latency, Karpenter cold-node launch, and the Rust CAS's
true chunk/materialize throughput (the Python prototype's wall times are not representative).
