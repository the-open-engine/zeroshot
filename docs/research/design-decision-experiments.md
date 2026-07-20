# Design-decision experiments (round 2)

Resolving the open design decisions extracted from the spec + prototype + 3 review layers.
Same discipline as [`workspace-stack-experiments.md`](workspace-stack-experiments.md):
real measurements, notes per finding, push after each, then audit → adversarial → review.
Tracker: issue the-open-engine/zeroshot#744. Prototype: `capsule-workspace-core/`.

Legend: ✅ done · 🔄 running · ⬜ planned · ⚠️ finding.

| ID  | Decision                                                             | Experiment                                                                                        | Status |
| --- | -------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- | ------ |
| E6  | Publish concurrency: bounded-memory streaming vs parallel throughput | bounded-parallel pipeline: throughput vs peak-RSS curve                                           | 🔄     |
| E7  | Manifest index: monolithic vs sharded                                | sharded index; cold-start time-to-first-file at 100k/500k/1M                                      | ⬜     |
| E8  | Garbage collection strategy                                          | grace-period mark-sweep; growth tracks live set; no live chunk collected under concurrent publish | ⬜     |
| E9  | Chunking: fixed-256K vs CDC                                          | FastCDC vs fixed dedup across a real repo's git history                                           | ⬜     |
| E10 | Warm cache: node affinity vs statistical                             | shared-LRU simulation under aggressive reap; warm-hit rate                                        | ⬜     |
| E11 | Hardlinks: flatten vs inode-track                                    | real pnpm/cargo density + blowup; implement + verify preservation                                 | ⬜     |
| E12 | Path encoding: UTF-8 String vs bytes                                 | byte-path variant; non-UTF8 round-trip on Linux                                                   | ⬜     |
| E13 | Cross-tenant dedup scope: per-org vs global                          | measure dedup forgone by per-org scoping on real multi-project corpus                             | ⬜     |

## Results

### E6 — Publish concurrency: bounded-parallel pipeline ✅

Implemented `publish_pipelined` (producer streams+dedups → bounded channels → N compressor
threads → 1 sequential packer). Identity-equivalent to streaming (test `pipelined_equivalent_to_streaming`:
same logical digest at w=1/2/4/8, round-trips — proves the logical digest is packing-order-independent).

1 GB distinct incompressible tree, 18-core Mac:

| Mode            | Throughput   | Peak RSS |
| --------------- | ------------ | -------- |
| streaming (w=0) | 259 MB/s     | 137 MB   |
| pipeline w=2    | 294 MB/s     | 152 MB   |
| pipeline w=4    | 314 MB/s     | 166 MB   |
| pipeline w=8    | **354 MB/s** | 209 MB   |

**Finding (not what I'd have guessed): the pipeline reclaims only ~1.37×, not ~N×** — once
compression is offloaded, the **serial producer (read + sha256 for the dedup decision) is the
bottleneck**, and the curve flattens (2→4→8 is sublinear). Memory stays bounded (~140–210 MB
for a 1 GB tree; channel-bounded, so the same for 10 GB modulo the O(chunk-count) index).

**Decision:** ship the bounded-parallel pipeline (modest speedup + bounded RAM), but **don't
chase more parallelism by default.** At ~350 MB/s publish is already ≈ the S3-upload rate
(measured 350 MB/s cold / 1.09 GB/s parallel), and the common case is _incremental_ publish
(tiny delta). Full parallel-hashing (dedup-in-packer, at the cost of wasted compression on
dups + more in-flight RAM) is a **metric-gated** follow-up, only if cold first-publish CPU
proves to matter. **Senior default: bounded-parallel pipeline, workers ≈ cores, cap ≈ 8.**

### E7 — Manifest index: monolithic vs sharded ✅

Two things measured: (1) monolithic download budget at scale, (2) incremental shard-**reuse**
by sharding scheme × edit locality (50/5000 files changed).

- **Monolithic is cheap at realistic scale:** real manifest **89 MB @ 300k files** → **0.25s**
  download @350MB/s (1M files ~0.85s). **A cold FULL materialize needs the whole index
  regardless**, so sharding buys ~nothing for cold-full — only for warm/incremental.
- **Incremental shard reuse** (what a warm node with gen N cached must re-fetch for gen N+1):

  | Edit                                   | prefix/content-hash (256) | locality / contiguous-file (64) |
  | -------------------------------------- | ------------------------- | ------------------------------- |
  | **Clustered** (realistic: one dir/dep) | 66%                       | **97%**                         |
  | Random                                 | 68%                       | 46%                             |

**Finding:** the naive sharding choice (prefix/content-hash) is a trap — it's edit-agnostic
(~66%) and _underperforms on the realistic clustered edit_, where **locality sharding reaches
97% reuse**. But locality sharding is _worse_ than prefix on random edits.

**Decision:** **monolithic index for v1** — at realistic file counts (≤300k) it's a
sub-second download and cold-full needs it all anyway. Add **locality-based sharding
(contiguous file ranges, following tree order) ONLY if** (a) file counts routinely exceed
~500k–1M, or (b) warm incremental-materialize latency becomes the bottleneck. Do **not** reach
for content-hash/prefix sharding — it doesn't help the case that matters. G3 is real only at
extreme scale.

### E8 — Garbage collection: grace-period mark-sweep ✅ (highest-risk subsystem)

Implemented `gc::collect(store, live_manifests, grace)` — mark referenced blocks from all live
manifests, sweep orphans **only if older than `grace`**. Three tests, all pass:

| Test                                                                              | Result                                                                                                                           |
| --------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| Reclaims orphans (10 gens full churn, live=[HEAD], grace=0)                       | store **41.9 MB → 4.19 MB = exactly the live set** (9 orphan blocks + 9 superseded manifests collected); HEAD still materializes |
| **Grace protects in-flight publish** (blocks written, manifest not yet committed) | grace=1h → **0 deleted** (young orphan protected), publish then commits + materializes; grace=0 → **1 deleted** (would corrupt)  |
| Live manifest's blocks never collected (even grace=0)                             | referenced blocks kept, materializes intact                                                                                      |

**Finding:** the **grace period is the entire safety mechanism** — proven by the contrast
(grace=1h safe, grace=0 corrupts the in-flight publish). A publish writes blocks _before_
committing the manifest that references them, so those blocks are momentarily orphan-but-young;
grace must exceed the longest possible publish, or a concurrent GC deletes a block the
about-to-commit manifest needs.

**Decision (senior default):** grace-period mark-sweep, **not** ref-counting (fragile under
concurrent publishers). **grace ≫ max publish duration** (e.g. 1h against seconds-scale
publishes / the 6h execution deadline). Mark set = the lineage HEAD **plus all retained history
within the 7-day window** (§13) — miss one and you collect its blocks. GC reclaims to exactly
the live set. This is the subsystem to test hardest in the real build (restic/kopia shipped GC
bugs here for years); the grace-period invariant above is the thing to never violate.

### E9 — Chunking: fixed-256K vs FastCDC on real git history ✅

Added `fastcdc` + a `cdcbench` bin; measured cumulative dedup across **80 real versions of
flask's source** (146 MB raw material) with fixed vs content-defined chunking:

| Chunk size     | FIXED unique | CDC unique | CDC saves vs fixed |
| -------------- | ------------ | ---------- | ------------------ |
| 256 KiB (spec) | 10.9 MB      | 10.9 MB    | **0.0%**           |
| 64 KiB (Xet)   | 10.7 MB      | 9.3 MB     | **13.0%**          |

**Finding — the CDC benefit is entirely chunk-size-dependent.** At the spec's **256K, CDC
gives literally 0%** on real source: files are ≤1 chunk, so there are no internal boundaries
for an insertion to shift, and both methods dedup identically (cross-version dedup comes from
_unchanged files_, which both handle). CDC's 13% only appears at **64K**, where source files
span multiple chunks and insertions shift fixed boundaries (the D1 mechanism) — but 64K also
means ~4× more chunks → more index/manifest metadata (the G3/E7 cost).

**Decision (senior default): fixed-256K, no CDC** — confirmed on real data, not just D1's
synthetic case. CDC earns its complexity only if you (a) drop to ≤64K chunks for finer dedup
_and_ (b) have large insertion-edited files — and the workspace's big bytes are the
dependency plane (large files replaced wholesale, not insertion-edited) while source lives in
the git plane. The CDC-vs-fixed and chunk-size decisions are coupled: at 256K the question is
moot.

### E10 — Warm cache: statistical shared vs node-affinity ✅

Simulated 8 nodes (2 GB LRU cache each), 20 projects with shared common-dep chunks, zipf
popularity (the "one project for weeks" reality), 3000 runs:

| Placement                                                  | Warm-hit rate |
| ---------------------------------------------------------- | ------------- |
| random (shared cache, no affinity)                         | 58.1%         |
| round-robin                                                | 57.5%         |
| **soft affinity** (prefer node that last ran this lineage) | **99.4%**     |
| pinned (hard affinity)                                     | 99.4%         |

**Finding:** **soft affinity matches hard pinning (99.4%) while staying a scheduling
_preference_ that preserves R4** (any node/AZ). A pure statistical shared cache (random
placement) gets only **58%** — the shared common-dep pool accumulates in every node's cache,
but each project's _private_ working set (half the bytes) misses unless the run lands on a
warm node, which only affinity ensures.

**Decision (senior default): soft affinity** — the operator hints the scheduler to prefer the
node that most recently materialized this lineage, as a soft preference (never a hard
constraint, or R4 breaks). This **recovers the warm benefit the earlier "traded away for R5
economy" note gave up** — but with a caveat that ties it to the reap timer: affinity only
helps if the target node is still alive between a project's runs. An **active** project (frequent
runs) keeps its node from idle-reaping → warm; a **sparse** project (touched once a day) gets
its node reaped → cold fallback (58%). So the real knob is **soft affinity + a reap timer
tuned so active projects stay warm** — not a binary "affinity vs economy" trade.
