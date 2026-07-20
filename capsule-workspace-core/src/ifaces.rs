//! Compatibility interfaces — the drop-in seam onto zeroshot-cloud.
//!
//! Every type here mirrors an existing shape in `zeroshot-cloud` (verified 2026-07-20) so
//! this core can replace the workspace data plane without reshaping the control-plane
//! contracts. Where the new design extends a contract, the extension is additive.
//!
//! Mapping:
//! - `ClaimRole`, `ProviderNeutralClaim`, `ProviderNeutralClaims`
//!     == `backend/services/orchestrator/src/capsule_control/types.rs`
//!   The provisioning command still carries provider-neutral claims; the storage provider
//!   behind the `Workspace` claim changes from an EBS PVC to a daemon-materialized snapshot.
//! - `ArtifactRef` == the OECP `ArtifactRef` receipt (sha256 + byte_length; bytes external).
//!   A published workspace snapshot is surfaced as one of these.
//! - `Fence` == the monotonic `current_fence` already persisted as
//!   `capsule_attempts.fencing_token` and enforced in `capsule_control.rs`. Reused here to
//!   guard lineage-HEAD advancement (single-writer) instead of physical volume fencing.
//! - `RecoveryStatusCompletedParts` mirrors `backend/crates/common/src/funded_run/recovery.rs`.
//!   The two `*_pvc_uid` fields are reinterpreted as content identities (see WS04 note).

use serde::{Deserialize, Serialize};

/// Mirrors `capsule_control::types::ClaimRole`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClaimRole {
    Workspace,
    AgentState,
}

/// Mirrors `capsule_control::types::ProviderNeutralClaim`. `fake_claim_uid` is retained for
/// wire compatibility; the daemon binds it to the workspace lineage instead of a PVC UID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderNeutralClaim {
    pub role: ClaimRole,
    pub claim_name: String,
    pub fake_claim_uid: String,
}

/// Mirrors `capsule_control::types::ProviderNeutralClaims`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderNeutralClaims {
    pub workspace: ProviderNeutralClaim,
    pub agent_state: ProviderNeutralClaim,
}

/// OECP `ArtifactRef` receipt shape — a published snapshot is byte-addressed; bytes stay
/// external (in the block store). This is what an agent hands a reader over the bus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub sha256: String,
    pub byte_length: u64,
}

/// Monotonic fence. Mirrors the orchestrator's `current_fence` / `fencing_token`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Fence(pub u64);

/// Stable identity of a workspace lineage (one project's evolving state). The manifest
/// digest is the point-in-time snapshot; the lineage HEAD (guarded by `Fence`) is mutable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LineageId(pub String);

/// Additive extension carried alongside the existing recovery callback. `manifest_digest`
/// is the durable point-in-time snapshot the replacement runtime materializes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSnapshotRef {
    pub lineage_id: LineageId,
    pub manifest_digest: String,
    pub fence: Fence,
}

/// Mirrors `funded_run::recovery::RecoveryStatusCompletedParts`. Field names are preserved;
/// `workspace_pvc_uid` now carries the workspace manifest digest and `agent_state_pvc_uid`
/// the agent-state snapshot digest (WS04 callback contract change — flagged in the spec).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryStatusCompletedParts {
    pub runtime_generation: u64,
    pub source_attempt_number: u32,
    pub target_attempt_number: u32,
    pub workspace_pvc_uid: String,
    pub agent_state_pvc_uid: String,
}
