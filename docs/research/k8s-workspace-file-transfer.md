# Workspace file transfer for Zeroshot clusters on Kubernetes — research & options

**Date:** 2026-07-19 · **Status:** Research / options for discussion · **Scope:** AWS only (us-east-1)

## TL;DR

- **The instinct is correct and industry-backed.** No verified platform uses a shared
  POSIX FS or per-workspace network block device as the durable workspace layer. Gitpod —
  the most experienced team here — documents rejecting per-workspace PVCs for _exactly_
  our reasons (unpredictable attach timing, attach-count limits, AZ pinning). The
  convergent pattern is **node-local NVMe for hot I/O + object storage for durability +
  a per-project warm cache**.
- **Recommended architecture (after a 4-engineer debate that converged in 2 rounds — §4.0):**
  **git carries source truth** (WIP commits, node-local bare mirror + `--reference`/blobless
  fetch — reconciliation stays pure git, matching the single-writer rule), and **a
  node-local NVMe cache doing warm reflink/hardlink restore is the load-bearing perf
  mechanism** regardless of what bulk engine sits behind it. A published state is an
  immutable **manifest** (an `ArtifactRef`-style receipt); readers materialize a frozen
  digest read-only, so immutability replaces distributed-FS coherence and a validator sees
  exactly the writer's published bytes.
- **Phased, and the phasing is the recommendation:**
  - **v0 (now):** git plane + node-local NVMe cache + lockfile-keyed tar.zst derived
    caches, **on the already-merged LVM-thin/TopoLVM/XFS-pquota substrate**; same-node
    writer→reader fork = **LVM-thin COW clone** (Ceph semantics without running Ceph).
    Instrument everything.
  - **v1:** cross-node distribution of the derived/base plane via **OCI images +
    ImageVolume (GA on EKS 1.36) + SOCI parallel-pull**, reusing the signed-OCI/Flux/ECR
    machinery the cloud repo already operates. An OCI digest _is_ the ArtifactRef receipt.
  - **v2:** the content-addressed **chunk store (Option B)** — built **only if a named v0
    metric** proves large incrementally-mutated trees (Rust `target/`, sccache) dominate.
- **The debate demoted the original recommendation.** "Commit to B (chunk store) as the
  v1 engine" — the doc's original §4 position — is now held by _no one_: on the stated
  cross-_run_ priority, B/C/D are indistinguishable (the node cache dominates), and
  immutable-manifest read-fork correctness is a property of _any_ content-addressed digest,
  not B's alone. **Ceph-on-NVMe (Option F) rejected; EBS snapshots (Option A) = backup only.**
- **Already built vs greenfield (§7):** the heavy _substrate_ the plan needs is **already
  merged** — instance-store NVMe Karpenter pool, LVM-thin/TopoLVM/pquota (dormant), EBS+KMS,
  S3, OCI/Flux/ECR, EKS 1.36/containerd 2.2.4. The _transfer logic_ (git plane, node cache
  daemon, materialize/publish, OCI-workspace packing) is **greenfield in both repos**, and
  the capsule control plane that hosts it is still in progress.
- **Waste risk of "v1 as stated → improve in v2" is LOW, by construction** — _if_ three
  seams are preserved (`WorkspaceLease` locator enum, `ArtifactStore` "inject external CAS"
  #699, `#669` ReadOnly|Exclusive token). v0/v1/v2 are all one content-addressed receipt
  model, so phases don't churn the abstraction. Two benchmarks gate v0/v1 (NVMe-vs-EBS;
  ImageVolume+gVisor). **The only real waste exposure is building B speculatively as v1 —
  which the converged plan explicitly defers to metric-gated v2.**

## 0. Problem statement

Run a Zeroshot cluster (conductor + workers + validators, i.e. many agent pods) on
Kubernetes (EKS) and decide how workspace files (repo checkout + untracked state:
`node_modules`, `target/`, build caches, uncommitted edits) move between nodes.

Constraints (from Michael, 2026-07-19):

- **No cross-pod/node persistent shared filesystem** (EFS/FSx-class): attach and
  read/write latency is too slow for us.
- Direction of interest: a **snapshot protocol with caching across cluster runs** —
  users generally work on one project over a long period, so warm-project restores
  dominate cold ones.
- **Never parallel write nodes without git reconciliation.** Parallel nodes are
  generally **read-only**; there is a single writer per workspace lineage at a time.
- AWS only.

## 1. Internal context (what already exists / is planned)

### 1.1 OECP — Open Engine Cluster Protocol (zeroshot#643, docs/openengine-cluster-protocol/v1)

- Wire contract `openengine.cluster/v1`; graph profiles `openengine.graph.full/v1` and
  `single-worker/v1`. JSON-RPC: `initialize, plan, apply, get, watch, update, stop,
retry, resubmit, delete, logs, agent/attach`.
- **Workspace creation/materialization is outside the protocol.** Durable outputs are
  hash-addressed `ArtifactRef` receipts; _bytes and signed URLs remain external_.
  Artifact resolvers materialize declared receipts as read-only content inside the
  allocated isolation (`docs/openengine-cluster-protocol/v1/legacy-worker.md:49`).
- Consequence: a k8s workspace-transfer design is a **product/runtime concern**, not a
  protocol change. It slots under the anticipated `RemoteExecutionRuntime` /
  `ExecutionTransport` seam without changing graph semantics.

### 1.2 zeroshot-rust epic (zeroshot#665 "Zeroshot 2: separate native Rust engine")

- v1 workspaces (#677): closed module, `WorkspaceMode = Borrowed | Worktree | Docker`,
  `WorkspaceLease { id, mode, locator, owner_token, state }`, ops `prepare / inspect /
cleanup`. Deterministic lease identity + owner token persisted in `ClusterLedger`
  before creation; recovery reattaches to the exact resource; fail closed otherwise.
- #669 owns the **`ReadOnly | Exclusive` workspace access token** — `Exclusive` is the
  default and conflicts with all other access; `ReadOnly` may coexist only when the
  compiled workflow proves that mode. This matches the single-writer/many-reader
  constraint exactly and is the natural admission point for snapshot-based read replicas.
- `ArtifactStore` (#699): product-local **hash-addressed filesystem CAS** behind
  `ArtifactRef`; "a future hosted product may inject an external CAS without changing
  engine or wire semantics." A workspace snapshot store is philosophically the same
  object: content-addressed bytes + manifest receipts.
- Source delivery (#680): all mutating git operations go through `SourceCodeProvider`
  with intent → operate → inspect → receipt recovery. Push/PR/merge are already
  first-class, which is what makes "git reconciliation between write lineages" a
  product-native concept.
- Explicit v1 non-goals: remote execution, brokers, pods, Kubernetes, distributed
  leases, snapshot/restore. The k8s design below is the v2 target that must not force a
  v1 re-architecture — hence the emphasis on keeping the seam at
  `WorkspaceLease.prepare/inspect/cleanup` + CAS manifests.

### 1.3 zeroshot-cloud capsule platform (private repo, planning/spec)

Approved beta shape (capsule-sandbox-platform.md, infrastructure-shape.md):

- EKS, `us-east-1`, two clusters per cell (product + capsule); capsule pods run under
  gVisor (`runsc --platform=systrap`), Karpenter-managed standard EC2 nodes, **baked
  AMIs** (runtime image preloaded), warm-node headroom, provisioning target <2s p50 warm.
- **Beta storage path:** two capsule-bound encrypted `gp3` EBS PVCs per capsule
  (workspace + agent-state), `WaitForFirstConsumer`, KMS-encrypted; pod/node loss →
  exact old-runtime fencing → **same-AZ reattach** of the same PVC UIDs. Cross-AZ
  same-volume reattach is explicitly not claimed. EBS snapshots are manual
  backup/restore tooling only.
- **Dormant P4 substrate** (merged, tested, unselected): instance-store **NVMe + LVM
  thin pool (`vg_capsule/capsule_thin`) via TopoLVM v0.41**, XFS `pquota` project
  quotas per capsule (workspace + sidecar-state), thin snapshots
  (`iac/runtime/capsule-storage-node/`). The pre-simplification design (descoped by
  post-p4-v1-simplification-proposal.md §S4) was: periodic local thin snapshot →
  **Kopia encryption → S3 upload, 60-second RPO**, one-in-flight snapshot Lease,
  cleanup-drain. Descoped because complexity landed before evidence that EBS can't
  serve the beta; the NVMe path "remains an optional performance experiment" and may
  become the launch path only after benchmarks (attach/start latency, workload I/O,
  cost, failure recovery) beat an approved envelope.
- Deferred to v2+: per-user persistent home across capsules, point-in-time
  rewind, hibernation.

**Reading of internal state:** the beta answers "one capsule, one node, durable-ish
workspace" with EBS. It does **not** answer the question asked here — how N pods on
different nodes in one cluster run share a workspace lineage fast, and how a project's
workspace state gets a warm cache across runs. That is exactly the gap this document
covers. The dormant NVMe+thin-snapshot substrate and the descoped Kopia→S3 pipeline are
prior art _inside_ the org for the recommended direction.

## 2. External research

Claims below marked **[verified]** passed a 3-voter adversarial verification pass
against the live primary source (2026-07-19); the rest are sourced but single-checked.

### 2.1 The convergent industry pattern

Across every platform whose mechanism could be verified, **none uses a shared POSIX
filesystem or per-workspace network block devices as the durable workspace layer**.
The convergent architecture is: **node-local NVMe/SSD for hot I/O, durability in
object storage, warm caches keyed per project/config**, in three mechanism families:

1. **Archive/template to object storage + warm compute** (Gitpod, Codespaces).
2. **Content-addressed chunk store with lazy loading + tiered caches** (Modal; AWS
   Lambda's own container loader).
3. **Whole-VM / image-level snapshots** (E2B, Daytona, Morph; Blacksmith as the
   block-native outlier — self-managed Ceph on local NVMe, still avoiding cloud
   block storage).

### 2.2 Platform case studies

**Gitpod (Kubernetes era → Flex)** — [verified]

- Tried per-workspace PVCs and rejected them for: unpredictable attach/detach timing
  → unpredictable startup; reliability failures during startup; per-instance
  disk-attach count limits constraining the scheduler; AZ-locality constraining
  cross-AZ balancing. ("We're leaving Kubernetes", Christian Weichel & Alejandro de
  Brito Fontes, Oct 31 2024, https://ona.com/stories/we-are-leaving-kubernetes)
- Production converged on **node-local SSD RAID 0** for hot storage + a **DaemonSet
  uploading/downloading uncompressed tar archives to/from S3** for durability and
  cross-node mobility. Backup/restore I/O was heavy enough that they added per-workspace
  cgroup IO limits to stop co-located workspaces starving each other. (same source)
- Direct validation of the "no shared FS, no per-workspace network volumes" constraint,
  from the team with the most k8s-dev-environment mileage in the industry.

**GitHub Codespaces** — [verified]

- Prebuilds are **container-snapshot templates in blob storage, replicated per
  region**; creation fetches the template and attaches it to a VM. (github.blog "Codespaces
  for the largest repositories just got faster", Feb 23 2022; docs "About GitHub
  Codespaces prebuilds", live 2026)
- **Cache key = (repository, branch, devcontainer.json config)** — a shared artifact,
  not per-user; refreshed by CI on push (GitHub-managed Actions workflows; one
  concurrent prebuild run per config; alternatives: on-config-change or scheduled).
- Scale datum: github/github ≈ 13 GB; full clone 20 min; bootstrap ≥45 min; shallow
  clone + background unshallow → 90 s clone / ~5 min bootstrap; **warm prebuilt pools →
  ~10 s creation** (github.blog "GitHub's Engineering Team has moved to Codespaces",
  Aug 11 2021). The amortization target any caching design must beat.

**Modal** — [verified]

- Container images are not pulled eagerly: a **~5 MB file-metadata index over
  content-addressed blobs** loads in 1–100 ms and FUSE-mounts in ~2 ms; file contents
  fetched lazily on demand → ~1 min eager pull becomes ≤100 ms to usable FS; deferred
  I/O still costs (~2.5 GiB/s, ~200 ms for a 512 MiB file from cache). (modal.com/blog
  "Fast, lazy container loading in Modal.com", Sep 8 2024)
- Tiered cache behind it: RAM → local SSD → AZ cache → regional CDN → blob storage
  (illustrative figures: 100 µs local SSD, ~1 ms AZ, ~200 ms blob; speaker later noted
  they divested the AZ tier toward the CDN path). Mirrors the Lambda-paper tiering.
- **Three snapshot granularities** (sandbox-snapshots docs + "Directory Snapshots"
  blog, Feb 24 2026): filesystem snapshots stored as **diffs from the base image**;
  **directory snapshots** — `sb.snapshot_directory("/project")` → mount into another
  sandbox, decoupled from base image, riding the lazy-loading FS ("mounted instantly,
  contents prioritized for pre-loading"); memory snapshots (alpha, same-instance-type
  restriction). Directory snapshots ≈ exactly our "workspace as first-class snapshot
  object, restored lazily elsewhere" shape.

**E2B** — [verified]

- Firecracker pause/resume: pause persists filesystem + memory, **~4 s per GiB RAM**;
  **resume ~1 s**; network connections severed on pause. Filesystem-only variant via
  `keepMemory:false`. (e2b.dev/docs/sandbox/persistence, live 2026-07-19)

**Daytona** — [verified]

- Named persistence artifact "snapshot" is **OCI-image-based** (image/Dockerfile built
  into snapshot; `daytona snapshot push`); container sandboxes = filesystem-only "cold"
  snapshots, VM sandboxes = +memory "hot" (`includeMemory`). Stop→auto-archive to
  object storage; S3-backed FUSE volumes for shared data. (daytona.io/docs snapshots +
  volumes, live 2026-07-19)

**Blacksmith (CI sticky disks)** — [verified]

- Per-key persistent cache disks backed by a **self-managed Ceph cluster on local
  NVMe**; runners proxy through Storage Agents; disk exposed as ext4 block device;
  on request, **the last committed snapshot is cloned (Ceph RBD COW) and mounted**;
  on completion, unmounted and committed. (docs.blacksmith.sh sticky disks;
  useblacksmith/stickydisk README)
- The block-native way to get per-project clone-on-mount caching — at the cost of
  operating Ceph. Prior art for "snapshot lineage + clone per run" semantics.

### 2.3 More platforms — the CI/agent caching layer (single-checked)

The pattern splits three ways: **(a) block-level fork of local-NVMe volumes, no transfer
step**; **(b) whole-VM/disk snapshot-restore**; **(c) tarball-to-object-storage**.
Measured restore speeds: ~125 MB/s for (c) vs ~900+ MB/s attach-throughput for (a).

- **Depot (CI + agent sandboxes)** — Docker-layer cache on a **self-managed Ceph
  cluster of local NVMe SSDs, attached per-project as a network volume**; builder + volume
  pinned to same AZ. Migrated **off EBS** (write 146→900 MB/s, write IOPS 3k→30k,
  read 150→1,800 MB/s) — a direct "EBS was too slow, went to Ceph-on-NVMe" data point.
  Default 50 GB/project, $0.20/GB/mo. Agent sandboxes start "<5 s" with a persistent
  filesystem auto-mounted. (depot.dev/blog/depot-magic-explained 2024-01-18;
  /cache-v2-faster-builds 2023-07-17; /now-available-remote-agent-sandboxes 2025-08-13)
- **Namespace** — **local-NVMe persistent volumes with copy-on-write forks**: each run gets
  its private COW copy of the volume as of the last successful commit; exit 0 commits the
  fork as new parent (last-write-wins), failures discarded; "close to zero impact on
  startup latency," no upload/download. This is _exactly_ the single-writer/read-fork
  model, done at block level. (namespace.so/docs/architecture/storage/cache-volumes,
  updated 2026-06-26)
- **Buildkite hosted agents** — cluster-scoped external volumes on **local NVMe**, same
  fork/parent-version model (each job attaches a writable fork of the last committed
  parent); explicitly "not durable storage." Specialized git-mirror + container-cache
  volume types. (buildkite.com/docs/pipelines/hosted-agents/cache-volumes)
- **WarpBuild** — Snapshot Runners capture a full VM snapshot just before exit; next run
  boots from it (network-attached disk; docs warn it "might perform worse than the cache
  action" for very large file counts). (warpbuild.com/blog/snapshot-runners 2024-09-12)
- **Cursor cloud agents** — "efficiently hibernate and resume agent VMs between messages";
  "checkpoint, restore, and fork VM images"; docs: if `install` takes >a few seconds,
  Cursor takes an **internal checkpoint snapshot** and starts future agents from it.
  Storage tech undisclosed. Orchestration on Temporal; >40% of their PRs from cloud
  agents. (cursor.com/blog/cloud-agent-lessons 2026-06-02; /docs/cloud-agent)
- **OpenAI Codex cloud** — "caches container state for up to 12 hours"; resume = checkout
  branch + optional maintenance script; cache invalidated on setup/maintenance/env/secret
  change; Business/Enterprise share caches across users of an environment.
  (developers.openai.com/codex/cloud/environments)
- **Devin (Cognition)** — every session boots "a fresh copy of a snapshot" of the VM
  (cloned repos, installed tools, node_modules/venvs, env, browser cookies); org-wide
  "golden snapshots." Snapshot creation **~30 min → ~15 s**; time-to-first-message
  **~25 s → ~10 s**. (docs.devin.ai/product-guides/snapshots; cognition.com/blog Dec 2024)
- **CI baseline** — GitHub Actions cache = tarball to object storage; 10 GB/repo (now
  expandable, 2025-11), **~125 MB/s (1 Gbps) cap to Azure**; 4 GB save+restore = 89 s (GH)
  vs 25 s (Blacksmith) / 27 s (Namespace). CircleCI/Buildkite: tar+gzip→S3, keep <500 MB.
  (runs-on.com/benchmarks/github-actions-cache-performance 2026-07-08)

### 2.4 AWS primitives (numbers — single-checked, us-east-1)

**EBS attach/snapshot — the case against Option A:**

- **Attach latency: AWS publishes no number.** Clean detach+attach ≈ "tens of seconds"
  (Tiger Data 2025); the EBS CSI driver raised its attach timeout 15 s→60 s because
  `ControllerPublishVolume` can exceed 15 s; k8s force-detaches stuck volumes only after
  **6 min**; hard-node-failure detach can take **10–20 min**. Unpredictable tail = the
  Gitpod complaint, quantified.
- **Snapshot create is async, `pending`→`completed` "can take several hours"** (no
  published rate). So you cannot snapshot-per-publish in an inner loop.
- **CreateVolume-from-snapshot is lazy**: first-touch hydration from S3 measured at **`dd`
  ≈ 9 MB/s / ~70 ms per read, fio ≈ 45 MB/s** during hydration (35 h / 7 h to fully init
  1.5 TB); single-digit ms only after full init. (AWS Storage Blog, Apr 2022)
- **Fast Snapshot Restore** removes the penalty but costs **$0.75/DSU-hour/snapshot/AZ ≈
  $540/mo per snapshot per AZ**; credit model yields only **~1 volume per ~8 min** for a
  128 GiB snapshot; default quota **5 FSR snapshots/region**. Uneconomical per-project.
- **Provisioned Rate for Volume Initialization** (GA May 2025): force hydration at
  **100–300 MiB/s**, **$0.0036/GiB** at 300 MiB/s (10 GiB → ~34 s); regional cap
  5,000 MiB/s. Cheaper than FSR but still a per-restore tax and a hard throughput ceiling.
- **EBS direct APIs** (`ListChangedBlocks`/`GetSnapshotBlock`): **512 KiB blocks**, diff
  two snapshots **without creating a volume**, but **~500 MiB/s per snapshot** (1,000
  req/s) and ListChangedBlocks capped at **50 req/s**; GetSnapshotBlock $0.003/1k.
  Usable for incremental backup, too slow/rate-limited for a hot fan-out path.

**The primitives you'd actually build on:**

- **gp3**: 3,000 IOPS + 125 MiB/s free, up to 80,000 IOPS / 2,000 MiB/s (raised from
  16k/1,000 in late 2025); **$0.08/GB-mo** + $0.005/IOPS + $0.04/MiB/s. Single-digit-ms.
- **io2 Block Express**: up to 256,000 IOPS / 4,000 MB/s, **<500 µs** avg 16 KiB latency;
  $0.125/GB-mo + tiered IOPS. (fastest AWS block latency, but still network-attached)
- **Instance-store NVMe** (the hot tier): i4i.4xlarge 3,750 GB / 400k read IOPS /
  **2,800 MB/s**; i3en up to 60 TB / 2M IOPS / **16 GB/s**; m6id.4xlarge 950 GB. Nitro SSD
  ~60% lower latency vs i3. **Ephemeral — lost on stop/terminate**; this is why durability
  must live in S3, not on the node.
- **S3 Standard**: **100–200 ms** first-byte; **5,500 GET/s per prefix**, unlimited
  prefixes; GET **$0.0004/1k**, PUT **$0.005/1k**; **~11 GB/s** aggregate via CRT ranged
  parallelism on a 100 Gbps NIC. VPC gateway endpoint = no NAT/egress cost in-region.
- **S3 Express One Zone**: **~5 ms** round-trip (single-digit-ms), 2M GET / 200k PUT TPS
  per directory bucket; after Apr-2025 cuts GET **$0.00003/1k** (−85%), PUT $0.00113/1k,
  $0.11/GB-mo + per-GB bandwidth. **Single-AZ durability** — a cache/hot tier, not the
  system of record. "5–10× slower than EBS" but ~20–40× faster first-byte than S3 Std.
- **Karpenter**: **~45–60 s** pod-pending→node-ready cold; mitigate with low-priority
  **pause-pod overprovisioning** for warm headroom (the capsule beta already does warm
  nodes + baked AMIs).

**OCI lazy-load / ImageVolume — Option C just got more viable:**

- **Kubernetes `ImageVolume` (KEP-4639) is GA in k8s 1.36** (2026-04-22), and **EKS has
  supported 1.36 since 2026-06-02**; EKS AL2023 AMIs ship **containerd 2.2.4** (meets the
  containerd ≥2.1, subPath ≥2.2 requirement). Mount an OCI image read-only into a pod as a
  volume, natively. _Caveat:_ no first-party AWS doc yet confirms end-to-end
  ImageVolume-on-EKS; needs a spike. This materially de-risks "workspace-as-OCI-image".
- **SOCI (Seekable OCI)** lazy pull: index artifact beside the image + FUSE snapshotter;
  Fargate ~50% faster starts. **On EKS, lazy mode is self-managed** (bootstrap the
  snapshotter via node userData/Karpenter). **Parallel-pull mode is now built into recent
  EKS AL2023/Bottlerocket AMIs** (10.7 GB image 1m52s→45s, ~60%). gzip layers only, no
  zstd yet. SOCI index v2 (2025) adds a build-time `soci convert` step.
- **overlaybd/DADI** (Alibaba, ATC'20): block-device image format; "cold-start 10,000
  containers on 1,000 hosts in 4 s" in production. Nydus (RAFS/EROFS) + eStargz are the
  other lazy formats. All require snapshotter opers work on EKS.
- **ECR**: layer max **~50 GiB**, storage **$0.10/GB-mo**, **in-region transfer to
  EC2/EKS is free**; **PutImage 10 req/s** (push-side bottleneck for high-churn tags),
  GetDownloadUrlForLayer 3,000 req/s. Content-addressed → identical layers dedupe on-node.
- **containerd** content store is content-addressed (identical blobs stored once, shared
  across tags); GC is reference/lease-based (churned tags leave collectable garbage).
  Registry-at-scale pain is real (Uber built Kraken P2P; AWS Lambda abandoned whole-image
  pulls for 10 GiB images — see §2.5).

### 2.5 Systems prior art (single-checked)

**AWS Lambda, "On-demand Container Loading in AWS Lambda"** (Brooker, Danilov, Greenwood,
Piwonka; USENIX ATC'23) — the canonical AWS-native design for this exact problem, and the
blueprint for Option B:

- Container flattened to a block device, **fixed 512 KiB chunks**; manifest maps offsets
  → chunk keys.
- **Convergent encryption**: per-chunk key derived from the chunk's **SHA-256**, AES-CTR —
  dedup across tenants without a shared plaintext store. (We are multi-tenant → see OQ.)
- **Dedup**: **~80% of newly uploaded functions produce zero unique chunks**; among those
  with new chunks, mean **4.3%** / median **2.5%** unique; up to **23×** storage reduction.
- **Tiered cache hit rates** (one week, one large region): **67% worker-local / 32% in-AZ
  shared / 0.06% S3**. AZ cache alone hits **99.9%** median.
- **Erasure coding** in the AZ cache: **4-of-5** (25% overhead) to cut tail latency.
- Scale: up to **15,000 container starts/s** for one customer; cold starts as low as
  **50 ms**; images up to 10 GiB.

**Chunking + dedup:**

- **FastCDC** (Xia et al., ATC'16): **~2.5–3 GB/s** chunking (10× Rabin, 3× Gear) at
  near-equal dedup; dedup ratios **40–97%** across 7 datasets. The default modern CDC.
- **HuggingFace Xet — the critical refinement**: pure chunk-level CAS **doesn't scale**
  (45 PB ≈ 690 B chunks → millions of requests per upload). Their fix: CDC at **~64 KB
  chunks aggregated into ~64 MB content-addressed blocks (~1,000× fewer CAS entries)** +
  shards (file→chunk maps) + ~0.1% "key chunks" for global dedup lookup. Measured **~50%
  transfer/storage reduction** on code/model repos. **This is the answer to S3
  small-object request cost** — store blocks, not raw chunks.
- **Backup engines**: restic = Rabin CDC 512 KiB–8 MiB (~1 MiB avg), zstd since 0.14.
  kopia = buzhash **~4 MiB** default, zstd/s2; benchmarks show kopia ~4× restic throughput
  on multi-file S3 workloads (parallel uploads). **rustic** = Rust reimpl of the restic
  format, self-described beta / `rustic_core` "early development" — evaluate but don't
  assume production-ready.

**Git-native transfer:**

- **Partial clone `--filter=blob:none`** (blobless) is GitHub's recommended mode: all
  commits+trees, blobs on demand; `log`/`merge-base` behave normally. **Shallow clone is
  "the worst option"** for anything but throwaway checkouts (later fetches expensive,
  server can't reuse packfiles — treeless = 4× pack CPU). github/github: 20 min full clone
  → **90 s shallow**.
- **`bundle-uri`** (client support Git 2.38, late 2022): seed a clone from a static bundle
  on a CDN/object store, then incremental fetch from origin. GitLab shipped it in Gitaly
  (bundles in object storage; 3 concurrent bundle-URI clones ≈ imperceptible CPU vs a
  spike from one normal clone). **GitHub does not offer it.** `packfile-uris` (CDN pack
  offload) remains experimental.
- **`git clone --reference` / `--dissociate`**: borrow objects from a local mirror via
  `alternates`; "possibly dangerous" if the source prunes needed objects — `--dissociate`
  repacks to break the dependency. Node-local bare-mirror + reference clone is the fast
  in-cluster path.

**Shared-FS latency (evidence for the rejection):**

- **JuiceFS** (S3-backed POSIX + local cache): their own guide — small-file writes carry a
  **fixed 10–30 ms object-store API overhead** per file on a cache miss, vs **45 µs**
  buffered/cached — ~3 orders of magnitude. Confirms cold small-file latency on any
  S3-backed FS is bounded by S3, exactly the "too slow" the constraint anticipates.

**microVM (for completeness, Option E):**

- Firecracker docs make **no ms restore claim** ("optimized for speed"); UFFD backend does
  lazy page loading. REAP (ASPLOS'21): snapshot exec 95% slower than mem-resident from page
  faults, record-and-prefetch cuts cold-start **3.7×**. FaaSnap (EuroSys'22): within 3.5%
  of in-memory. All require KVM/bare-metal — outside the gVisor-on-EC2 plan.

## 3. Option space

Model of the workload the options are judged against:

- One cluster run = a graph of agent pods (conductor, planner, implementation worker,
  N validators, adversarial tester). **One writer** mutates the workspace; validators
  and testers want **point-in-time read-only copies** of the writer's published state,
  potentially on other nodes. Writes across lineages reconcile only through git.
- Workspace = `repo checkout` + `derived state` (`node_modules`, `target/`, venv,
  build caches) + `dirty overlay` (uncommitted edits, untracked files). Typical sizes:
  repo 10 MB–2 GB, derived 0.2–20 GB, overlay usually ≪ 100 MB.
- In-run publish→consume handoff should be seconds; cross-run warm materialization
  should be seconds; cold (new project / cold cell) tens of seconds is acceptable.
- Cross-run caching matters more than cold-start: users hammer one project for weeks.

### Option A — EBS-native block snapshots

Workspace lives on a per-workspace gp3 volume. Move between nodes by detach/attach
(same AZ). Fan out to readers and persist across runs via EBS snapshots; readers
create volumes from the snapshot.

- Cross-run cache: snapshot lineage is incremental and S3-backed by AWS itself.
- In-run fan-out: snapshot must reach a usable state, then each reader does
  CreateVolume + attach + mount; restored volumes lazy-load blocks from S3 with a
  first-touch penalty unless Fast Snapshot Restore is paid for per snapshot × AZ.
- Verdict (pre-research): right primitive for durability (already the capsule beta
  path), wrong primitive for fast in-run fan-out and per-project caching — attach
  latency, snapshot-completion latency, per-reader volume churn, AZ pinning, and API
  rate limits all fight the workload. Numbers in §2 confirm/deny.

### Option B — Content-addressed file-level snapshots over S3 + node-local NVMe cache

The Lambda-paper pattern applied at file granularity; the org's descoped
Kopia→S3 design, rebuilt as a first-class transfer protocol rather than a backup.

- **Manifest** = file tree (path, mode, symlink/exec bits, content hash, chunk list).
  **Chunks** = content-defined (FastCDC-class, ~64 KiB–1 MiB avg), zstd-compressed,
  SHA-256-addressed, KMS-envelope-encrypted, in a per-cell S3 bucket (VPC gateway
  endpoint, no NAT cost). Manifests are themselves small CAS objects — the
  `ArtifactRef` receipt pattern already in OECP/zeroshot-rust.
- **Node cache**: DaemonSet owning an LRU chunk CAS on instance-store NVMe
  (hostPath); materialization = hardlink/reflink farm from cache into the pod's
  workspace dir, parallel S3 GET for misses. Publish = incremental scan (mtime+size
  fast path), upload new chunks + manifest, emit receipt on the message bus/ledger.
- Writer/reader semantics drop out of immutability: a published manifest digest is a
  frozen point-in-time snapshot; readers materialize it read-only; the Exclusive
  lease (zeroshot-rust #669) fences the single writer.
- Cross-run caching: chunk LRU on nodes + soft scheduling affinity ("prefer nodes
  that recently served this project") + S3 as the durable tier. Dedup is automatic
  across runs, branches, and (within an org) across projects sharing deps.
- Verdict (pre-research): best fit to the constraints; cost is building/operating the
  chunk store + node daemon + scan/materialize tooling. Off-the-shelf engines
  (kopia/restic/rustic) prove the data model but are backup-shaped (single repo lock
  models, no node-cache tier, no receipt integration), so likely "reuse the design,
  own the implementation" — which also matches the Rust ArtifactStore CAS trajectory.

### Option C — OCI images as the snapshot format (ECR + containerd)

Pack workspace state as OCI layers; publish to ECR; readers consume via image pull
(or Kubernetes `ImageVolume`) with containerd's content store as the node cache;
optionally SOCI/stargz for lazy pulls.

- Cross-run caching falls out of the containerd image store + Karpenter node reuse;
  distribution, signing (cosign), GC, and registry auth are all existing machinery —
  the cloud repo already ships signed OCI releases via Flux.
- Costs: tar-diff whole-file granularity (no sub-file dedup), layer-chain growth
  needing periodic squash, publish latency = tar+compress+push of the delta (**ECR
  PutImage 10 req/s** caps high-churn tags), containerd GC pressure from high-churn
  artifact images.
- **Maturity update from §2.4:** `ImageVolume` is **GA in k8s 1.36 and EKS runs 1.36
  since 2026-06-02** with containerd 2.2.4 AMIs — the read path is now a supported
  primitive, not a bet (still needs an EKS+gVisor spike). SOCI parallel-pull is baked
  into current EKS AMIs.
- Verdict: attractive "no new storage service" packaging of Option B's read path,
  weaker on publish latency and dedup; strongest where snapshots are **coarse and
  infrequent** (per-run base images refreshed like Codespaces prebuilds) rather than
  per-publish. Good candidate for the _base-snapshot_ tier, not the inner loop.

### Option D — Git as the transfer plane + keyed derived caches (two-plane)

Lean into the constraint that reconciliation is git-based and Zeroshot culture
already mandates WIP commits over stash:

- **Repo plane:** the writer publishes state as git commits (WIP commits included) to
  a cluster-reachable store — node-local bare mirror + S3 bundle/packfile fallback;
  readers `clone --reference`/fetch from the node mirror (delta packs only). Git is
  already a content-addressed store with optimal delta transfer; nothing to build for
  correctness.
- **Derived plane:** deps/build dirs restored from cache entries keyed by
  `(lockfile hash, toolchain, os/arch)` — tar.zst or CAS-chunked objects in S3 with
  the same node-local cache; miss ⇒ rebuild (`npm ci`, `cargo build`) instead of
  transfer.
- **Overlay:** uncommitted/untracked residue either forbidden by convention (agents
  commit WIP) or carried as a small CAS snapshot (reuses Option B machinery at tiny
  scale).
- Verdict (pre-research): cheapest credible v0 and philosophically aligned; its weak
  spot is exactly-reproducing full workspace state (validators seeing precisely what
  the writer saw, including mid-build artifacts) and rebuild cost on derived-cache
  misses.

### Option E — microVM/pause-resume snapshots (Firecracker/CRIU family)

What E2B/Modal/Morph do (memory + disk snapshot, resume anywhere). Requires
bare-metal/KVM nodes and a different runtime than the selected gVisor-on-EC2 EKS
path; zeroshot-cloud explicitly excludes live-memory/CRIU restore. Captured here
because the _disk-side_ techniques these platforms use (chunked lazy-loading CAS,
snapshot trees) are Option B in different clothing — see §2.

### Option F — self-managed distributed block store on local NVMe, clone-on-fork

What **Depot, Blacksmith, Namespace, and Buildkite independently converged on**: a
storage service (Ceph in Depot/Blacksmith's case) running on **instance-store NVMe**,
exposing per-workspace block volumes that are **snapshot-cloned (COW) and mapped** into
the run at start, and **committed** back on success. No object-storage round-trip in the
hot path; single-writer/read-fork falls out of COW snapshots.

- Strong evidence: this is the _most common_ answer among serious CI/agent platforms,
  and Depot published the explicit "**EBS too slow → Ceph-on-NVMe**" migration (146→900
  MB/s writes). Namespace's model (private COW fork of last commit, exit-0 commits new
  parent) is almost line-for-line our single-writer/read-fork requirement.
- Cross-run caching: excellent — the volume _is_ the warm project cache; clone is cheap.
- Costs: **you operate a distributed storage system** (Ceph RBD or equivalent) on the
  cluster — the single biggest ops burden of any option here; node-affinity/AZ pinning of
  volumes; NVMe is ephemeral so you still need async replication or S3 backup for
  durability (Ceph replication across nodes handles the node-loss case at 2–3× storage).
- Verdict: the pragmatic "buy vs build" mirror of Option B — **operate infrastructure
  instead of building a CAS**. Correct semantics out of the box; heavy to run. Viable if
  we'd rather run Ceph than write a chunk store, but it fights the "no cross-node network
  block attach" instinct (it _is_ network-attached, just backed by NVMe + proxied).

### Rejected baseline — shared POSIX filesystem

EFS / FSx (and self-hosted S3-backed POSIX layers like JuiceFS) are excluded by
constraint; §2 records their measured latency profiles as evidence for the record.

## 4. Recommendation

### 4.0 Debate outcome (2026-07-19) — supersedes the B-first framing below

A four-senior-engineer adversarial debate (storage architect / platform-SRE /
ship-it-staff / k8s-runtime) **converged in 2 rounds** and materially revised the
original recommendation. **The doc's headline "commit to B (chunk store) as the v1 bulk
engine" is now held by no one, including the architect who argued it.** Two arguments
dissolved B's primacy:

1. **On the stated priority (one project hammered for weeks → cross-_run_ warm restore),
   B, C, and D are indistinguishable** — the node-local NVMe cache doing the warm
   hardlink/reflink restore (2.8–16 GB/s) is the load-bearing perf mechanism regardless
   of bulk engine. "Which bulk engine" is therefore only a cold/miss/churn-path and
   storage-cost question.
2. **Immutable-manifest single-writer/read-fork correctness is a property of _any_
   content-addressed digest** (git SHA, OCI digest, keyed object) — not a reason to
   pick B. That was B's headline correctness argument; it confers no edge.

That leaves B with only sub-file dedup, which pays off solely on large
incrementally-mutated trees. **Converged plan:**

- **v0 (now):** ship **Option D** — git plane (node-local bare mirror + blobless/
  `--reference` fetch) + **node-local NVMe cache doing the warm reflink restore** +
  lockfile/toolchain-keyed tar.zst derived caches — **on the already-merged
  LVM-thin/TopoLVM/XFS-pquota substrate**. Adopt the WorkspaceManifest/ArtifactRef
  receipt model as the semantic backbone; keep the `ArtifactStore` "inject external CAS"
  and `WorkspaceLease` `Snapshot(digest)` seams **open**; and **instrument** (warm-restore
  p99, derived-miss cost, cross-node transfer volume, derived-tree mutation granularity,
  gVisor materialization overhead).
- **Same-node writer→reader fork = LVM-thin COW clone** on the merged P4 substrate
  ("F-semantics without operating Ceph"), _not_ a distributed store.
- **v1 (cross-node distribution of derived/base plane) = Option C: OCI + ImageVolume +
  SOCI parallel-pull**, reusing already-operated machinery (containerd 2.2.4, EKS 1.36
  ImageVolume GA, Flux signed-OCI). An OCI digest _is_ the hash-addressed ArtifactRef
  receipt. (Split on _timing_ only — proactive vs after a gVisor spike vs metric-gated.)
- **v2 = Option B**, built **only if a named v0 metric** proves large
  incrementally-mutated cross-node trees (Rust `target/`, sccache) dominate — the one
  regime where sub-file CDC beats OCI whole-file layers.
- **Reject operating Ceph-on-NVMe (F)** as a distributed engine outright.

**Unresolved:** the exact v0 metric/threshold that would promote C or greenlight B;
sequencing of the ImageVolume+gVisor-on-EKS spike (OQ#4). **Dissent:** the ship-it
engineer would defer even C until a metric fails (treating OCI as the likely-cheapest of
three metric-triggered arms), not pre-adopt it.

The §4.1–4.6 material below is the _original_ B-centric write-up; read it as the design
of the B tier (now v2/metric-gated), not the primary recommendation. §7 analyses how much
of the _converged_ plan is already built and the waste risk.

---

**Recommended (original doc position, now the v2 tier): Option D + Option B as bulk engine**
("git is the bus for source truth; a content-addressed snapshot store is the bus for
everything else"), with Option A (EBS snapshots) retained only as operational
backup — which is what the capsule beta already decided.

**The one real fork in the road is B vs F: build a content-addressed chunk store, or
operate a distributed NVMe block store (Ceph).** Both are proven — B by AWS Lambda,
Modal, and HF Xet; F by Depot, Blacksmith, Namespace, and Buildkite. Recommendation is
**B**, because (1) zeroshot-rust is _already_ building a hash-addressed `ArtifactStore`
CAS with an "inject an external CAS" seam, so B reuses a component instead of adding
Ceph as a new operational surface; (2) B's dedup is cross-run _and_ cross-project
(automatic — Lambda saw 80% of uploads produce zero new chunks), which is precisely the
stated priority; (3) immutable manifests give the single-writer/read-fork semantics for
free without a distributed lock service. Choose **F instead only if** the team would
rather run Ceph than write and maintain a chunk store — a legitimate call, and the note
below on Depot's "EBS→Ceph" migration is the evidence it works.

### 4.1 The workspace snapshot model

A published workspace state is a small immutable **manifest** (a CAS object, addressed
by digest, carried around as an `ArtifactRef`-style receipt):

```
WorkspaceManifest v1
  base:      { repo_id, commit_sha, git_pack_refs }   # source plane — git-native
  derived:   [ { key: (path, lockfile_hash, toolchain), chunks: [...] } ]  # deps/build
  overlay:   { files: [ (path, mode, chunk_list) ] }  # dirty/untracked residue
```

- **Source plane:** the writer publishes WIP commits (already Zeroshot convention —
  WIP commits over stash) and the manifest pins a commit SHA. Readers materialize via
  fetch from a node-local bare mirror / S3-cached packs. Git gives content addressing
  and minimal deltas for free; reconciliation across write lineages stays pure git,
  exactly matching the "never parallel writes without git reconciliation" rule.
- **Derived plane** (`node_modules`, `target/`, venvs): FastCDC-chunked (~64 KiB avg),
  zstd-compressed, SHA-256-addressed. **Store chunks aggregated into ~64 MB
  content-addressed blocks, not as individual S3 objects** (HuggingFace Xet's
  "chunks→blocks" — pure per-chunk CAS creates millions of S3 requests per publish; at
  S3 PUT $0.005/1k that is both slow and a real bill). Manifests reference (block,
  offset, len) spans. Restores hit a **node-local NVMe chunk cache** first
  (DaemonSet-owned LRU CAS, hostPath on an i/*d-family node), then S3 (Standard for
  durability; S3 Express One Zone as an optional ~5 ms hot tier for manifests + popular
  blocks).
- **Overlay:** whatever `git status` can't account for, same chunk machinery; kept
  small by convention (agents commit WIP).

Publish = incremental scan (mtime+size fast path) → upload new chunks + manifest →
emit receipt on the ledger/bus. Consume = materialize manifest read-only (hardlink
farm from node cache; parallel S3 GET for misses).

### 4.2 Concurrency and fencing

- One **Exclusive** writer lease per workspace lineage, ledger-fenced with owner
  token — this is precisely zeroshot-rust's `WorkspaceLease` + #669
  `ReadOnly|Exclusive`, extended with a `Snapshot(manifest_digest)` locator for k8s.
- Readers never lease the writer's copy; they materialize a **frozen manifest digest**
  — immutability replaces distributed-FS coherence. A validator always sees exactly
  the bytes the writer published, which is _stronger_ than sharing a live FS.
- Publishes are atomic: chunks first, manifest last, receipt in the ledger after —
  the same "write value last" fail-closed pattern the capsule grant store uses.

### 4.3 Caching across cluster runs

Three tiers, coldest wins nothing:

1. **Node-local chunk cache** (instance-store NVMe; Karpenter *d/i-family nodes):
   warm-project restores mostly hardlink without network. Soft scheduling affinity
   ("prefer nodes that served this project recently") raises hit rates without
   correctness dependence — like Lambda's 67% local hit rate.
2. **S3 chunk store** (regional, gateway endpoint, no NAT cost): durable tier; chunk
   dedup across runs/branches/projects-within-org is automatic. Optionally S3
   Express One Zone for manifests + small hot chunks if p99 matters later.
3. **Project base snapshots**: a per-(repo, branch, lockfile-hash) "prebuild"
   manifest refreshed opportunistically after runs — the Codespaces cache key,
   applied to our world. New cluster runs start from the newest base manifest and
   fetch only the delta.

### 4.4 Why not the alternatives as primary

- **EBS snapshot protocol (A):** Gitpod's four documented PVC failure modes are our
  workload's failure modes. The numbers make it concrete: snapshot `pending→completed`
  "can take several hours" (no per-publish snapshots); from-snapshot restore lazy-loads
  at **9–45 MB/s first-touch**; **FSR is ~$540/mo per snapshot per AZ** with a 5/region
  quota; even Provisioned-Rate init caps at 300 MiB/s and taxes every restore; attach
  tail latency is unbounded (6-min force-detach). Keep for backup/restore only (already
  the beta posture).
- **OCI images (C):** read path is now a supported primitive (ImageVolume GA on EKS
  1.36), but whole-file tar-diff granularity, layer-chain growth, and **ECR PutImage
  10 req/s** make it the wrong _publish_ path for per-iteration snapshots. Reasonable
  _packaging_ for coarse per-run base images (Codespaces-style prebuilds); not the inner
  loop.
- **Ceph-on-NVMe (F):** correct semantics, proven by 4 platforms, but adds a distributed
  storage system to operate and doesn't dedup across projects the way a CAS does. It's
  the fallback if building B proves too costly, not the default.
- **Shared FS (rejected by constraint):** industry evidence (Gitpod explicitly; every
  other platform implicitly) supports the rejection.
- **microVM memory snapshots (E):** solves a problem we don't have (resume running
  processes); requires KVM/bare-metal + runtime change; files-only snapshots keep the
  gVisor-on-EC2 EKS plan intact.

### 4.5 Fit with existing plans

- **OECP:** no wire changes. Manifests/chunks are exactly the "external CAS behind
  `ArtifactRef` receipts" the protocol already anticipates; workspace materialization
  stays a product-side resolver concern.
- **zeroshot-rust:** `WorkspaceLease` gains a fourth locator shape (snapshot manifest)
  under the same prepare/inspect/cleanup + owner-token contract; `ArtifactStore`'s
  "future hosted product may inject an external CAS" seam is where the S3 chunk store
  plugs in. Nothing in v1 needs to change now.
- **zeroshot-cloud:** the dormant P4 NVMe substrate becomes the node-cache/workspace
  tier (its thin-snapshot machinery is _optional_ under this design — the CAS layer,
  not LVM, provides snapshots; XFS pquota still does per-workspace quotas); the
  descoped Kopia→S3 pipeline is vindicated in shape but returns as a _transfer
  protocol_ with a node cache, not a backup loop. EBS PVC beta path remains valid for
  single-capsule durability and as the fallback while this is built.

### 4.6 Phasing

1. **v0 (cheapest credible):** git plane (node-local bare mirror + `--reference`/
   blobless fetch; **never plain shallow**) + per-key derived-cache tarballs (tar.zst to
   S3, keyed by lockfile hash) + node-local cache dir. No chunking yet. This is the
   Gitpod/CI-cache baseline; GH's own cache path caps ~125 MB/s and still beats cold
   rebuilds, so this likely already hits "seconds warm, <1 min cold." Ship this first and
   measure before building anything fancier.
2. **v1:** replace tarballs with CDC-chunked, **block-aggregated** CAS (kopia-style
   engine in Rust; evaluate `rustic_core` first — note its "early development" status);
   manifests as `ArtifactRef`-style receipts; node DaemonSet cache with LRU + metrics;
   soft project affinity. Optional: per-(repo, branch, lockfile) base-snapshot published
   as an **OCI image consumed via ImageVolume** (now GA on EKS) for the cold-start tier.
3. **v2 (only if p99 demands):** lazy materialization (FUSE or overlay upperdir over a
   chunk-backed lower) for very large derived trees — the Modal/Lambda read path — and/or
   an in-AZ shared cache tier with erasure coding (Lambda's 32% AZ hit rate) if node-local
   hit rates disappoint.

## 5. Open questions

1. **Granularity of publish:** per graph-node handoff (worker→validators) vs
   per-iteration? Drives chunk-scan frequency; incremental scan of a 2 GB tree is
   ~O(100ms–1s) warm, fine either way.
2. **Docker-mode workspaces:** inside-container paths differ; the chunk cache mount
   and materialization need the isolation-manager's cooperation (mount the
   materialized dir vs materialize inside the container).
3. **Per-org dedup scope:** convergent-style dedup across orgs leaks
   existence-of-content; scope chunk dedup + encryption keys per org (Lambda paper's
   convergent encryption is single-tenant-operator context; we are multi-tenant).
4. **gVisor I/O overhead** on hardlink-farm materialization (runsc file access via
   gofer/directfs) — needs a benchmark before trusting warm-restore latency targets.
5. **Node cache admission/eviction:** per-org quotas on the shared node cache to
   prevent noisy-neighbor eviction; tie into existing XFS pquota machinery?
6. **Where the writer runs:** pinning the writer pod to the node holding the
   warmest cache (affinity) vs letting the scheduler roam and paying one cold
   restore. Likely: soft affinity, measure.
7. **Manifest format ownership:** OECP-adjacent spec (like ArtifactRef) or purely
   product-internal? Recommend product-internal v0, promote once stable.

## 6. Sources

Method: platform case studies (§2.2) passed a 3-voter adversarial verification against
the live primary source on 2026-07-19 (deep-research run `wf_3bbf724b-e93`). §2.3–2.5
are single-checked against the cited primary source (mostly fetched 2026-07-19). Numbers
flagged UNCONFIRMED below could not be verified from a primary source.

**Platform case studies (verified):**

- Gitpod, "We're leaving Kubernetes" — https://ona.com/stories/we-are-leaving-kubernetes (2024-10-31)
- GitHub, "Codespaces for the largest repositories just got faster" — https://github.blog/news-insights/product-news/codespaces-largest-repositories-faster/ (2022-02-23)
- GitHub Docs, "About GitHub Codespaces prebuilds" — https://docs.github.com/en/codespaces/prebuilding-your-codespaces/about-github-codespaces-prebuilds
- GitHub Docs, "Configuring prebuilds" — https://docs.github.com/en/codespaces/prebuilding-your-codespaces/configuring-prebuilds
- GitHub, "GitHub's Engineering Team has moved to Codespaces" — https://github.blog/engineering/githubs-engineering-team-moved-codespaces/ (2021-08-11)
- Modal, "Fast, lazy container loading in Modal.com" — https://modal.com/blog/jono-containers-talk (2024-09-08)
- Modal, "Directory Snapshots: Resumable project state for Sandboxes" — https://modal.com/blog/directory-snapshots-resumable-project-state-for-sandboxes (2026-02-24)
- Modal Docs, "Sandbox snapshots" — https://modal.com/docs/guide/sandbox-snapshots
- E2B Docs, "Sandbox persistence" — https://e2b.dev/docs/sandbox/persistence
- Daytona Docs, "Snapshots" / "Volumes" — https://www.daytona.io/docs/en/snapshots/ , https://www.daytona.io/docs/en/volumes/
- Blacksmith Docs, "Sticky disks" — https://docs.blacksmith.sh/blacksmith-caching/dependencies-sticky-disks ; https://github.com/useblacksmith/stickydisk

**Other platforms / CI (single-checked):**

- Depot — https://depot.dev/blog/depot-magic-explained (2024-01-18); https://depot.dev/blog/cache-v2-faster-builds (2023-07-17); https://depot.dev/blog/introducing-depot-cache (2025-01-14); https://depot.dev/blog/now-available-remote-agent-sandboxes (2025-08-13); https://depot.dev/blog/now-available-claude-code-sessions-in-depot (2025-07-01)
- Namespace — https://namespace.so/docs/architecture/storage/cache-volumes
- Buildkite — https://buildkite.com/docs/pipelines/hosted-agents/cache-volumes
- WarpBuild — https://www.warpbuild.com/blog/snapshot-runners (2024-09-12); https://docs.warpbuild.com/ci/snapshot-runners
- Cursor — https://cursor.com/blog/cloud-agent-lessons (2026-06-02); https://cursor.com/docs/cloud-agent
- OpenAI Codex — https://developers.openai.com/codex/cloud/environments
- Devin — https://docs.devin.ai/product-guides/snapshots ; https://cognition.com/blog/dec-24-product-update (2024-12)
- GitHub Actions cache size change — https://github.blog/changelog/2025-11-20-github-actions-cache-size-can-now-exceed-10-gb-per-repository/ ; cache v2 — https://github.com/actions/cache/discussions/1510
- CI cache benchmark — https://runs-on.com/benchmarks/github-actions-cache-performance/ (2026-07-08)
- CircleCI caching — https://circleci.com/docs/guides/optimize/caching/

**AWS primitives:**

- EBS attach anecdata — https://www.tigerdata.com/blog/fluid-storage-forkable-ephemeral-durable-infrastructure-age-of-agents ; https://planetscale.com/blog/planetscale-metal-theres-no-replacement-for-displacement ; EBS CSI FAQ — https://github.com/kubernetes-sigs/aws-ebs-csi-driver/blob/master/docs/faq.md
- EBS snapshots — https://docs.aws.amazon.com/ebs/latest/userguide/how_snapshots_work.html ; restore latency — https://aws.amazon.com/blogs/storage/addressing-i-o-latency-when-restoring-amazon-ebs-volumes-from-ebs-snapshots/ (2022-04) ; init — https://docs.aws.amazon.com/ebs/latest/userguide/initalize-volume.html ; Provisioned Rate GA — https://aws.amazon.com/about-aws/whats-new/2025/05/ebs-provisioned-rate-volume-initialization/
- FSR — https://docs.aws.amazon.com/ebs/latest/userguide/ebs-fast-snapshot-restore.html ; credits — https://docs.aws.amazon.com/ebs/latest/userguide/volume-creation-credits.html
- EBS direct APIs — https://docs.aws.amazon.com/ebs/latest/userguide/ebs-accessing-snapshot.html ; pricing — https://docs.aws.amazon.com/ebs/latest/userguide/ebsapi-pricing.html
- gp3/io2 — https://docs.aws.amazon.com/ebs/latest/userguide/general-purpose.html ; https://aws.amazon.com/ebs/volume-types/ ; https://aws.amazon.com/ebs/pricing/
- Instance store NVMe — https://docs.aws.amazon.com/ec2/latest/instancetypes/so.html ; https://aws.amazon.com/ec2/instance-types/i4i/ ; /i3en/ ; ephemerality — https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/InstanceStorage.html
- S3 perf/pricing — https://docs.aws.amazon.com/AmazonS3/latest/userguide/optimizing-performance-guidelines.html ; https://aws.amazon.com/s3/pricing/ ; CRT throughput — https://aws.amazon.com/blogs/storage/improving-amazon-s3-throughput-for-the-aws-cli-and-boto3-with-the-aws-common-runtime/ (2024-05)
- S3 Express One Zone — https://aws.amazon.com/s3/storage-classes/express-one-zone/ ; price cut — https://aws.amazon.com/blogs/aws/up-to-85-price-reductions-for-amazon-s3-express-one-zone/ (2025-04) ; latency check — https://nixiesearch.substack.com/p/benchmarking-read-latency-of-aws (2025-12)
- Karpenter — https://cast.ai/blog/deploy-karpenter-eks-node-autoscaling/ ; warm headroom — https://aws.amazon.com/blogs/containers/eliminate-kubernetes-node-scaling-lag-with-pod-priority-and-over-provisioning/ (2023-01)

**OCI lazy-load / ImageVolume:**

- ImageVolume — https://kubernetes.io/docs/tasks/configure-pod-container/image-volumes/ ; 1.33 beta — https://kubernetes.io/blog/2025/04/29/kubernetes-v1-33-image-volume-beta/ ; 1.35 — https://kubernetes.io/blog/2025/12/17/kubernetes-v1-35-release/ ; KEP-4639 — https://github.com/kubernetes/enhancements/tree/master/keps/sig-node/4639-oci-volume-source
- EKS 1.36 — https://aws.amazon.com/about-aws/whats-new/2026/06/amazon-eks-distro-kubernetes-version-1-36/ ; EKS versions — https://docs.aws.amazon.com/eks/latest/userguide/kubernetes-versions.html ; EKS feature-gate request — https://github.com/aws/containers-roadmap/issues/512
- SOCI — https://github.com/awslabs/soci-snapshotter ; Fargate — https://aws.amazon.com/blogs/aws/aws-fargate-enables-faster-container-startup-using-seekable-oci/ ; parallel-pull on EKS — https://aws.amazon.com/blogs/containers/introducing-seekable-oci-parallel-pull-mode-for-amazon-eks/ ; EKS snapshotter guide — https://awslabs.github.io/ai-on-eks/docs/guidance/container-startup-time/accelerate-pull-process/containerd-snapshotter
- DADI/overlaybd — https://www.usenix.org/conference/atc20/presentation/li-huiba ; Nydus — https://github.com/dragonflyoss/nydus ; Slacker — https://www.usenix.org/conference/fast16/technical-sessions/presentation/harter
- ECR — https://docs.aws.amazon.com/AmazonECR/latest/userguide/service-quotas.html ; https://aws.amazon.com/ecr/pricing/ ; https://docs.aws.amazon.com/AmazonECR/latest/userguide/pull-through-cache.html
- containerd — https://github.com/containerd/containerd/blob/main/docs/content-flow.md ; GC — https://github.com/containerd/containerd/blob/main/docs/garbage-collection.md ; Uber Kraken — https://github.com/uber/kraken

**Systems prior art:**

- AWS Lambda container loading — https://www.usenix.org/system/files/atc23-brooker.pdf / https://arxiv.org/abs/2305.13162 (ATC'23)
- FastCDC — https://www.usenix.org/conference/atc16/technical-sessions/presentation/xia (ATC'16)
- HuggingFace Xet — https://huggingface.co/blog/from-files-to-chunks (2024-11-20); https://huggingface.co/blog/from-chunks-to-blocks (2025-02-12); https://huggingface.co/docs/hub/xet/deduplication
- restic chunker — https://restic.readthedocs.io/en/stable/100_references.html ; compression — https://restic.net/blog/2022-08-25/restic-0.14.0-released/ ; kopia — https://kopia.io/docs/advanced/compression/ ; rustic — https://rustic.cli.rs/docs/comparison-restic.html ; https://docs.rs/rustic_core/
- kopia vs restic benchmark — https://cloudcasa.io/blog/comparing-restic-vs-kopia-for-kubernetes-data-movement/
- Git partial/shallow — https://github.blog/open-source/git/get-up-to-speed-with-partial-clone-and-shallow-clone/ (2020-12-21) ; clone study — https://github.blog/open-source/git/git-clone-a-data-driven-study-on-cloning-behaviors/ ; bundle-uri — https://about.gitlab.com/blog/reduce-the-load-on-gitlab-gitaly-with-bundle-uri/ (2025-06-24) ; https://git-scm.com/docs/git-bundle ; https://git-scm.com/docs/git-clone
- JuiceFS perf — https://juicefs.com/docs/community/performance_evaluation_guide/ ; SeaweedFS — https://github.com/seaweedfs/seaweedfs
- Firecracker snapshots — https://github.com/firecracker-microvm/firecracker/blob/main/docs/snapshotting/snapshot-support.md ; REAP — https://marioskogias.github.io/docs/reap.pdf ; FaaSnap — https://www.sysnet.ucsd.edu/~voelker/pubs/faasnap-eurosys22.pdf

**Internal (repo/issues):**

- OECP epic zeroshot#643; docs/openengine-cluster-protocol/v1/*
- zeroshot-rust epic zeroshot#665; workspaces #677, source delivery #680, ArtifactStore #699
- zeroshot-cloud: planning/spec/capsule-sandbox-platform.md, infrastructure-shape.md, post-p4-v1-simplification-proposal.md; iac/runtime/capsule-storage-node/

**Key UNCONFIRMED items** (need direct verification before relying on them):
AWS-official EBS gp3 attach/detach wall-clock; snapshot `pending→completed` MB/s;
PutSnapshotBlock per-request price; exact date of the gp3 80k-IOPS increase; first-party
AWS confirmation that ImageVolume works end-to-end on EKS 1.36 + gVisor; WarpBuild
snapshot retention; whether Cursor/Codex use Firecracker vs EBS snapshots (undisclosed).

## 7. What's already built in zeroshot-cloud, and the v1→v2 waste risk

Inventory taken 2026-07-19 against a fresh `the-open-engine/zeroshot-cloud` clone
(grep across `backend/` + `iac/`, excluding `planning/`). This maps the **converged
plan** (§4.0), not the original B-first framing, onto what exists.

### 7.1 What exists — and which plan tier it serves

| Component (converged plan)                                                                  | Built in zeroshot-cloud?                                                                                   | Where                                                                                                                                                                                                             |
| ------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Instance-store **NVMe node pool** (v0 hot-tier + same-node fork hardware)                   | **Yes**                                                                                                    | `iac/modules/eks-cluster/karpenter-substrate/main.tf` — `capsule-nvme` NodePool, families `c6id/m6id/r6id`, gVisor, `storage-not-ready` startup taint                                                             |
| **LVM-thin / TopoLVM / XFS-pquota** substrate (v0 hot tier + LVM-thin COW same-node fork)   | **Yes, but dormant/unselected**                                                                            | `iac/runtime/capsule-storage-node/` — `initialize.sh` (thin pool, capacity accounting, project-ID allocator, TopoLVM `lvmd.yaml`), `quota.sh` (XFS pquota), `loopback.test.sh` (proves a **thin snapshot** = COW) |
| **S3 durable tier**                                                                         | **Yes** (generic cell bucket)                                                                              | `iac/modules/blob/main.tf` — versioned, SSE-KMS, TLS-enforced                                                                                                                                                     |
| **EBS gp3 + KMS** (A = backup/agent-state/fallback)                                         | **Yes, active beta path**                                                                                  | `iac/modules/capsule-storage/main.tf` — `capsule-encrypted-gp3` StorageClass                                                                                                                                      |
| **OCI / ECR / cosign / Flux** distribution (v1 read side)                                   | **Yes, operated** — for _deploy_ artifacts                                                                 | Flux signed-OCI pipeline (infra-shape D14); EKS 1.36 + **containerd 2.2.4** AMIs ⇒ **ImageVolume GA available**                                                                                                   |
| `ArtifactRef` receipt type (manifest backbone)                                              | **Partial** — exists as an OECP wire type + capsule recovery seam; **no WorkspaceManifest specialization** | `backend/**` (recovery), `crates/openengine-cluster-protocol` (main repo)                                                                                                                                         |
| **Git plane** (bare mirror, blobless/`--reference` fetch, S3 pack fallback)                 | **No** — greenfield                                                                                        | — (no clone/fetch/gix/git2 in product code)                                                                                                                                                                       |
| **Node-local cache daemon** + reflink/hardlink materialize/publish                          | **No** — greenfield                                                                                        | —                                                                                                                                                                                                                 |
| **Lockfile-keyed tar.zst derived caches**                                                   | **No** — greenfield                                                                                        | —                                                                                                                                                                                                                 |
| **OCI packing of _workspaces_** (vs deploy images) + ImageVolume consumption for workspaces | **No** — greenfield; + the ImageVolume+gVisor spike is unrun                                               | —                                                                                                                                                                                                                 |
| **Content-addressed chunk store (B)** — FastCDC, blocks, DaemonSet LRU, distributed GC      | **No — 0 files** (`fastcdc`/`kopia`/`restic`/`overlayfs`/`reflink` all absent)                             | —                                                                                                                                                                                                                 |

**Also not yet built:** the capsule _control plane_ that would host any workspace
transfer — `backend/crates` has `capsule-agent`/`runner`/`runtime-stub`/`auth`/`common`/
`eventing` but **no `capsule-operator` or `capsule-ext-authz` crate yet**, and the beta
`/workspace` PVC starts empty with no materialization (the v1 stub doesn't even interpret
instructions). Workspace file transfer is downstream of a control plane still in progress.

**Headline:** the **substrate and operated machinery** the converged plan leans on are
largely **already built** (NVMe pool, LVM-thin/TopoLVM/pquota, EBS+KMS, S3, OCI/Flux/ECR,
EKS 1.36/containerd 2.2.4). The **workspace-transfer logic itself is greenfield** in both
repos. This is a much better starting position for the _converged_ plan than for the
original B-first plan — because v0 and the same-node fork run on the dormant P4 substrate
rather than on a from-scratch chunk store.

### 7.2 Is "v1 as stated, then improve in v2" a waste risk?

Reading "v1 as stated" as the **zeroshot-rust epic #665 v1 scope** (Borrowed/Worktree/
Docker `WorkspaceLease`; explicit non-goals: _no Kubernetes, no remote workspace service,
no distributed leases, no snapshot/restore_) and "improve in v2" as adding the k8s
workspace-transfer plane. **Net assessment: low waste risk, by construction — provided
three seams are preserved. The convergence in §4.0 is what makes it low-risk; the original
B-first plan carried the real waste exposure.**

**Why it's low-waste:**

- The plan is **seam-preserving at every tier.** v0 (git), v1 (OCI digest), v2 (chunk
  manifest) are all **content-addressed digests behind one `ArtifactRef`/WorkspaceManifest
  backbone**, so the semantic model does not churn between phases — only the bytes engine
  behind the seam changes. #665 already reserves the seams: `WorkspaceLease` is a closed
  enum with `prepare/inspect/cleanup`, `ArtifactStore` (#699) has an explicit "inject an
  external CAS" seam, and #669 supplies the `ReadOnly|Exclusive` access token that the
  read-fork model needs.
- The **dormant P4 substrate is reused, not thrown away** — it becomes the v0 hot tier and
  the same-node COW-fork mechanism. (This is the inverse of what the B-first framing
  implied, where LVM-thin was "aligned with F, largely sunk.")
- The one **intentionally throwaway** piece is small: v0's tar.zst derived-cache
  packing/restore is replaced if/when the derived plane moves to OCI (v1). That is a few
  hundred lines shipped _specifically to earn the metric_ that decides v1 — the
  YAGNI-justified cost, not accidental waste.

**The real risks to manage (each has a concrete mitigation):**

1. **Collapsing the seams in v1 forces a v2 refactor.** If implementers flatten
   `WorkspaceLease` (drop the locator enum) or make `ArtifactStore` single-node-only "to
   ship faster," then adding the `Snapshot(digest)` locator + external CAS + remote runtime
   in v2 is a workspace-module rewrite. _Mitigation:_ treat the three seams above as
   load-bearing invariants in v1 review, even though v1 doesn't exercise them. This is the
   single most important thing to protect.
2. **v0 depends on _activating_ a deliberately-parked substrate.** The LVM-thin/NVMe path
   is dormant, non-gating, and its health checks explicitly do **not** select it; the beta
   runs on EBS pending the EBS-vs-local-NVMe benchmark the simplification proposal demanded
   (attach/start latency, workload I/O, cost, recovery). v0's hot-tier assumption is only
   valid once that benchmark clears. _Mitigation:_ run that benchmark as the first v0 task;
   if it favors EBS, the v0 hot-tier design changes (fall back to EBS gp3 for the node
   cache, which is already active).
3. **OCI-for-workspaces is a higher-churn regime than the deploy-OCI already operated.**
   "We already run signed OCI via Flux" is true for low-churn, cosign-verified _deploy_
   releases; per-(repo,branch,lockfile) _workspace_ base images are high-churn and hit ECR
   `PutImage` 10 req/s, layer-chain growth, and containerd GC pressure. _Mitigation:_ the
   unresolved ImageVolume+gVisor-on-EKS spike must land before committing v1 to OCI; until
   then v0's tar.zst derived cache carries the derived plane.
4. **The one thing that _would_ waste months is building B (the CAS) as v1** — the
   original doc position. `#699`'s "reuses the ArtifactStore CAS" is the easy ~10%
   (single-node filesystem CAS); the S3 chunk store + DaemonSet LRU + grace-period
   distributed GC + gVisor-aware materialization is the hard ~90%, greenfield, with a
   silent-corruption long tail (restic/kopia shipped GC/prune bugs for years on far simpler
   single-writer-locked repos). _Mitigation:_ the converged plan's core move — **B is
   metric-gated v2, not v1.** Do not pull it forward without the named metric.

**Bottom line for the phasing question:** implementing v1 as stated (zeroshot-rust's
isolation-mode workspaces) and adding the k8s transfer plane in v2 does **not** strand v1
work, _because the workspace abstraction and receipt model are stable across phases and the
heavy k8s substrate is already merged_. The waste exposure is almost entirely in one
avoidable decision — building the B chunk store speculatively — which the converged plan
explicitly defers. Keep the `WorkspaceLease`/`ArtifactStore`/`#669` seams open, run the
NVMe-vs-EBS and ImageVolume+gVisor benchmarks early, and the v1→v2 path is incremental
rather than a rewrite.
