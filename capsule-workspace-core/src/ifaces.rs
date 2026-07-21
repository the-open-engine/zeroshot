//! Compatibility interfaces — the drop-in seam onto zeroshot-cloud.
//!
//! HONESTY NOTE (corrected 2026-07-21, research finding C): an earlier header claimed "every type
//! here mirrors an existing shape in zeroshot-cloud (verified)". That overstated the mapping. The
//! types here fall into TWO categories, now labeled per-type:
//!
//! LITERAL MIRRORS — a byte/field-for-field shape that exists verbatim in zeroshot-cloud:
//!   - `Fence` ⟷ the monotonic `current_fence` persisted as `capsule_attempts.fencing_token` and
//!     enforced in `capsule_control.rs`. Reused here to guard lineage-HEAD advancement
//!     (single-writer CAS) instead of physical volume fencing.
//!   - `RecoveryStatusCompletedParts` ⟷ `common::funded_run::recovery::RecoveryStatusCompletedParts`
//!     (all fields restored below for a true drop-in — see the struct doc).
//!
//! NEUTRAL GENERALIZATIONS — an abstraction seam of OUR design that does NOT exist verbatim in
//! zeroshot-cloud (the real shapes are noted so nobody mistakes these for literal mirrors):
//!   - `ClaimRole`, `ProviderNeutralClaim`, `ProviderNeutralClaims` — the real orchestrator shapes
//!     are `PersistentVolumeClaimIntent` / `PersistentVolumeClaims` + `BoundClaim { pvc_uid,
//!     availability_zone }`. Ours is a provider-neutral rename so the storage provider behind the
//!     `Workspace` claim can be a daemon-materialized snapshot instead of an EBS PVC.
//!   - `ArtifactRef` — NOT a struct in zeroshot-cloud; it is the SHAPE of the sidecar
//!     `RunnerToAgent::Result` receipt (sha256 + byte_length). Modeled here as a struct for the seam.
//!   - `LineageId`, `WorkspaceSnapshotRef` — our own additive types (no upstream counterpart).

use serde::{Deserialize, Serialize};

/// NEUTRAL GENERALIZATION (not a literal mirror). Provider-neutral rename of the orchestrator's
/// claim-role concept; upstream distinguishes claims by their `PersistentVolumeClaimIntent`, not a
/// `ClaimRole` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClaimRole {
    Workspace,
    AgentState,
}

/// NEUTRAL GENERALIZATION (not a literal mirror). Our provider-neutral stand-in for the
/// orchestrator's `PersistentVolumeClaimIntent` + `BoundClaim { pvc_uid, availability_zone }`.
/// `fake_claim_uid` is the seam field the daemon binds to the workspace lineage instead of a PVC UID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderNeutralClaim {
    pub role: ClaimRole,
    pub claim_name: String,
    pub fake_claim_uid: String,
}

/// NEUTRAL GENERALIZATION (not a literal mirror). Pairs the two provider-neutral claims; upstream
/// carries `PersistentVolumeClaims`, not this shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderNeutralClaims {
    pub workspace: ProviderNeutralClaim,
    pub agent_state: ProviderNeutralClaim,
}

/// NEUTRAL GENERALIZATION (not a literal mirror). `ArtifactRef` is NOT a struct in zeroshot-cloud;
/// this models the SHAPE of the sidecar `RunnerToAgent::Result` receipt (sha256 + byte_length, bytes
/// external in the block store) — what an agent hands a reader over the bus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub sha256: String,
    pub byte_length: u64,
}

/// LITERAL MIRROR of the orchestrator's monotonic `current_fence` / `capsule_attempts.fencing_token`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Fence(pub u64);

/// OUR OWN additive type (no upstream counterpart). Stable identity of a workspace lineage (one
/// project's evolving state). The manifest digest is the point-in-time snapshot; the lineage HEAD
/// (guarded by `Fence`) is mutable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LineageId(pub String);

/// OUR OWN additive type (no upstream counterpart). Carried alongside the existing recovery callback.
/// `manifest_digest` is the durable point-in-time snapshot the replacement runtime materializes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSnapshotRef {
    pub lineage_id: LineageId,
    pub manifest_digest: String,
    pub fence: Fence,
}

/// LITERAL MIRROR of `common::funded_run::recovery::RecoveryStatusCompletedParts`. All upstream
/// fields are carried so this is a true drop-in (an earlier prototype dropped `run_id`,
/// `reservation_id`, and `fence` — restored here, research finding B mismatch #3). Field names are
/// preserved; `workspace_pvc_uid` now carries the workspace manifest digest and `agent_state_pvc_uid`
/// the agent-state snapshot digest (WS04 callback contract change — flagged in the spec). This type is
/// part of the compatibility seam and is not otherwise exercised by the standalone core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryStatusCompletedParts {
    /// The funded run this recovery belongs to (a UUID rendered as a String upstream).
    pub run_id: String,
    /// The capacity reservation the replacement runtime binds under.
    pub reservation_id: String,
    /// The monotonic fence the recovery completed at (single-writer guard).
    pub fence: Fence,
    pub runtime_generation: u64,
    pub source_attempt_number: u32,
    pub target_attempt_number: u32,
    pub workspace_pvc_uid: String,
    pub agent_state_pvc_uid: String,
}
