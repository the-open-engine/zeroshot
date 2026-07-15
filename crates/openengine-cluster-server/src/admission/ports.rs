//! Verifier, journal, verified-I/O, cancellation, and atomic aggregate ports.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{
    ApplyResult, ClusterStatus, CompiledGraphIr, Cursor, Generation, GraphDiagnostic, GraphSpec,
    IdempotencyKey, Phase, RequestFingerprint, RunId,
};
use serde_json::Value;
use thiserror::Error;

#[derive(Clone, Default)]
pub struct CancellationSignal(Arc<AtomicBool>);

impl CancellationSignal {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl fmt::Debug for CancellationSignal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CancellationSignal")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl PartialEq for CancellationSignal {
    fn eq(&self, other: &Self) -> bool {
        self.is_cancelled() == other.is_cancelled()
    }
}

impl Eq for CancellationSignal {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedGraph {
    pub compiled_ir: CompiledGraphIr,
    pub diagnostics: Vec<GraphDiagnostic>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum VerificationError {
    #[error("graph verification rejected the graph")]
    Rejected { diagnostics: Vec<GraphDiagnostic> },
    #[error("graph verifier failed internally: {0}")]
    Internal(String),
}

#[async_trait]
pub trait GraphVerifier: Send + Sync + 'static {
    async fn verify(&self, graph: &GraphSpec) -> Result<VerifiedGraph, VerificationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlSnapshot {
    pub spec: Option<GraphSpec>,
    pub compiled_ir: Option<CompiledGraphIr>,
    pub generation: Option<Generation>,
    pub run_id: Option<RunId>,
    pub phase: Phase,
    pub cursor: Option<Cursor>,
}

impl Default for ControlSnapshot {
    fn default() -> Self {
        Self {
            spec: None,
            compiled_ir: None,
            generation: None,
            run_id: None,
            phase: Phase::Empty,
            cursor: None,
        }
    }
}

impl ControlSnapshot {
    #[must_use]
    pub fn status(&self) -> ClusterStatus {
        ClusterStatus {
            phase: self.phase,
            observed_generation: self.generation,
            current_run_id: self.run_id.clone(),
            at_cursor: self.cursor.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedSeed {
    pub run_id: RunId,
    pub input: Value,
    pub cursor: Cursor,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AdmissionSnapshot {
    pub control: ControlSnapshot,
    pub seed: Option<VerifiedSeed>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdempotencyRecord {
    pub fingerprint: RequestFingerprint,
    pub receipt: ApplyResult,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommitProposal {
    pub graph: GraphSpec,
    pub compiled_ir: CompiledGraphIr,
    pub input: Option<Value>,
    pub if_generation: Option<Generation>,
    pub idempotency_key: IdempotencyKey,
    pub fingerprint: RequestFingerprint,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum StoreError {
    #[error("admission store failed: {0}")]
    Internal(String),
    #[error("idempotency key was already used with different parameters")]
    IdempotencyReuse,
    #[error("generation precondition failed")]
    GenerationConflict { current: Option<Generation> },
    #[error("cluster phase does not admit apply: {current:?}")]
    InvalidPhase { current: Phase },
    #[error("admission parameters violate the run-input contract: {0}")]
    SchemaViolation(String),
    #[error("admission was cancelled before atomic commit")]
    Cancelled,
}

/// Logical durable control-journal view.
#[async_trait]
pub trait ControlJournal: Send + Sync {
    async fn read_control(&self) -> Result<ControlSnapshot, StoreError>;
    async fn lookup_idempotency(
        &self,
        key: &IdempotencyKey,
    ) -> Result<Option<IdempotencyRecord>, StoreError>;
}

/// Logical durable verified-I/O view. Only verified root seed input exists in this slice.
#[async_trait]
pub trait VerifiedIoLedger: Send + Sync {
    async fn read_verified_seed(&self, run_id: &RunId) -> Result<Option<VerifiedSeed>, StoreError>;
}

/// Atomic aggregate port. Implementations allocate one ordering across both logical stores and
/// check cancellation immediately before writing any effect.
#[async_trait]
pub trait AdmissionStore: ControlJournal + VerifiedIoLedger + Send + Sync + 'static {
    async fn read_snapshot(&self) -> Result<AdmissionSnapshot, StoreError>;
    async fn commit(
        &self,
        proposal: CommitProposal,
        cancellation: &CancellationSignal,
    ) -> Result<ApplyResult, StoreError>;
}
