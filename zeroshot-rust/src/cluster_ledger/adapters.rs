//! Protocol store adapters backed by one coherent ordered-prefix fold.

use async_trait::async_trait;
use openengine_cluster_protocol::{
    canonical_value_bytes, ApplyResult, CompiledGraphIr, Cursor, Generation, GraphSpec, Phase,
    RequestFingerprint, RunId,
};
use openengine_cluster_server::admission::{
    AdmissionSnapshot, AdmissionStore, CancellationSignal, CommitProposal, ControlJournal,
    ControlSnapshot, IdempotencyRecord, StoreError as ProtocolStoreError, VerifiedIoLedger,
    VerifiedSeed,
};
use openengine_cluster_server::lifecycle::{
    CompletionResult, DispatchPermit, LifecycleSnapshot, LifecycleStore,
    MutationReceipt as ProtocolMutationReceipt, StopProposal, UpdateProposal, VerifiedCompletion,
};
use openengine_cluster_protocol::{StopResult, UpdateResult};

use super::record::{CanonicalDigest, RecordPayload};
use super::store::{AppendGuard, IdempotencyId, MutationReceipt, StoreError};
use super::{ClusterLedger, LedgerError, LedgerErrorKind, MutationIdentity};

#[derive(Clone)]
pub struct ClusterLedgerAdapters {
    ledger: ClusterLedger,
}

impl ClusterLedgerAdapters {
    #[must_use]
    pub const fn new(ledger: ClusterLedger) -> Self {
        Self { ledger }
    }

    #[must_use]
    pub const fn ledger(&self) -> &ClusterLedger {
        &self.ledger
    }

    async fn folded(&self) -> Result<FoldedProtocolState, ProtocolStoreError> {
        let state = self.ledger.state().await.map_err(protocol_error)?;
        FoldedProtocolState::from_replay(&state)
    }

    fn existing_protocol_apply(
        &self,
        state: &super::ReplayState,
        key: &IdempotencyId,
        fingerprint: [u8; 32],
    ) -> Result<Option<ApplyResult>, ProtocolStoreError> {
        self.ledger
            .existing_receipt::<ApplyResult>(
                crate::fault::FaultContext::Admission,
                state,
                key,
                "protocol_apply",
                fingerprint,
            )
            .map(|existing| {
                existing.map(|existing| {
                    let mut result = existing.value;
                    result.deduped = true;
                    result
                })
            })
            .map_err(protocol_error)
    }

    async fn commit_unchanged(
        &self,
        state: &super::ReplayState,
        proposal: CommitProposal,
        key: IdempotencyId,
        fingerprint: [u8; 32],
        generation: Option<Generation>,
        cancellation: &CancellationSignal,
    ) -> Result<ApplyResult, ProtocolStoreError> {
        if proposal.input.is_some() {
            return Err(ProtocolStoreError::SchemaViolation(
                "unchanged admission cannot replace verified input".into(),
            ));
        }
        let current = state
            .admission
            .as_ref()
            .expect("unchanged admission requires current state");
        let result = ApplyResult {
            generation,
            run_id: Some(protocol_run_id(current.run)),
            phase: Phase::Running,
            deduped: false,
            diff: None,
        };
        if cancellation.is_cancelled() {
            return Err(ProtocolStoreError::Cancelled);
        }
        self.commit_protocol_apply(state, key, fingerprint, Vec::new(), result, cancellation)
            .await
    }

    async fn commit_changed(
        &self,
        state: &mut super::ReplayState,
        proposal: CommitProposal,
        key: IdempotencyId,
        fingerprint: [u8; 32],
        canonical_compiled_ir: Vec<u8>,
        cancellation: &CancellationSignal,
    ) -> Result<ApplyResult, ProtocolStoreError> {
        if !state.active_dispatches.is_empty()
            || state
                .effects
                .values()
                .any(|effect| effect.receipt_digest.is_none())
        {
            return Err(ProtocolStoreError::InvalidPhase {
                current: Phase::Running,
            });
        }
        let changed = prepare_changed_apply(state, proposal, canonical_compiled_ir)?;
        if cancellation.is_cancelled() {
            return Err(ProtocolStoreError::Cancelled);
        }
        self.commit_protocol_apply(
            state,
            key,
            fingerprint,
            changed.payloads,
            changed.result,
            cancellation,
        )
        .await
    }

    async fn commit_protocol_apply(
        &self,
        state: &super::ReplayState,
        key: IdempotencyId,
        fingerprint: [u8; 32],
        payloads: Vec<RecordPayload>,
        result: ApplyResult,
        cancellation: &CancellationSignal,
    ) -> Result<ApplyResult, ProtocolStoreError> {
        let committed = self
            .ledger
            .commit_guarded(
                crate::fault::FaultContext::Admission,
                state,
                MutationIdentity::new(key, "protocol_apply", fingerprint),
                payloads,
                &result,
                cancellation_guard(cancellation),
            )
            .await
            .map_err(protocol_error)?;
        let mut value = committed.value;
        value.deduped = committed.replayed;
        Ok(value)
    }
}

struct ChangedApply {
    result: ApplyResult,
    payloads: Vec<RecordPayload>,
}

fn prepare_changed_apply(
    state: &mut super::ReplayState,
    proposal: CommitProposal,
    canonical_compiled_ir: Vec<u8>,
) -> Result<ChangedApply, ProtocolStoreError> {
    let generation = state
        .identities
        .allocate_generation()
        .map_err(|_| ProtocolStoreError::Internal("generation allocation failed".into()))?;
    let run = state
        .identities
        .allocate_run()
        .map_err(|_| ProtocolStoreError::Internal("run allocation failed".into()))?;
    let canonical_graph = serde_json::to_vec(&proposal.graph)
        .map_err(|_| ProtocolStoreError::Internal("graph encoding failed".into()))?;
    let canonical_input = canonical_value_bytes(proposal.input.as_ref().ok_or_else(|| {
        ProtocolStoreError::SchemaViolation("changed admission requires verified input".into())
    })?)
    .map_err(|_| ProtocolStoreError::SchemaViolation("input is not canonical".into()))?;
    let input_digest = CanonicalDigest::of(&canonical_input);
    let payloads = changed_apply_payloads(
        &proposal,
        generation,
        run,
        canonical_graph,
        canonical_input,
        input_digest,
        canonical_compiled_ir,
    );
    Ok(ChangedApply {
        result: ApplyResult {
            generation: Some(Generation::new(generation.get()).map_err(|_| {
                ProtocolStoreError::Internal("generation exceeds protocol range".into())
            })?),
            run_id: Some(protocol_run_id(run)),
            phase: Phase::Running,
            deduped: false,
            diff: None,
        },
        payloads,
    })
}

fn changed_apply_payloads(
    proposal: &CommitProposal,
    generation: super::GenerationId,
    run: super::RunSequence,
    canonical_graph: Vec<u8>,
    canonical_input: Vec<u8>,
    input_digest: CanonicalDigest,
    canonical_compiled_ir: Vec<u8>,
) -> Vec<RecordPayload> {
    vec![
        RecordPayload::Admission {
            generation,
            run,
            graph_digest: CanonicalDigest::of(&canonical_graph),
            input_digest,
            policy_digest: CanonicalDigest::of(&canonical_compiled_ir),
            catalog_digest: CanonicalDigest::of(b"worker-catalog/v1"),
            profile_digest: CanonicalDigest::of(proposal.compiled_ir.profile.as_str().as_bytes()),
            absolute_deadline_ms: u64::MAX,
            canonical_graph,
            canonical_compiled_ir,
        },
        RecordPayload::VerifiedInput {
            run,
            digest: input_digest,
            canonical_bytes: canonical_input,
        },
    ]
}

fn current_protocol_generation(
    state: &super::ReplayState,
) -> Result<Option<Generation>, ProtocolStoreError> {
    state
        .admission
        .as_ref()
        .map(|admission| {
            Generation::new(admission.generation.get())
                .map_err(|_| ProtocolStoreError::Internal("durable generation is invalid".into()))
        })
        .transpose()
}

fn validate_protocol_generation(
    expected: Option<Generation>,
    current: Option<Generation>,
) -> Result<(), ProtocolStoreError> {
    let matches = match expected {
        None => true,
        Some(expected) if expected.get() == 0 => current.is_none(),
        Some(expected) => current == Some(expected),
    };
    if matches {
        Ok(())
    } else {
        Err(ProtocolStoreError::GenerationConflict { current })
    }
}

fn ensure_apply_phase(state: &super::ReplayState) -> Result<(), ProtocolStoreError> {
    if state.terminal_outcome.is_some() {
        Err(ProtocolStoreError::InvalidPhase {
            current: Phase::Finished,
        })
    } else {
        Ok(())
    }
}

struct FoldedProtocolState {
    admission: AdmissionSnapshot,
    lifecycle: LifecycleSnapshot,
}

impl FoldedProtocolState {
    fn from_replay(state: &super::ReplayState) -> Result<Self, ProtocolStoreError> {
        let Some(admission) = &state.admission else {
            return Ok(Self {
                admission: AdmissionSnapshot::default(),
                lifecycle: LifecycleSnapshot::default(),
            });
        };
        let generation = Generation::new(admission.generation.get())
            .map_err(|_| ProtocolStoreError::Internal("durable generation is invalid".into()))?;
        let run_id = protocol_run_id(admission.run);
        let cursor = protocol_cursor(state.position);
        let phase = if state.terminal_outcome.is_some() {
            Phase::Finished
        } else {
            Phase::Running
        };
        let control = ControlSnapshot {
            spec: decode_graph(&admission.canonical_graph)?,
            compiled_ir: decode_compiled_ir(&admission.canonical_compiled_ir)?,
            generation: Some(generation),
            run_id: Some(run_id.clone()),
            phase,
            cursor: Some(cursor.clone()),
        };
        Ok(Self {
            admission: AdmissionSnapshot {
                control,
                seed: verified_seed(state, admission.run, run_id)?,
            },
            lifecycle: lifecycle_snapshot(state, cursor)?,
        })
    }
}

fn decode_graph(bytes: &[u8]) -> Result<Option<GraphSpec>, ProtocolStoreError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    serde_json::from_slice(bytes)
        .map(Some)
        .map_err(|_| ProtocolStoreError::Internal("durable graph encoding is invalid".into()))
}

fn decode_compiled_ir(bytes: &[u8]) -> Result<Option<CompiledGraphIr>, ProtocolStoreError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    serde_json::from_slice(bytes).map(Some).map_err(|_| {
        ProtocolStoreError::Internal("durable compiled graph encoding is invalid".into())
    })
}

fn verified_seed(
    state: &super::ReplayState,
    run: super::RunSequence,
    run_id: RunId,
) -> Result<Option<VerifiedSeed>, ProtocolStoreError> {
    state
        .verified_inputs
        .get(&run)
        .map(|verified| {
            Ok(VerifiedSeed {
                run_id,
                input: serde_json::from_slice(&verified.canonical_bytes).map_err(|_| {
                    ProtocolStoreError::Internal(
                        "durable verified input encoding is invalid".into(),
                    )
                })?,
                cursor: protocol_cursor(verified.position),
            })
        })
        .transpose()
}

fn lifecycle_snapshot(
    state: &super::ReplayState,
    cursor: Cursor,
) -> Result<LifecycleSnapshot, ProtocolStoreError> {
    let in_flight = u32::try_from(state.active_dispatches.len())
        .map_err(|_| ProtocolStoreError::Internal("dispatch count exceeds u32".into()))?;
    let mut operational = openengine_cluster_protocol::OperationalStatus {
        in_flight,
        ..Default::default()
    };
    if state.terminal_outcome.is_some() {
        operational.dispatch_state = openengine_cluster_protocol::DispatchState::Stopped;
    }
    Ok(LifecycleSnapshot {
        operational: Some(operational),
        latest_cursor: Some(cursor),
        ..Default::default()
    })
}

#[async_trait]
impl ControlJournal for ClusterLedgerAdapters {
    async fn read_control(&self) -> Result<ControlSnapshot, ProtocolStoreError> {
        Ok(self.folded().await?.admission.control)
    }

    async fn lookup_idempotency(
        &self,
        key: &openengine_cluster_protocol::IdempotencyKey,
    ) -> Result<Option<IdempotencyRecord>, ProtocolStoreError> {
        let key = IdempotencyId::new(key.as_str())
            .map_err(|_| ProtocolStoreError::Internal("invalid idempotency key".into()))?;
        let receipt = self
            .ledger
            .receipt(crate::fault::FaultContext::Recovery, &key)
            .await
            .map_err(protocol_error)?;
        match receipt {
            Some(receipt) => protocol_idempotency_record(receipt),
            None => Ok(None),
        }
    }
}

#[async_trait]
impl VerifiedIoLedger for ClusterLedgerAdapters {
    async fn read_verified_seed(
        &self,
        run_id: &RunId,
    ) -> Result<Option<VerifiedSeed>, ProtocolStoreError> {
        let folded = self.folded().await?;
        Ok(folded.admission.seed.filter(|seed| &seed.run_id == run_id))
    }
}

#[async_trait]
impl AdmissionStore for ClusterLedgerAdapters {
    async fn read_snapshot(&self) -> Result<AdmissionSnapshot, ProtocolStoreError> {
        Ok(self.folded().await?.admission)
    }

    async fn read_aggregate(
        &self,
    ) -> Result<(AdmissionSnapshot, LifecycleSnapshot), ProtocolStoreError> {
        let folded = self.folded().await?;
        Ok((folded.admission, folded.lifecycle))
    }

    async fn commit(
        &self,
        proposal: CommitProposal,
        cancellation: &CancellationSignal,
    ) -> Result<ApplyResult, ProtocolStoreError> {
        let key = IdempotencyId::new(proposal.idempotency_key.as_str())
            .map_err(|_| ProtocolStoreError::Internal("invalid idempotency key".into()))?;
        let fingerprint = fingerprint_bytes(&proposal.fingerprint)?;
        let mut state = self
            .ledger
            .validated_state(crate::fault::FaultContext::Admission)
            .await
            .map_err(protocol_error)?;
        if let Some(existing) = self.existing_protocol_apply(&state, &key, fingerprint)? {
            return Ok(existing);
        }
        ensure_apply_phase(&state)?;
        let current_generation = current_protocol_generation(&state)?;
        validate_protocol_generation(proposal.if_generation, current_generation)?;
        let canonical_compiled_ir = proposal
            .compiled_ir
            .canonical_bytes()
            .map_err(|_| ProtocolStoreError::Internal("compiled graph encoding failed".into()))?;
        let unchanged = state
            .admission
            .as_ref()
            .is_some_and(|admission| admission.canonical_compiled_ir == canonical_compiled_ir);
        if unchanged {
            return self
                .commit_unchanged(
                    &state,
                    proposal,
                    key,
                    fingerprint,
                    current_generation,
                    cancellation,
                )
                .await;
        }
        self.commit_changed(
            &mut state,
            proposal,
            key,
            fingerprint,
            canonical_compiled_ir,
            cancellation,
        )
        .await
    }
}

#[async_trait]
impl LifecycleStore for ClusterLedgerAdapters {
    async fn read_lifecycle_snapshot(&self) -> Result<LifecycleSnapshot, ProtocolStoreError> {
        Ok(self.folded().await?.lifecycle)
    }

    async fn update_lifecycle(
        &self,
        _proposal: UpdateProposal,
    ) -> Result<UpdateResult, ProtocolStoreError> {
        Err(ProtocolStoreError::InvalidPhase {
            current: self.folded().await?.admission.control.phase,
        })
    }

    async fn stop_lifecycle(
        &self,
        _proposal: StopProposal,
    ) -> Result<StopResult, ProtocolStoreError> {
        Err(ProtocolStoreError::InvalidPhase {
            current: self.folded().await?.admission.control.phase,
        })
    }

    async fn acquire_dispatch(
        &self,
        _turn_id: openengine_cluster_server::lifecycle::TurnId,
    ) -> Result<DispatchPermit, ProtocolStoreError> {
        Err(ProtocolStoreError::DispatchDenied {
            current: self
                .folded()
                .await?
                .lifecycle
                .dispatch_state()
                .unwrap_or(openengine_cluster_protocol::DispatchState::Stopped),
        })
    }

    async fn complete_dispatch(
        &self,
        _completion: VerifiedCompletion,
    ) -> Result<CompletionResult, ProtocolStoreError> {
        Err(ProtocolStoreError::UnknownLease)
    }
}

fn protocol_error(error: LedgerError) -> ProtocolStoreError {
    match error.kind() {
        LedgerErrorKind::IdempotencyConflict => ProtocolStoreError::IdempotencyReuse,
        LedgerErrorKind::Storage(StoreError::AppendCancelled) => ProtocolStoreError::Cancelled,
        _ => ProtocolStoreError::Internal("native cluster ledger operation failed".into()),
    }
}

fn cancellation_guard(cancellation: &CancellationSignal) -> AppendGuard {
    let observer = cancellation.observer();
    AppendGuard::cancelled_when(move || observer.is_cancelled())
}

fn protocol_cursor(position: super::store::Position) -> Cursor {
    Cursor::new(format!("ledger:{}", position.get()))
}

fn protocol_run_id(run: super::RunSequence) -> RunId {
    RunId::new(format!("run:{}", run.get()))
}

fn fingerprint_bytes(fingerprint: &RequestFingerprint) -> Result<[u8; 32], ProtocolStoreError> {
    decode_hex_32(fingerprint.as_str())
        .ok_or_else(|| ProtocolStoreError::Internal("request fingerprint is invalid".into()))
}

fn protocol_idempotency_record(
    receipt: MutationReceipt,
) -> Result<Option<IdempotencyRecord>, ProtocolStoreError> {
    let fingerprint =
        serde_json::from_value(serde_json::Value::String(hex_32(receipt.fingerprint)))
            .map_err(|_| ProtocolStoreError::Internal("durable fingerprint is invalid".into()))?;
    let mutation_receipt = match receipt.method.as_str() {
        "protocol_apply" => ProtocolMutationReceipt::Apply(
            serde_json::from_slice(&receipt.response)
                .map_err(|_| ProtocolStoreError::Internal("apply receipt is invalid".into()))?,
        ),
        "protocol_update" => ProtocolMutationReceipt::Update(
            serde_json::from_slice(&receipt.response)
                .map_err(|_| ProtocolStoreError::Internal("update receipt is invalid".into()))?,
        ),
        "protocol_stop" => ProtocolMutationReceipt::Stop(
            serde_json::from_slice(&receipt.response)
                .map_err(|_| ProtocolStoreError::Internal("stop receipt is invalid".into()))?,
        ),
        _ => return Ok(None),
    };
    Ok(Some(IdempotencyRecord {
        fingerprint,
        receipt: mutation_receipt,
    }))
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut result = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_digit(pair[0])?;
        let low = hex_digit(pair[1])?;
        result[index] = (high << 4) | low;
    }
    Some(result)
}

const fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn hex_32(value: [u8; 32]) -> String {
    let mut result = String::with_capacity(64);
    for byte in value {
        use std::fmt::Write as _;
        write!(&mut result, "{byte:02x}").expect("writing to String cannot fail");
    }
    result
}
