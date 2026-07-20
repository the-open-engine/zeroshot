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
