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

> **⚠️ SUPERSEDED (publish rows) by the V2 hardening.** These publish numbers are the
> **pre-V2, rayon-parallel** publish. The V2 fix for the sparse-file OOM DoS made publish
> **single-threaded streaming** (bounded memory), which trades throughput: publish is now
> **~276 MB/s single-threaded** (measured, 1 GiB distinct: 3.9s), not 1.11 GB/s. The
> memory win is deliberate and large; the real daemon reclaims throughput with a
> bounded-parallel producer/queue/packer pipeline. `materialize` is unchanged (still
> rayon-parallel: 14k files in 0.28s). Compression/dedup ratios below are unaffected.

| Stage                            | Rust core (8 vCPU)                        | Python prototype (4 vCPU) |
| -------------------------------- | ----------------------------------------- | ------------------------- |
| Publish 374 MB (PRE-V2 parallel) | 0.90s wall (superseded — see note)        | ~5.8s                     |
| Publish (POST-V2 streaming)      | **~276 MB/s single-thread, bounded RAM**  | —                         |
| Cold materialize 14,063 files    | **0.28s** wall (3 blocks, write 3.3 GB/s) | ~16s (Python write)       |
| Round-trip identity              | ✅ verified                               | ✅                        |
| zstd ratio / dedup %             | 2.87× / 65.8% incremental                 | 2.89× / 65.8% (matches)   |

`cargo build --release`: clean, **no warnings** (V3-verified). Ratios match the Python run;
the durable takeaway is that the Python wall-times were single-threaded-Python artifacts.

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

### D1 — fixed-256K vs CDC on real edit types (10 MB file, 40 chunks)

| Edit                                 | New chunks  | Verdict                 |
| ------------------------------------ | ----------- | ----------------------- |
| Append 1 KB at end                   | **1 / 40**  | fixed-block fine        |
| Modify 1 byte in place (same length) | **1 / 40**  | fixed-block fine        |
| Prepend 1 KB (shifts all boundaries) | **41 / 40** | ⚠️ fixed-block defeated |

Only **byte insertion/deletion that shifts boundaries** defeats fixed chunking; append and
same-length edits are fine. The big dedup win is on **dependency trees** (files replaced
wholesale on version bump, never edited in place) and source lives in the git plane — so the
shift weakness rarely bites the workspace workload. **This confirms the spec's decision to
defer content-defined chunking (CDC) to a metric-gated v2** rather than build it now.

### Scale / memory / crash (EC2 c6id.2xlarge, 8 vCPU, 15 GB RAM, hardened Rust core)

| ID     | Scenario                                          | Result                                                                                                                                                                                    |
| ------ | ------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **C6** | Publish memory footprint (3 GB all-distinct tree) | ⚠️ **peak RSS 9.07 GB = 2.95× tree size** — prototype buffers per-file chunks + new-chunk map + block buffers. 5 GB workspace ⇒ ~15 GB RAM. **Real daemon must stream (bounded memory).** |
| **G3** | Real manifest size (300k files)                   | ⚠️ **89 MB** (→ ~296 MB @ 1M files); materialize incl. parse = 3.23s. Confirms the inlined index must be **sharded**.                                                                     |
| **C1** | Crash mid-publish (kill -9 during 2 GB publish)   | ✅ **atomicity holds**: 0 manifests (written last), 0 leaked `.tmp` (write-then-rename), store usable after crash — fresh publish + materialize round-trips identically.                  |

**Still open as design items (structural, for the real v1 daemon — not quick fixes):**

- **G3 manifest index sharding** — split the chunk index into CAS objects (Modal/Xet
  pattern) so a cold node fetches only the shards for the files it touches.
- **C6 streaming publish** — stream chunks into blocks instead of buffering the whole
  delta; bound publish RAM well below tree size.

## Independent audit (V1) + hardening

A skeptical independent audit (issue #744) re-ran everything and returned
**PASS-WITH-CONCERNS**: findings honest, git history clean, headline results reproduced
exactly — but it caught **1 weak test and 5 real in-scope gaps the suite had missed**. All
addressed (tests now 15/15):

| Audit finding                                                                                       | Fix                                                                                                                                         |
| --------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `corrupt_block_detected` only tripped the zstd decoder, not the sha256==id check that G5 advertised | Added **`chunk_integrity_catches_content_swap`** — repoints a chunk at a _valid_ wrong block; caught by content hash. G5 wording corrected. |
| D1 was prose-only despite a `test(...)` commit                                                      | Added **`d1_fixed_block_shift_sensitivity`** (append→1, prepend→≥40).                                                                       |
| **Manifest not integrity-checked on read**                                                          | materialize now verifies `logical_digest == requested` (`manifest_tamper_wrong_digest_refused`).                                            |
| **Tampered manifest → content/mode swap incl. setuid**                                              | logical-digest identity + per-chunk integrity + **mode masking** (`& 0o777`, drop setuid/setgid/sticky).                                    |
| **Symlink targets = write-through escape**                                                          | materialize now writes **files first, symlinks last** (`symlink_no_write_through`).                                                         |
| **Manifest digest entangled with zstd output** (breaks cross-node identity)                         | identity is now a **logical digest** over files+chunk-plaintext-ids, excluding block layout (`logical_digest_ignores_block_layout`).        |
| `put_manifest` non-atomic                                                                           | write-then-rename (matches `put_block`).                                                                                                    |

**Suite is now 15 tests, all passing.** The audit's value is exactly this: it found real
security-relevant gaps (untrusted-manifest handling) sitting inside the stated threat model.

## Adversarial round (V2) + fixes

A senior-dev adversarial pass (`tests/adversarial2.rs`) found **9 issues the audit and the
single-publish EC2 benchmarks were structurally blind to** — its sharp meta-point: single
publishes can't see cross-generation growth or concurrency. Triaged; the security/correctness
ones fixed (tests flipped to assert the fix), fidelity gaps documented.

| #   | Finding                                                                                                                                                                 | Severity                               | Disposition                                                                                                                                                                                                                                                                                                                                                                                                                     |
| --- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| P1  | **Sparse/large-file OOM DoS** — `std::fs::read` whole-file × parallel; sparse files bypass XFS quota (bounds _allocated_, not _apparent_). 96 KiB disk → **6.5 GB RAM** | **HIGH** (tenant-reachable, node-wide) | ✅ **FIXED** — streaming publish (chunk-at-a-time). Peak RSS is now bounded **independent of apparent/per-file size**: 3 GiB-apparent sparse → **7 MB** (V3-reproduced 6.98 MB); distinct data flat at **~132 MB** from 64 MiB→1 GiB. (Not literally "one block": ~2× block-target + O(chunk-count) index metadata — same O(tree) metadata as G3, not the DoS.) Trade-off: publish is now single-threaded (see Rust-core note). |
| P3  | **Cumulative manifest index** — every manifest embedded all of `known`, growing with lineage churn (the R2 cold-start object)                                           | MED-HIGH                               | ✅ **FIXED** — index carries only referenced chunks (bounded to live tree).                                                                                                                                                                                                                                                                                                                                                     |
| P6  | `put_block` fixed temp name → concurrent identical writes spuriously fail (~7/8)                                                                                        | MED-HIGH                               | ✅ **FIXED** — per-writer unique temp; 0 spurious errors.                                                                                                                                                                                                                                                                                                                                                                       |
| P5  | Write-through a **pre-existing symlink** in a non-empty `out` escapes the workspace                                                                                     | MEDIUM                                 | ✅ **FIXED** — materialize refuses a non-empty `out`.                                                                                                                                                                                                                                                                                                                                                                           |
| P7  | materialize trusts manifest `size` for allocation (u64::MAX → crash)                                                                                                    | LOW-MED                                | ✅ **FIXED** — size validated against actual chunk bytes; clean error.                                                                                                                                                                                                                                                                                                                                                          |
| P2  | **Hardlinks** flattened to N copies (pnpm/npm/cargo) — inflates materialized size vs R2 budget                                                                          | MEDIUM                                 | 📋 documented design gap (needs inode tracking + hardlink entry type)                                                                                                                                                                                                                                                                                                                                                           |
| P8  | Non-UTF8 paths lossy (`to_string_lossy`) → collision/data-loss on Linux                                                                                                 | MED (fidelity)                         | 📋 documented (needs byte/OsString paths through the manifest)                                                                                                                                                                                                                                                                                                                                                                  |
| P1b | Empty directories dropped                                                                                                                                               | LOW-MED                                | 📋 documented (needs explicit dir entries)                                                                                                                                                                                                                                                                                                                                                                                      |
| P4  | **No GC** → orphan blocks + superseded manifests accumulate (**10× on-disk after 10 gens**)                                                                             | MED-HIGH (design)                      | 📋 spec §13 defers GC to phase 3; rate now quantified                                                                                                                                                                                                                                                                                                                                                                           |

Confirmed **FINE** (checked, correct): empty files, exact chunk-boundary files (256K/512K/±1),
symlink loops (preserved not followed), missing-block → clean error.

**Suite: 25 tests (15 + 10), all passing.** The adversarial round's highest-value contribution
was methodological — it showed the EC2 numbers needed cross-generation and concurrency probes,
and the two it flagged for a deployment gate (sparse-OOM, cumulative index) are both now fixed
and re-measured.

## Final review (V3) + corrections

An independent final reviewer verified the 5 V2 fixes by **code-reading + its own probes +
independent RSS measurement** (not trusting the author's tests). Verdict:
**PASS-WITH-CONCERNS** — all 5 fixes **genuinely correct, no correctness regression**;
concerns were **disclosure/precision in this log**, now corrected:

- Independently reproduced the **7 MB sparse RSS** (measured 6.98 MB) and extended it: distinct
  data is flat **~132 MB across 64 MiB→1 GiB** (16× size range) — memory bounded independent of
  apparent/per-file size, as claimed.
- Independently verified the **referenced-only index materializes across generations** (3-gen
  inheritance round-trips) — a path the author's test hadn't covered; now added as
  `referenced_index_inheritance_materializes`.
- Confirmed put_block race-freedom, non-empty-`out` rejection, and size-validation are correct
  and don't over-reject legitimate inputs; confirmed the documented gaps are honestly labeled.
- **Corrected here:** the Rust-core publish throughput table (was stale — pre-V2 parallel;
  publish is now single-threaded streaming ~276 MB/s), the memory-bound wording (~2 blocks +
  O(chunk-count) metadata, not "one block"), and the "no warnings" claim (1 warning, now fixed).

**Final state: 26 tests passing, clean build (no warnings), three review layers (audit →
adversarial → final) all reconciled.**

## Bottom line

Every load-bearing assumption in the spec that could be tested on one node **held**:
content-addressed blocks collapse 100k files into ~16 S3 GETs; the publish barrier is O(1);
incremental publish ships only the delta; the git fast-tier is ~0.6s; gVisor boot (~1.7s),
not storage, dominates the start budget. Real compression is ~2.9×. The one nuance the
experiments _added_ is that strong dedup is temporal (same project over time) — which is
precisely the workload. Remaining unmeasured pieces need the built control plane and a
16-vCPU quota: real pod scheduling latency, Karpenter cold-node launch, and the Rust CAS's
true chunk/materialize throughput (the Python prototype's wall times are not representative).
