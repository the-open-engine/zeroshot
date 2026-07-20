# capsule-workspace-core

Minimal, **interface-compatible** core of the capsule workspace storage data plane from
[`docs/specs/capsule-workspace-storage.md`](../docs/specs/capsule-workspace-storage.md).
Built to **measure and optimize now, drop into `zeroshot-cloud` later** — the interface
types mirror the existing control-plane contracts so the real daemon can replace this
without reshaping them.

Standalone Cargo workspace (own `[workspace]` table) — **not** part of the zeroshot root
workspace, so it never affects the main build.

## What it is

| Module     | Responsibility                                                                                                     | Real-system counterpart                                 |
| ---------- | ------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------- |
| `cas`      | 256 KiB chunks → sha256 → zstd → 64 MiB blocks; `BlobStore` trait + `LocalBlobStore`                               | the node daemon's chunk store + node-local cache tier   |
| `manifest` | immutable per-publish manifest (the commit point)                                                                  | `ArtifactRef`-addressed snapshot                        |
| `daemon`   | `publish()` (freeze→chunk→dedup→pack→upload→manifest) and `materialize()` (fetch→decompress→write), rayon-parallel | the `capsule-storage-node` DaemonSet                    |
| `lineage`  | fence-guarded HEAD CAS (single-writer)                                                                             | Postgres lineage row + `capsule_attempts.fencing_token` |
| `ifaces`   | the drop-in compatibility types                                                                                    | see mapping below                                       |

## Drop-in mapping onto zeroshot-cloud (verified 2026-07-20)

Everything in `src/ifaces.rs` mirrors an existing shape:

- `ClaimRole`, `ProviderNeutralClaim`, `ProviderNeutralClaims`
  ↔ `backend/services/orchestrator/src/capsule_control/types.rs`.
  The provisioning command is **unchanged**; only the provider behind the `Workspace`
  claim changes (EBS PVC → daemon-materialized snapshot).
- `ArtifactRef { sha256, byte_length }` ↔ the OECP receipt (bytes stay external).
- `Fence` ↔ the orchestrator's monotonic `current_fence` / `fencing_token`. Reused to
  guard lineage-HEAD advancement — this is what **replaces physical volume fencing** with a
  logical CAS (a second writer is rejected, not corrupting).
- `RecoveryStatusCompletedParts` ↔ `backend/crates/common/src/funded_run/recovery.rs`.
  Field names preserved; `workspace_pvc_uid` / `agent_state_pvc_uid` now carry content
  digests. **This is the one WS04 callback-contract change flagged in the spec (§15).**

### What must be built to make it the real daemon

1. `S3BlobStore` implementing `cas::BlobStore` (feature `s3`; stub wired, AWS SDK behind the
   flag). The trait boundary is the only change point.
2. `PgLineageStore` implementing `lineage::LineageStore` against the existing Postgres fence.
3. Wiring into `capsule-storage-node` as a DaemonSet: materialize after pod placement to a
   hostPath, `HostToContainer` bind-mount; publish on the `capsule-agent` barrier signal.
4. LVM thin snapshot for the O(1) freeze before a delta walk (measured O(1) already).

None of these touch `ifaces` — that is the point.

## Use

```sh
cargo build --release
# publish a tree (dedup against prior state), print stats, remember chunks for next publish
./target/release/capsule-workspace-core publish --tree <dir> --store <dir> --state idx.json
# materialize a manifest digest into a fresh tree
./target/release/capsule-workspace-core materialize --store <dir> --manifest <digest> --out <dir>
# one-shot publish+cold-materialize with timings
./target/release/capsule-workspace-core bench --tree <dir> --store <dir>
```

`LocalBlobStore` isolates the **CPU-bound** stages (chunk/hash/compress/decompress/write)
for throughput measurement; S3 network rates were measured separately (~350 MB/s fetch,
~1.09 GB/s parallel upload — see the experiment log).

## Status

Prototype. Correct (round-trip verified, dedup verified) and interface-compatible. Not
hardened: no grace-period GC, no hostile-tree traversal defense, no per-org key scoping,
no crash-atomicity beyond write-then-rename. Those are spec §11/§13 work.
