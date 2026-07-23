//! Deterministic admission fixtures. These types script verifier assertions and admission state;
//! they are not a native graph verifier or production executor.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{
    ApplyResult, CompiledGraphIr, Cursor, DispatchState, Generation, GraphDiagnostic, GraphSpec,
    IdempotencyKey, OperationalStatus, Phase, RunId,
};
use openengine_cluster_server::admission::{
    AdmissionSnapshot, AdmissionStore, CancellationSignal, CommitProposal, ControlJournal,
    ControlSnapshot, GraphVerifier, IdempotencyRecord, StoreError, VerificationError,
    VerifiedGraph, VerifiedIoLedger, VerifiedSeed,
};
use openengine_cluster_server::lifecycle::{LeaseId, LifecycleSnapshot, MutationReceipt, TurnId};
use tokio::sync::{Mutex, Notify};

mod fixtures;
mod inspection;
pub use fixtures::*;
pub use inspection::StoreInspection;

use crate::watch::ObservationState;

#[derive(Clone, Debug)]
pub enum ScriptedOutcome {
    Approve {
        compiled_ir: Box<CompiledGraphIr>,
        diagnostics: Vec<GraphDiagnostic>,
    },
    Reject {
        diagnostics: Vec<GraphDiagnostic>,
    },
    Park {
        barrier: VerifierBarrier,
        then: Box<ScriptedOutcome>,
    },
    Fail {
        message: String,
    },
}

impl ScriptedOutcome {
    #[must_use]
    pub fn approve(compiled_ir: CompiledGraphIr, diagnostics: Vec<GraphDiagnostic>) -> Self {
        Self::Approve {
            compiled_ir: Box::new(compiled_ir),
            diagnostics,
        }
    }

    #[must_use]
    pub fn reject(diagnostics: Vec<GraphDiagnostic>) -> Self {
        Self::Reject { diagnostics }
    }

    #[must_use]
    pub fn park(barrier: VerifierBarrier, then: Self) -> Self {
        Self::Park {
            barrier,
            then: Box::new(then),
        }
    }

    #[must_use]
    pub fn fail(message: impl Into<String>) -> Self {
        Self::Fail {
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct VerifierBarrier {
    inner: Arc<VerifierBarrierInner>,
}

#[derive(Debug, Default)]
struct VerifierBarrierInner {
    entered: AtomicBool,
    released: AtomicBool,
    entered_notify: Notify,
    released_notify: Notify,
}

impl VerifierBarrier {
    pub async fn wait_until_entered(&self) {
        while !self.inner.entered.load(Ordering::Acquire) {
            self.inner.entered_notify.notified().await;
        }
    }

    pub fn release(&self) {
        self.inner.released.store(true, Ordering::Release);
        self.inner.released_notify.notify_waiters();
    }

    async fn park(&self) {
        self.inner.entered.store(true, Ordering::Release);
        self.inner.entered_notify.notify_waiters();
        while !self.inner.released.load(Ordering::Acquire) {
            self.inner.released_notify.notified().await;
        }
    }
}

#[derive(Debug)]
pub struct ScriptedVerifier {
    outcomes: Mutex<VecDeque<ScriptedOutcome>>,
    calls: AtomicUsize,
}

impl ScriptedVerifier {
    #[must_use]
    pub fn new(outcomes: Vec<ScriptedOutcome>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into()),
            calls: AtomicUsize::new(0),
        }
    }

    #[must_use]
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::Acquire)
    }
}

#[async_trait]
impl GraphVerifier for ScriptedVerifier {
    async fn verify(&self, _graph: &GraphSpec) -> Result<VerifiedGraph, VerificationError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        let mut outcome = self.outcomes.lock().await.pop_front().ok_or_else(|| {
            VerificationError::Internal("scripted verifier queue exhausted".into())
        })?;
        loop {
            match outcome {
                ScriptedOutcome::Approve {
                    compiled_ir,
                    diagnostics,
                } => {
                    return Ok(VerifiedGraph {
                        compiled_ir: *compiled_ir,
                        diagnostics,
                    });
                }
                ScriptedOutcome::Reject { diagnostics } => {
                    return Err(VerificationError::Rejected { diagnostics });
                }
                ScriptedOutcome::Park { barrier, then } => {
                    barrier.park().await;
                    outcome = *then;
                }
                ScriptedOutcome::Fail { message } => {
                    return Err(VerificationError::Internal(message));
                }
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppendKind {
    Control,
    VerifiedSeed,
    Idempotency,
    Lifecycle,
    VerifiedOutput,
    Void,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppendReceipt {
    pub sequence: u64,
    pub cursor: Option<Cursor>,
    pub kind: AppendKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ControlReceipt {
    pub generation: Generation,
    pub run_id: RunId,
    pub cursor: Cursor,
    pub spec: GraphSpec,
}

#[derive(Debug, Default)]
pub struct InMemoryAdmissionStore {
    pub(crate) state: Mutex<StoreState>,
}

#[derive(Debug, Default)]
pub(crate) struct StoreState {
    pub(crate) control: ControlSnapshot,
    pub(crate) control_journal: Vec<ControlReceipt>,
    pub(crate) seed_ledger: Vec<VerifiedSeed>,
    pub(crate) idempotency_records: BTreeMap<IdempotencyKey, IdempotencyRecord>,
    pub(crate) append_order: Vec<AppendReceipt>,
    next_sequence: u64,
    next_run: u64,
    pub(crate) next_cursor: u64,
    pub(crate) lifecycle: LifecycleSnapshot,
    pub(crate) leases: BTreeMap<LeaseId, ActiveLease>,
    pub(crate) cancelled_leases: BTreeSet<LeaseId>,
    pub(crate) next_lease: u64,
    pub(crate) observation: ObservationState,
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveLease {
    pub(crate) turn_id: TurnId,
    pub(crate) cancellation: CancellationSignal,
}

impl StoreState {
    fn commit(
        &mut self,
        proposal: CommitProposal,
        cancellation: &CancellationSignal,
    ) -> Result<ApplyResult, StoreError> {
        if let Some(receipt) = self.replay(&proposal)? {
            return Ok(receipt);
        }
        enforce_generation(proposal.if_generation, self.control.generation)?;
        if !matches!(self.control.phase, Phase::Empty | Phase::Running) {
            return Err(StoreError::InvalidPhase {
                current: self.control.phase,
            });
        }
        let unchanged = self.is_unchanged(&proposal.compiled_ir)?;
        if self.control.phase == Phase::Running {
            let dispatch_state = self
                .lifecycle
                .operational
                .as_ref()
                .ok_or_else(|| StoreError::Internal("running lifecycle metadata is absent".into()))?
                .dispatch_state;
            if dispatch_state != DispatchState::Active || (!unchanged && !self.leases.is_empty()) {
                return Err(StoreError::InvalidPhase {
                    current: self.control.phase,
                });
            }
        }
        validate_commit_input(&proposal, unchanged)?;
        if cancellation.is_cancelled() {
            return Err(StoreError::Cancelled);
        }
        let result = if unchanged {
            self.unchanged_receipt()
        } else {
            self.changed_receipt(&proposal)?
        };
        self.record_idempotency(proposal, &result);
        Ok(result)
    }

    fn replay(&self, proposal: &CommitProposal) -> Result<Option<ApplyResult>, StoreError> {
        let Some(existing) = self.idempotency_records.get(&proposal.idempotency_key) else {
            return Ok(None);
        };
        if existing.fingerprint != proposal.fingerprint {
            return Err(StoreError::IdempotencyReuse);
        }
        let MutationReceipt::Apply(mut receipt) = existing.receipt.clone() else {
            return Err(StoreError::IdempotencyReuse);
        };
        receipt.deduped = true;
        Ok(Some(receipt))
    }

    fn is_unchanged(&self, desired: &CompiledGraphIr) -> Result<bool, StoreError> {
        self.control
            .compiled_ir
            .as_ref()
            .map(|current| {
                Ok(current
                    .identity()
                    .map_err(|error| StoreError::Internal(error.to_string()))?
                    == desired
                        .identity()
                        .map_err(|error| StoreError::Internal(error.to_string()))?)
            })
            .transpose()
            .map(Option::unwrap_or_default)
    }

    fn unchanged_receipt(&self) -> ApplyResult {
        ApplyResult {
            generation: self.control.generation,
            run_id: self.control.run_id.clone(),
            phase: self.control.phase,
            deduped: false,
            diff: None,
        }
    }

    fn changed_receipt(&mut self, proposal: &CommitProposal) -> Result<ApplyResult, StoreError> {
        self.next_run += 1;
        self.next_cursor += 1;
        let generation_value = self
            .control
            .generation
            .map_or(1, |generation| generation.get() + 1);
        let generation = Generation::new(generation_value)
            .map_err(|error| StoreError::Internal(error.to_string()))?;
        let run_id = RunId::new(format!("run-{}", self.next_run));
        let cursor = Cursor::new(format!("cursor-{}", self.next_cursor));
        let input = proposal
            .input
            .clone()
            .expect("changed admission validated required input");
        self.control = ControlSnapshot {
            spec: Some(proposal.graph.clone()),
            compiled_ir: Some(proposal.compiled_ir.clone()),
            generation: Some(generation),
            run_id: Some(run_id.clone()),
            phase: Phase::Running,
            cursor: Some(cursor.clone()),
        };
        for lease in self.leases.values() {
            lease.cancellation.cancel();
        }
        self.leases.clear();
        self.cancelled_leases.clear();
        self.lifecycle = LifecycleSnapshot {
            operational: Some(OperationalStatus::default()),
            latest_cursor: Some(cursor.clone()),
            records: Vec::new(),
            verified_turns: Vec::new(),
            void_turns: Vec::new(),
        };
        self.control_journal.push(ControlReceipt {
            generation,
            run_id: run_id.clone(),
            cursor: cursor.clone(),
            spec: proposal.graph.clone(),
        });
        append(self, Some(cursor.clone()), AppendKind::Control);
        self.seed_ledger.push(VerifiedSeed {
            run_id: run_id.clone(),
            input: input.clone(),
            cursor: cursor.clone(),
        });
        append(self, Some(cursor.clone()), AppendKind::VerifiedSeed);
        let status = self.control.status_with_lifecycle(&self.lifecycle);
        self.record_public_event(
            &run_id,
            cursor,
            openengine_cluster_protocol::WatchEvent::Phase {
                status,
                admission: Some(Box::new(openengine_cluster_protocol::AdmissionTransition {
                    run_id: run_id.clone(),
                    spec: proposal.graph.clone(),
                    seed_input: input,
                })),
            },
        );
        Ok(ApplyResult {
            generation: Some(generation),
            run_id: Some(run_id),
            phase: Phase::Running,
            deduped: false,
            diff: None,
        })
    }

    fn record_idempotency(&mut self, proposal: CommitProposal, result: &ApplyResult) {
        self.idempotency_records.insert(
            proposal.idempotency_key,
            IdempotencyRecord {
                fingerprint: proposal.fingerprint,
                receipt: MutationReceipt::Apply(result.clone()),
            },
        );
        append(self, self.control.cursor.clone(), AppendKind::Idempotency);
    }
}

fn validate_commit_input(proposal: &CommitProposal, unchanged: bool) -> Result<(), StoreError> {
    if unchanged {
        return if proposal.input.is_none() {
            Ok(())
        } else {
            Err(StoreError::SchemaViolation(
                "unchanged apply must omit input; use future resubmit semantics".into(),
            ))
        };
    }
    let input = proposal.input.as_ref().ok_or_else(|| {
        StoreError::SchemaViolation("apply that starts a run requires input".into())
    })?;
    proposal
        .graph
        .initial_input
        .validate_value(input)
        .map_err(|error| StoreError::SchemaViolation(error.to_string()))
}

#[async_trait]
impl ControlJournal for InMemoryAdmissionStore {
    async fn read_control(&self) -> Result<ControlSnapshot, StoreError> {
        Ok(self.state.lock().await.control.clone())
    }

    async fn lookup_idempotency(
        &self,
        key: &IdempotencyKey,
    ) -> Result<Option<IdempotencyRecord>, StoreError> {
        Ok(self
            .state
            .lock()
            .await
            .idempotency_records
            .get(key)
            .cloned())
    }
}

#[async_trait]
impl VerifiedIoLedger for InMemoryAdmissionStore {
    async fn read_verified_seed(&self, run_id: &RunId) -> Result<Option<VerifiedSeed>, StoreError> {
        Ok(self
            .state
            .lock()
            .await
            .seed_ledger
            .iter()
            .rev()
            .find(|seed| seed.run_id == *run_id)
            .cloned())
    }
}

#[async_trait]
impl AdmissionStore for InMemoryAdmissionStore {
    async fn read_snapshot(&self) -> Result<AdmissionSnapshot, StoreError> {
        let state = self.state.lock().await;
        Ok(admission_snapshot(&state))
    }

    async fn read_aggregate(&self) -> Result<(AdmissionSnapshot, LifecycleSnapshot), StoreError> {
        let state = self.state.lock().await;
        Ok((admission_snapshot(&state), state.lifecycle.clone()))
    }

    async fn commit(
        &self,
        proposal: CommitProposal,
        cancellation: &CancellationSignal,
    ) -> Result<ApplyResult, StoreError> {
        self.state.lock().await.commit(proposal, cancellation)
    }
}

fn admission_snapshot(state: &StoreState) -> AdmissionSnapshot {
    let seed = state.control.run_id.as_ref().and_then(|run_id| {
        state
            .seed_ledger
            .iter()
            .rev()
            .find(|seed| seed.run_id == *run_id)
            .cloned()
    });
    AdmissionSnapshot {
        control: state.control.clone(),
        seed,
    }
}

pub(crate) fn append(state: &mut StoreState, cursor: Option<Cursor>, kind: AppendKind) {
    state.next_sequence += 1;
    state.append_order.push(AppendReceipt {
        sequence: state.next_sequence,
        cursor,
        kind,
    });
}

pub(crate) fn enforce_generation(
    expected: Option<Generation>,
    current: Option<Generation>,
) -> Result<(), StoreError> {
    let matches = match expected {
        None => true,
        Some(expected) if expected.get() == 0 => current.is_none(),
        Some(expected) => current == Some(expected),
    };
    if matches {
        Ok(())
    } else {
        Err(StoreError::GenerationConflict { current })
    }
}
