# Spec: Capsule workspace storage and transfer

**Status:** Draft for review · **Date:** 2026-07-20 · **Scope:** AWS only (us-east-1), EKS
**Intended home:** `zeroshot-cloud/planning/spec/` — companion to `capsule-sandbox-platform.md`
**Research basis:** [`k8s-workspace-file-transfer.md`](../research/k8s-workspace-file-transfer.md)

Supersedes the workspace-storage portions of `capsule-sandbox-platform.md` §6a and the
`WS05` PVC consumption model. It does not change identity, billing, eventing, or the
data-plane protocol.

---

## 1. Requirements

Fixed by the product owner. Not negotiable; where two conflict, §3 states the resolution.

| #      | Requirement                                                                                                                                                  |
| ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **R1** | Capsule work must survive **both pod death and node death**.                                                                                                 |
| **R2** | Capsule cold/warm start must be very fast: **<5s on resume**, excluding the initial git clone.                                                               |
| **R3** | **No assumptions about project structure.** No lockfiles, no known directories, no language/toolchain/ecosystem heuristics. The workspace is an opaque tree. |
| **R4** | **Fully flexible node placement** for pods of the same logical Zeroshot run. No node pinning, no AZ pinning.                                                 |
| **R5** | **Economical.** Pods disappear quickly when idle or finished; nodes scale down aggressively.                                                                 |

Confirmed product decisions (2026-07-20):

- **Process/memory state is NOT preserved** across pod death. Snapshots are files-only; a
  respawned pod restores the filesystem and restarts processes. The gVisor-on-EC2 runtime
  is retained; Firecracker/microVM memory snapshots are out of scope.
- **Typical materialized workspace: 1–5 GB.**
- **Nothing is deployed to AWS yet** — the capsule infrastructure exists only as code.
  Every change in §11 is therefore a text edit, not a migration.
- **Snapshot retention: run lifetime + ~7 days**, then garbage collected.

### Standing context

Single writer per workspace lineage at a time, with git reconciliation; parallel readers
are read-only. Users work on one project over weeks. gVisor (`runsc --platform=systrap`)
on Karpenter-managed EC2. EFS/FSx rejected up front (attach and small-file latency).

---

## 2. The core invariant

> **Node-local storage is a cache. It is never authoritative.**

This is the single load-bearing decision; everything else follows. The justification is a
trilemma between R1 and R4:

If the authoritative copy of a workspace lives on a node's local disk, then either

- the pod must always be scheduled back onto that node — **R4 fails**; or
- when that node dies, the work is gone — **R1 fails**.

Network-attached block storage (EBS) escapes only half of it: it survives node death but
is **AZ-bound**, so R4 still fails, and its attach latency has an unbounded tail
(Kubernetes force-detaches only after 6 minutes), which fights R2.

Therefore the system of record must be **regional object storage**, and node-local NVMe
becomes pure cache and scratch.

---

## 3. Requirement conflicts and their resolutions

| Conflict                                                           | Resolution                                                                                                                                  | Cost                                             |
| ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------ |
| R1 vs node-local NVMe (instance store is wiped on node loss)       | S3 is the system of record; NVMe is cache only                                                                                              | Every durable write crosses the network          |
| R4 vs volume topology (local volumes pin the pod; EBS pins the AZ) | **No topology-bearing volume may exist before the scheduler has chosen a node.** The workspace is materialized _after_ placement            | The pod cannot declare its workspace as a PVC    |
| R2 vs R1+R4 (truth is remote, node may be cold)                    | Content-addressed dedup + node cache + lazy materialization                                                                                 | Cold start on a large workspace degrades; see §9 |
| R1 read as RPO=0                                                   | **Not satisfiable.** A per-write durable barrier costs ~200× local NVMe — roughly 21 minutes of pure durability tax on a 50,000-write build | Tiered RPO instead; see §8                       |
| R3 vs selective caching                                            | No semantic classification anywhere; whole opaque tree, fixed-size chunks                                                                   | No "cache only dependencies" shortcut            |

**R1 and R2 are delivered weaker than literally written.** §8 and §9 state exactly what is
delivered. This requires explicit sign-off.

---

## 4. Architecture

```
   ┌────────────────────────────────────────────────────────┐
   │  REGIONAL S3 (3-AZ)  —  SYSTEM OF RECORD               │
   │  • content-addressed chunks (immutable)                │
   │  • one manifest per barrier (the commit point)         │
   │  • per-lineage access-order trace (prefetch hint)      │
   └────────────────────────────────────────────────────────┘
                    ▲ publish            │ materialize
                    │                    ▼
   ┌────────────────────────────────────────────────────────┐
   │  NODE STORAGE DAEMON (DaemonSet, one per node)         │
   │  • sole holder of S3 credentials on the node           │
   │  • outlives every pod it serves                        │
   │  • owns the local chunk cache + LRU eviction           │
   └────────────────────────────────────────────────────────┘
                    ▲                    │
   ┌────────────────────────────────────────────────────────┐
   │  NODE-LOCAL NVMe  —  CACHE + SCRATCH (never truth)     │
   │  • chunk cache, shared across capsules on the node     │
   │  • live writable workspace tree                        │
   └────────────────────────────────────────────────────────┘
                                         │ bind mount
   ┌────────────────────────────────────────────────────────┐
   │  CAPSULE POD (gVisor)                                  │
   │  • capsule-agent sidecar   • tenant runtime            │
   │  • NO workspace PVC, NO agent-state PVC                │
   └────────────────────────────────────────────────────────┘
```

**Git remains the system of record for committed content.** S3 carries only the opaque
uncommitted tree. The two planes together define a workspace state.

### 4.1 Why a node-level DaemonSet and not a sidecar

A per-pod sidecar is not merely suboptimal — it is **prohibited three independent ways by
contracts already in the repo**, all verified 2026-07-20:

1. `backend/crates/capsule-agent/src/pod_contract.rs` enforces
   `!has_target(agent, WORKSPACE_MOUNT)` and returns `ContractBypass` otherwise. The
   trusted sidecar is contractually forbidden from mounting `/workspace`.
2. `iac/manifests/bootstrap/cilium/cilium-values.yaml:17` sets
   `policyEnforcementMode: always` — cluster-wide default-deny.
3. `iac/manifests/capsule-substrate/api-egress.yaml` grants only the kube-apiserver entity
   to `topolvm-system` and `capsule-system`, explicitly _"no DNS, CIDR, or external
   egress"_, and grants the `capsules` namespace nothing. **Tenant pods have no egress at
   all.**

Independently, a sidecar dies with its pod and so cannot perform the one flush that
matters. The node daemon survives the pod and completes the final publish afterwards —
which is what makes RPO=0 on pod death structural rather than best-effort (§8).

---

## 5. Data model

All formats are opaque to project structure (R3).

### 5.1 Chunks

- **Fixed 256 KiB** content-addressed chunks. Fixed size, not content-defined chunking.
- 256 KiB is chosen to equal the LVM thin pool's `--chunksize 256K`
  (`iac/runtime/capsule-storage-node/initialize.sh:81`), so block-level deltas map 1:1 onto
  chunk identities with no re-chunking.
- Each chunk is compressed (zstd) and addressed by SHA-256 of its plaintext.
- Chunks are immutable. Identical content has identical addresses, so concurrent writers
  cannot corrupt one another.

> **Content-defined chunking (FastCDC) is deliberately NOT used in v1.** It earns its
> complexity only for large files mutated internally. Revisit only if §14's measurement
> shows that regime dominates.

### 5.2 Manifest

One manifest object per barrier. Contains the file tree — path, mode, symlink/exec bits,
size, and the ordered chunk list per file — plus the parent manifest digest.

**The manifest PUT is the commit point.** Chunks are durable before the manifest that
references them is written. A torn write yields an unreferenced chunk (garbage, collected
later), never a manifest pointing at absent data.

### 5.3 Lineage

A **lineage** is the mutable pointer to the newest manifest for one workspace. It lives in
regional Postgres, guarded by compare-and-swap on the monotonic fence that already exists
in `backend/services/orchestrator/src/capsule_control.rs`.

Advancing the lineage HEAD is the only mutating operation requiring coordination.

### 5.4 Receipts

A published snapshot is identified by its manifest digest and surfaced as an OECP
`ArtifactRef`-shaped receipt (sha256 + byteLength). This is what one agent passes to
another over the message bus — bytes and URLs stay external, exactly as
`docs/openengine-cluster-protocol/v1/legacy-worker.md` already specifies. **No protocol
change is required.**

### 5.5 Access-order trace

Per lineage, the daemon records the order in which files were first read during previous
materializations. This is a prefetch hint only — never a correctness input, and safely
absent on the first ever run of a lineage.

---

## 6. Components

| Component               | Status            | Responsibility                                                                                             |
| ----------------------- | ----------------- | ---------------------------------------------------------------------------------------------------------- |
| **Node storage daemon** | **NEW**           | Chunk cache + LRU, materialize, publish, S3 credentials, quota enforcement                                 |
| `capsule-agent` sidecar | exists            | Signals publish barriers at task boundaries; owns instruction/event state. **Never touches `/workspace`.** |
| `capsule-operator`      | not built         | Creates pods without topology-bearing volumes; records lineage bindings                                    |
| Control-plane ledger    | exists (Postgres) | Authoritative instruction/event record and lineage HEAD                                                    |

### 6.1 Node storage daemon

Runs as a DaemonSet on `capsule-nvme` nodes, `hostNetwork`, outside gVisor, unreachable
from tenants.

- **Materialize**: given a manifest digest, construct the workspace tree on local NVMe —
  reflink/hardlink from the local chunk cache on hit, ranged GET from S3 on miss — and
  bind-mount it into the pod path.
- **Publish**: on a barrier, freeze via LVM thin snapshot, walk the delta, upload new
  chunks, then write the manifest, then CAS the lineage HEAD.
- **Cache**: per-org-namespaced chunk cache with LRU eviction; entries pinned while a
  materialization references them.

**It must never execute anything from a workspace, and must treat every tree as hostile**
(symlink traversal, hardlink bombs, pathological depth, adversarial filenames).

---

## 7. Lifecycle

There is **one lifecycle** with two spawn/reap policies. Interactive sessions are a policy
on top of the task-pod model, not a separate architecture.

|               | Task pod                     | Interactive                                    |
| ------------- | ---------------------------- | ---------------------------------------------- |
| Spawn trigger | graph node starts            | user interacts                                 |
| Reap trigger  | task completes               | idle timeout                                   |
| On death      | re-run the task              | respawn, materialize latest snapshot, continue |
| Publish       | continuously + at completion | continuously                                   |

Both use the same storage, publish, and materialize paths.

### 7.1 The publish barrier

1. `capsule-agent` signals a barrier (task completion, or the periodic timer).
2. Daemon freezes the tree (LVM thin snapshot — O(1), independent of file count).
3. Daemon uploads new chunks, then the manifest, then CAS-advances the lineage HEAD.
4. Only then is the task reported complete.

**A task is not "done" until its snapshot is durable.** Because the daemon outlives the
pod, the pod may be reaped before step 4 finishes — the daemon completes it.

### 7.2 Reader fan-out

Reviewer/validator pods are handed a manifest **digest** and materialize it read-only.
Because the snapshot is immutable, this needs no locking, no coordination, and no live
writer. A reviewer sees byte-exactly what the worker published — strictly stronger than a
shared filesystem, where the writer could still be mutating.

---

## 8. Durability and failure semantics

**R1 is delivered as tiered RPO. RPO=0 is not achievable at a tolerable price** — a
per-write durable barrier is ~200× local NVMe latency, roughly 21 minutes of overhead on a
50,000-write build.

| Failure                                             | Work lost                                         | Why                                                                                       |
| --------------------------------------------------- | ------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| Tenant process crash, pod lives                     | **0**                                             | Tree intact on local NVMe                                                                 |
| **Pod death** (OOM, crash, eviction)                | **0**                                             | Daemon outlives the pod and completes the barrier                                         |
| **Planned node termination** (drain, consolidation) | **0**                                             | `preStop` flush inside the grace period; the dominant node-loss case on an on-demand pool |
| **Unplanned hard node loss**                        | **≤ barrier interval** (target 5s p50; 5–15s p99) | Only unpublished delta is lost                                                            |
| AZ loss                                             | **0**                                             | S3 is regional (3-AZ)                                                                     |
| Mid-task loss of _in-flight_ work                   | task is **re-run**                                | Zeroshot already retries from recorded input                                              |

Two asymmetries make this cheap:

- **Committed work has RPO 0 always** — a git commit is not acknowledged until its
  packfile delta is durable.
- **In-flight work is a cost concern, not a correctness one.** Losing a partial task means
  re-running it. Periodic barriers exist to avoid re-spending API credits, not to protect
  correctness.

**Backlog bound.** During sustained multi-GB regeneration the barrier can fall behind,
making RPO formally unbounded. Bound it with cgroup v2 `io.max` throttling of the writer
once backlog exceeds ~2 GB or ~30s of estimated drain. This converts an unbounded RPO into
a bounded one, purchased with write throughput — a deliberate, tunable choice.

---

## 9. Performance budget

**Partially validated** (2026-07-20 — see §14 and
[`workspace-stack-experiments.md`](../research/workspace-stack-experiments.md)): host
materialization, S3 throughput, block-fetch count, O(1) freeze, gVisor boot, and git tier
are measured; pod scheduling and Karpenter cold-node launch remain estimates. Sized for the
confirmed **1–5 GB** workspace range. **Materialization uses hardlink-from-cache (measured
1.6s/100k files on the host), not reflink (3.75s)** for read-only fan-out.

> **Node pool correction.** `capsule-nvme` provisions **c6id/m6id/r6id**, not i4i/i3en.
> A c6id.4xlarge has ~950 GB NVMe behind an "up to 12.5 Gbps" NIC (~1.5 GB/s burst,
> materially lower sustained). **The NIC is the binding constraint, not S3.** Figures of
> 2,800 MB/s or ~11 GB/s via S3 CRT do not apply to this pool.

| Phase                 | Warm node           | Cold node                              |
| --------------------- | ------------------- | -------------------------------------- |
| Pod scheduling        | ~0.3–0.5s           | ~0.3–0.5s                              |
| gVisor sandbox boot   | ~0.5–1.5s           | ~0.5–1.5s                              |
| Workspace materialize | ~0.1–0.3s (reflink) | ~0.4–0.8s to mounted (manifest + lazy) |
| Working set resident  | —                   | +1–2s (2 GB), +2–4s (5 GB)             |
| **Total to usable**   | **~1–2.5s** ✅      | **~1.5–3.5s** ✅                       |

**Where <5s does NOT hold** — these must be written into the requirement:

- **Workspaces materially above 5 GB** — degrades past the budget.
- **First-ever materialization of a lineage** (5–17s): no access-order trace exists yet,
  so prefetch is blind.
- **When Karpenter must launch a node** (45–70s): a capacity problem, not a storage one.
  Mitigated only by paying for warm headroom (~$1.8k/mo for 1-per-AZ).

**The most important finding:** in the warm path, storage is only ~4–6% of the budget,
while scheduling plus gVisor boot is ~70%. **Further optimization of the storage layer is
wasted engineering.** The highest-leverage work is elsewhere — pin instance size, bake
capsule images into the AMI, and tighten readiness poll loops. Those total well under 50
lines of configuration and are worth more than any storage-layer choice in this spec.

**Block-level, not file-level, transfer.** Coalesced ranged GETs are bandwidth-bound
(~2.6s for a representative set) where ~20,000 individual file GETs are latency-bound
(~5.9s). **S3 Express One Zone is rejected**: it buys latency this path does not need and
is single-AZ, which would break R1.

---

## 10. Concurrency, consistency, security

### 10.1 Single writer without physical fencing

The current design fences physically: prove the old runtime is dead before reattaching a
disk, because two writers on one block device corrupt it. **The snapshot model replaces
this with a logical guarantee:**

- Chunks are immutable and content-addressed — concurrent writers cannot corrupt data.
- Only one writer may advance the lineage HEAD, enforced by CAS on the monotonic fence.
- A second writer's publish is **rejected**, not corrupting.

Consequences: **no old-runtime fencing, no volume reattach, no recovery generations, no
AZ-locality constraint.** This deletes a substantial amount of currently-specced machinery
(§11).

### 10.2 Multi-tenancy

- The chunk cache and chunk namespace are **scoped per organization**. Cross-org dedup is
  deliberately forfeited: a shared content-addressed cache is an existence oracle for
  other tenants' content. The stated workload (one project over weeks) is intra-org, so
  little is lost.
- Encryption via the existing per-cell SSE-KMS bucket configuration.
- The daemon holds the only S3 credential on the node, outside gVisor, unreachable by
  tenants (§4.1).

### 10.3 Size bounds without project knowledge

XFS project quotas (`iac/runtime/capsule-storage-node/quota.sh`, already built and tested)
bound each workspace on the node. No knowledge of what _should_ be in the tree is needed.

---

## 11. Changes to existing zeroshot-cloud code

Because **nothing is deployed**, these are text edits, not migrations.

### 11.1 Remove

- **The workspace PVC.** The workspace is a daemon-materialized bind mount, not a claim.
  This removes the `capsule-local-thin` base-PVC path for workspaces entirely.
- **`iac/manifests/capsule-substrate/storage-writers.yaml:112`** — the admission rule
  requiring a `zeroshot.dev/node-name` label on `capsule-local-thin` PVCs at creation
  time. **This rule structurally violates R4**: it forces the operator to choose a node
  before the PVC exists, bypassing the scheduler. Verified 2026-07-20.
- **The agent-state PVC.** A TopoLVM-backed agent-state claim pins the pod to a node,
  violating R4. Instruction/event state becomes ephemeral pod-local scratch; the
  **authoritative record is the control-plane ledger** (consistent with §2 — node-local is
  never authoritative). _This changes the WS05 contract and needs review._
- **Old-runtime fencing, volume reattach, recovery generations, same-AZ reattachment**
  (`capsule-sandbox-platform.md` §6a) — superseded by §10.1.
- **The snapshot/cleanup-drain contract** (`snapshot-contract.yaml`, `300s-or-drain`) —
  LVM thin snapshots become a short-lived internal implementation detail of the publish
  barrier, not a managed Kubernetes resource.

### 11.2 Change

- **`iac/modules/eks-cluster/karpenter-substrate/main.tf:75`** — `consolidateAfter = "5m"`
  with `WhenEmptyOrUnderutilized` terminates the node holding the warm cache 5 minutes
  after it goes idle, and instance store is wiped with it. Under R5 (economy) this is a
  **deliberate trade**, not a bug: cross-run warmth then comes from content dedup against
  S3 plus a statistically warm shared pool, not from per-project node affinity. Raise it
  only if measurement shows the cold path dominates cost.
- **`pod_contract.rs`** — the pod shape changes (no workspace/agent-state claims; a
  host-propagated workspace mount). The invariant that the agent never mounts `/workspace`
  is **retained and still enforced**.
- **Node affinity for cache locality is a SOFT preference only** — never a hard
  constraint, or R4 breaks.

### 11.3 Keep

- `capsule-nvme` Karpenter NodePool (instance-store NVMe hardware).
- The LVM thin pool + TopoLVM initializer — now serving the **daemon's** cache and scratch
  filesystem and providing O(1) freeze for the publish barrier, rather than per-capsule
  PVCs.
- XFS `pquota` (`quota.sh`) for per-workspace size bounds.
- The per-cell versioned SSE-KMS S3 bucket (`iac/modules/blob`).
- EBS gp3 + KMS — retained for backup/operational restore only, never the hot path.
- The monotonic fence in `capsule_control.rs` — now guarding the lineage HEAD.

---

## 12. Explicitly not doing

- Memory/process-state snapshots (would require Firecracker/microVM; gVisor is retained).
- Content-defined chunking (FastCDC) in v1 — fixed 256 KiB only.
- Operating a distributed storage system (Ceph/JuiceFS) — rejected.
- S3 Express One Zone (single-AZ, breaks R1).
- Shared POSIX filesystem (EFS/FSx) — rejected up front.
- Cross-organization dedup (existence oracle).
- Any project-structure introspection: no lockfile detection, no derived-directory lists,
  no toolchain detection (R3).
- EBS snapshots as a runtime checkpoint mechanism (backup/restore only).

---

## 13. Garbage collection and retention

- Intermediate snapshots live for the **run lifetime + ~7 days**, then are collected.
- Manifests are GC roots. A chunk is collectable when no live manifest references it
  **and** it is older than the longest in-flight publish (grace-period deletion — a
  mark-sweep over a live store with concurrent publishers corrupts data).
- Node cache eviction is LRU with pinning during materialization; prefer reflink so
  eviction cannot break a running pod's tree.
- The final delivered result (git commits / PR) is unaffected by snapshot GC.

---

## 14. Required benchmarks

Several are now **measured** (single `c6id.xlarge`, real S3, 2026-07-20; full log in
[`workspace-stack-experiments.md`](../research/workspace-stack-experiments.md)). Prototype
CAS is single-threaded Python, so its wall times are upper bounds; I/O rates, request
counts, and structural results are trustworthy.

1. **gVisor materialization overhead** — ✅ **MEASURED.** 100k-file host hardlink farm
   **1.61s** native vs **8.13s** in-gVisor (gofer) / **7.59s** (directfs) — 5× penalty;
   `--directfs` does not rescue it. Confirms: materialize on the **host**, use **hardlink**
   (1.6s) over reflink (3.75s) for read-only fan-out.
2. **c6id NIC throughput to S3** — ✅ **MEASURED.** Cold block fetch **~350 MB/s** (32
   parallel GETs); parallel block upload **1.09 GB/s**; small-object first-byte **26 ms p50
   / 63 ms p99** (S3 Express One Zone unnecessary → confirmed dropped). _Caveat:_ 4-vCPU
   box; 16-vCPU pool node is faster.
3. **Publish barrier** — ✅ **MEASURED (structural).** Materialize needs only **16 S3 GETs**
   for 102k files (chunks→blocks works); LVM thin snapshot freeze is **O(1)** (0.59s@1k vs
   0.66s@100k files); incremental publish ships **only the delta** (26 MB of a 1.86 GB
   tree). Real zstd **2.89×**. _Still needed:_ the Rust CAS's true chunk/delta-walk wall
   time (Python prototype: 21s — not representative).
4. **Warm-hit rate** under R5 reaping — ❌ **NOT measured** (needs the built cache + fleet).
   Nuance found: strong dedup is **temporal** (same lineage over time, ~97%), weak
   cross-install (~66%, due to `.pyc`/metadata non-determinism) — and the priority workload
   is the temporal case.
5. **Pod scheduling + gVisor boot floor** — ⚠️ **PARTIAL.** gVisor sandbox boot **~1.7s**
   (runsc) vs 1.3s (runc) via `docker run`. Full kube-scheduler + kubelet + Karpenter
   cold-node launch still needs the built stack + a 16-vCPU quota.

**Remaining, needing the built control plane:** real end-to-end pod start on EKS; Karpenter
cold-node launch (est. 45–70s); Rust CAS throughput; multi-node cross-node transfer under
concurrency.

---

## 15. Open items requiring sign-off

1. **R1 is tiered, not RPO=0** — "survive node death" is delivered as _nothing lost except
   ≤5s of uncommitted work on unplanned hard node loss_.
2. **R2 does not hold** for workspaces materially above 5 GB, first-ever lineage
   materialization, or when a node must be launched.
3. **Removing the agent-state PVC** changes the WS05 durability contract: instruction and
   event state becomes control-plane authoritative rather than pod-local durable.
4. **Warm cross-run caching is traded away** for R5 economy; cross-run benefit comes from
   dedup, not node affinity.

## 16. Phasing

1. **Foundations** — node daemon skeleton: chunk cache, materialize, publish, LRU. Fixed
   256 KiB chunks, manifests, lineage CAS. Run benchmark #1 in parallel (cheap, one day,
   gates everything).
2. **Lifecycle integration** — `capsule-agent` barrier signalling; operator creates pods
   with no topology-bearing volumes; reader fan-out by digest.
3. **Hardening** — hostile-tree traversal, grace-period GC, backpressure bound, per-org
   scoping, quota enforcement.
4. **Measure, then decide** — only if measurement demands it: content-defined chunking,
   an in-AZ cache tier, or warm headroom spend.
