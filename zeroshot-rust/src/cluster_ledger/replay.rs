use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::record::{
    CanonicalDigest, EffectId, ExecutionId, GenerationId, IdentityCounters, NodeInstanceId,
    RecordError, RecordPayload, RunSequence, StoredRecord,
};
use super::store::{IdempotencyId, MutationReceipt, Position, PrefixSnapshot, ResourceId};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AdmissionState {
    pub generation: GenerationId,
    pub run: RunSequence,
    pub graph_digest: CanonicalDigest,
    pub input_digest: CanonicalDigest,
    pub policy_digest: CanonicalDigest,
    pub catalog_digest: CanonicalDigest,
    pub profile_digest: CanonicalDigest,
    pub absolute_deadline_ms: u64,
    pub canonical_graph: Vec<u8>,
    pub canonical_compiled_ir: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DispatchState {
    pub run: RunSequence,
    pub node_instance: NodeInstanceId,
    pub execution: ExecutionId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EffectState {
    pub run: RunSequence,
    pub effect: EffectId,
    pub request_digest: CanonicalDigest,
    pub receipt_digest: Option<CanonicalDigest>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerifiedValue {
    pub digest: CanonicalDigest,
    pub canonical_bytes: Vec<u8>,
    pub position: Position,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReplayState {
    pub resource: ResourceId,
    pub position: Position,
    pub last_hash: [u8; 32],
    pub identities: IdentityCounters,
    pub admission: Option<AdmissionState>,
    pub active_dispatches: BTreeMap<ExecutionId, DispatchState>,
    pub settlements: BTreeMap<ExecutionId, CanonicalDigest>,
    pub settlement_runs: BTreeMap<ExecutionId, RunSequence>,
    pub effects: BTreeMap<EffectId, EffectState>,
    pub verified_inputs: BTreeMap<RunSequence, VerifiedValue>,
    pub verified_outputs: BTreeMap<ExecutionId, VerifiedValue>,
    pub safe_faults: Vec<Vec<u8>>,
    pub terminal_outcome: Option<CanonicalDigest>,
    pub cleanup_receipts: Vec<CanonicalDigest>,
    pub mutation_receipts: BTreeMap<IdempotencyId, MutationReceipt>,
}

impl ReplayState {
    #[must_use]
    pub fn empty(resource: ResourceId) -> Self {
        Self {
            resource,
            position: Position::ZERO,
            last_hash: [0; 32],
            identities: IdentityCounters::initial(),
            admission: None,
            active_dispatches: BTreeMap::new(),
            settlements: BTreeMap::new(),
            settlement_runs: BTreeMap::new(),
            effects: BTreeMap::new(),
            verified_inputs: BTreeMap::new(),
            verified_outputs: BTreeMap::new(),
            safe_faults: Vec::new(),
            terminal_outcome: None,
            cleanup_receipts: Vec::new(),
            mutation_receipts: BTreeMap::new(),
        }
    }

    pub fn public_bytes(&self) -> Result<Vec<u8>, ReplayError> {
        serde_json::to_vec(self).map_err(|_| ReplayError::Encoding)
    }
}

pub fn replay(
    snapshot: &PrefixSnapshot,
    resource: &ResourceId,
) -> Result<ReplayState, ReplayError> {
    let mut state = ReplayState::empty(resource.clone());
    let mut pending_mutation = Vec::new();
    for record in &snapshot.records {
        fold_record(&mut state, &mut pending_mutation, record)?;
    }
    if !pending_mutation.is_empty() {
        return Err(ReplayError::ReceiptCorrupt);
    }
    if state.position != snapshot.position {
        return Err(ReplayError::PositionMismatch);
    }
    if state.mutation_receipts.len() != snapshot.receipts.len() {
        return Err(ReplayError::ReceiptCorrupt);
    }
    for receipt in &snapshot.receipts {
        receipt
            .validate()
            .map_err(|_| ReplayError::ReceiptCorrupt)?;
        if receipt.committed_position > snapshot.position {
            return Err(ReplayError::ReceiptCorrupt);
        }
        match state.mutation_receipts.get(&receipt.idempotency_key) {
            Some(existing) if existing != receipt => return Err(ReplayError::ReceiptCorrupt),
            Some(_) => {}
            None => return Err(ReplayError::ReceiptCorrupt),
        }
    }
    Ok(state)
}

fn decode_receipt_response<T>(receipt: &MutationReceipt) -> Result<T, ReplayError>
where
    T: DeserializeOwned + Serialize,
{
    let decoded =
        serde_json::from_slice(&receipt.response).map_err(|_| ReplayError::ReceiptCorrupt)?;
    let canonical = serde_json::to_vec(&decoded).map_err(|_| ReplayError::ReceiptCorrupt)?;
    if canonical != receipt.response {
        return Err(ReplayError::ReceiptCorrupt);
    }
    Ok(decoded)
}

fn validate_receipt_binding(
    receipt: &MutationReceipt,
    state: &ReplayState,
    records: &[RecordPayload],
) -> Result<(), ReplayError> {
    match receipt.method.as_str() {
        "admit" => validate_admission_receipt(receipt, records)?,
        "dispatch" => validate_dispatch_receipt(receipt, records)?,
        "settle" => validate_settlement_receipt(receipt, state, records)?,
        "safe_fault" => validate_safe_fault_receipt(receipt, records)?,
        "effect_intent" | "effect_receipt" => validate_effect_receipt(receipt, records)?,
        "terminal" => validate_terminal_receipt(receipt, records)?,
        "cleanup_receipt" => validate_cleanup_receipt(receipt, records)?,
        "protocol_apply" => validate_protocol_apply_receipt(receipt, state, records)?,
        _ => return Err(ReplayError::ReceiptCorrupt),
    }
    Ok(())
}

fn validate_admission_receipt(
    receipt: &MutationReceipt,
    records: &[RecordPayload],
) -> Result<(), ReplayError> {
    let response = decode_receipt_response::<super::mutations::AdmissionAllocation>(receipt)?;
    match records {
        [
            RecordPayload::Admission {
                generation, run, ..
            },
            RecordPayload::VerifiedInput {
                run: verified_run, ..
            },
        ] if response.generation == *generation && response.run == *run && verified_run == run => {
            Ok(())
        }
        _ => Err(ReplayError::ReceiptCorrupt),
    }
}

fn validate_dispatch_receipt(
    receipt: &MutationReceipt,
    records: &[RecordPayload],
) -> Result<(), ReplayError> {
    let response = decode_receipt_response::<super::mutations::DispatchAllocation>(receipt)?;
    match records {
        [
            RecordPayload::Dispatch {
                run,
                node_instance,
                execution,
            },
        ] if response.run == *run
            && response.node_instance == *node_instance
            && response.execution == *execution =>
        {
            Ok(())
        }
        _ => Err(ReplayError::ReceiptCorrupt),
    }
}

fn validate_settlement_receipt(
    receipt: &MutationReceipt,
    state: &ReplayState,
    records: &[RecordPayload],
) -> Result<(), ReplayError> {
    let response = decode_receipt_response::<super::mutations::SettlementResult>(receipt)?;
    let Some(RecordPayload::Settlement {
        run,
        execution,
        outcome_digest,
        accepted,
    }) = records.first()
    else {
        return Err(ReplayError::ReceiptCorrupt);
    };
    let output_matches = match &records[1..] {
        [] => true,
        [
            RecordPayload::VerifiedOutput {
                run: output_run,
                execution: output_execution,
                digest,
                ..
            },
        ] => {
            *accepted
                && output_run == run
                && output_execution == execution
                && digest == outcome_digest
        }
        _ => false,
    };
    let authoritative_digest = if *accepted {
        *outcome_digest
    } else {
        *state
            .settlements
            .get(execution)
            .ok_or(ReplayError::ReceiptCorrupt)?
    };
    (response.execution == *execution
        && response.accepted == *accepted
        && response.authoritative_digest == authoritative_digest
        && output_matches)
        .then_some(())
        .ok_or(ReplayError::ReceiptCorrupt)
}

fn validate_safe_fault_receipt(
    receipt: &MutationReceipt,
    records: &[RecordPayload],
) -> Result<(), ReplayError> {
    let response = decode_receipt_response::<super::mutations::SafeFaultResult>(receipt)?;
    let valid = match records {
        [
            RecordPayload::SafeFault {
                run,
                execution: Some(fault_execution),
                ..
            },
            RecordPayload::Settlement {
                run: settlement_run,
                execution,
                outcome_digest,
                accepted: true,
            },
        ] => {
            run == settlement_run
                && fault_execution == execution
                && response.execution == Some(*execution)
                && !response.terminal
                && response.outcome_digest == *outcome_digest
        }
        [
            RecordPayload::SafeFault {
                run,
                execution: None,
                ..
            },
            RecordPayload::Terminal {
                run: terminal_run,
                outcome_digest,
            },
        ] => {
            run == terminal_run
                && response.execution.is_none()
                && response.terminal
                && response.outcome_digest == *outcome_digest
        }
        _ => false,
    };
    valid.then_some(()).ok_or(ReplayError::ReceiptCorrupt)
}

fn validate_effect_receipt(
    receipt: &MutationReceipt,
    records: &[RecordPayload],
) -> Result<(), ReplayError> {
    let response = decode_receipt_response::<super::mutations::EffectIntentResult>(receipt)?;
    let effect = match records {
        [RecordPayload::EffectIntent { effect, .. }] if receipt.method == "effect_intent" => effect,
        [RecordPayload::EffectReceipt { effect, .. }] if receipt.method == "effect_receipt" => {
            effect
        }
        _ => return Err(ReplayError::ReceiptCorrupt),
    };
    (response.effect == *effect)
        .then_some(())
        .ok_or(ReplayError::ReceiptCorrupt)
}

fn validate_terminal_receipt(
    receipt: &MutationReceipt,
    records: &[RecordPayload],
) -> Result<(), ReplayError> {
    let response = decode_receipt_response::<CanonicalDigest>(receipt)?;
    match records {
        [RecordPayload::Terminal { outcome_digest, .. }] if response == *outcome_digest => Ok(()),
        _ => Err(ReplayError::ReceiptCorrupt),
    }
}

fn validate_cleanup_receipt(
    receipt: &MutationReceipt,
    records: &[RecordPayload],
) -> Result<(), ReplayError> {
    let response = decode_receipt_response::<CanonicalDigest>(receipt)?;
    match records {
        [RecordPayload::CleanupReceipt { cleanup_digest }] if response == *cleanup_digest => Ok(()),
        _ => Err(ReplayError::ReceiptCorrupt),
    }
}

fn validate_protocol_apply_receipt(
    receipt: &MutationReceipt,
    state: &ReplayState,
    records: &[RecordPayload],
) -> Result<(), ReplayError> {
    let response = decode_receipt_response::<openengine_cluster_protocol::ApplyResult>(receipt)?;
    let (generation, run) = protocol_apply_identity(state, records)?;
    let expected_run = format!("run:{}", run.get());
    let valid = response
        .generation
        .map(openengine_cluster_protocol::Generation::get)
        == Some(generation.get())
        && response
            .run_id
            .as_ref()
            .map(openengine_cluster_protocol::RunId::as_str)
            == Some(expected_run.as_str())
        && response.phase == openengine_cluster_protocol::Phase::Running
        && !response.deduped
        && response.diff.is_none();
    valid.then_some(()).ok_or(ReplayError::ReceiptCorrupt)
}

fn protocol_apply_identity(
    state: &ReplayState,
    records: &[RecordPayload],
) -> Result<(GenerationId, RunSequence), ReplayError> {
    match records {
        [
            RecordPayload::Admission {
                generation, run, ..
            },
            RecordPayload::VerifiedInput {
                run: verified_run, ..
            },
        ] if verified_run == run => Ok((*generation, *run)),
        [] if state.terminal_outcome.is_none() => state
            .admission
            .as_ref()
            .map(|admission| (admission.generation, admission.run))
            .ok_or(ReplayError::ReceiptCorrupt),
        _ => Err(ReplayError::ReceiptCorrupt),
    }
}

fn fold_record(
    state: &mut ReplayState,
    pending_mutation: &mut Vec<RecordPayload>,
    record: &StoredRecord,
) -> Result<(), ReplayError> {
    let expected = state
        .position
        .checked_add(1)
        .map_err(|_| ReplayError::PositionOverflow)?;
    let payload = record
        .validate(&state.resource, expected, state.last_hash)
        .map_err(ReplayError::Record)?;
    let pending_payload =
        (!matches!(&payload, RecordPayload::MutationReceipt { .. })).then(|| payload.clone());
    if state.terminal_outcome.is_some() && !record.kind.allowed_after_terminal() {
        return Err(ReplayError::PostTerminalRecord);
    }
    match payload {
        payload @ RecordPayload::Admission { .. } => fold_admission(state, payload)?,
        payload @ RecordPayload::Dispatch { .. } => fold_dispatch(state, payload)?,
        payload @ RecordPayload::Settlement { .. } => fold_settlement(state, payload)?,
        payload @ RecordPayload::SafeFault { .. } => fold_safe_fault(state, payload)?,
        payload @ RecordPayload::EffectIntent { .. } => fold_effect_intent(state, payload)?,
        payload @ RecordPayload::EffectReceipt { .. } => fold_effect_receipt(state, payload)?,
        payload @ RecordPayload::Terminal { .. } => fold_terminal(state, payload)?,
        RecordPayload::CleanupReceipt { cleanup_digest } => {
            fold_cleanup_receipt(state, cleanup_digest)?;
        }
        payload @ RecordPayload::VerifiedInput { .. } => {
            fold_verified_input(state, payload, record.sequence)?;
        }
        payload @ RecordPayload::VerifiedOutput { .. } => {
            fold_verified_output(state, payload, record.sequence)?;
        }
        RecordPayload::MutationReceipt { receipt } => {
            fold_mutation_receipt(state, pending_mutation, receipt, record.sequence)?;
        }
    }
    if let Some(payload) = pending_payload {
        pending_mutation.push(payload);
    }
    state.position = record.sequence;
    state.last_hash = record.record_hash;
    Ok(())
}

fn fold_admission(state: &mut ReplayState, payload: RecordPayload) -> Result<(), ReplayError> {
    let RecordPayload::Admission {
        generation,
        run,
        graph_digest,
        input_digest,
        policy_digest,
        catalog_digest,
        profile_digest,
        absolute_deadline_ms,
        canonical_graph,
        canonical_compiled_ir,
    } = payload
    else {
        unreachable!("admission fold receives admission payload");
    };
    if !state.active_dispatches.is_empty()
        || state
            .effects
            .values()
            .any(|effect| effect.receipt_digest.is_none())
        || generation.get() != state.identities.next_generation
        || run.get() != state.identities.next_run
    {
        return Err(ReplayError::InvalidOrder);
    }
    state.identities.next_generation =
        advance_identity(state.identities.next_generation, generation.get())?;
    state.identities.next_run = advance_identity(state.identities.next_run, run.get())?;
    state.admission = Some(AdmissionState {
        generation,
        run,
        graph_digest,
        input_digest,
        policy_digest,
        catalog_digest,
        profile_digest,
        absolute_deadline_ms,
        canonical_graph,
        canonical_compiled_ir,
    });
    Ok(())
}

fn fold_dispatch(state: &mut ReplayState, payload: RecordPayload) -> Result<(), ReplayError> {
    let RecordPayload::Dispatch {
        run,
        node_instance,
        execution,
    } = payload
    else {
        unreachable!("dispatch fold receives dispatch payload");
    };
    require_current_run(state, run)?;
    if state.active_dispatches.contains_key(&execution)
        || state.settlements.contains_key(&execution)
    {
        return Err(ReplayError::InvalidOrder);
    }
    state.identities.next_node_instance =
        advance_identity(state.identities.next_node_instance, node_instance.get())?;
    state.identities.next_execution =
        advance_identity(state.identities.next_execution, execution.get())?;
    state.active_dispatches.insert(
        execution,
        DispatchState {
            run,
            node_instance,
            execution,
        },
    );
    Ok(())
}

fn fold_settlement(state: &mut ReplayState, payload: RecordPayload) -> Result<(), ReplayError> {
    let RecordPayload::Settlement {
        run,
        execution,
        outcome_digest,
        accepted,
    } = payload
    else {
        unreachable!("settlement fold receives settlement payload");
    };
    if !accepted {
        if !state.settlements.contains_key(&execution)
            || state.settlement_runs.get(&execution) != Some(&run)
        {
            return Err(ReplayError::InvalidSettlement);
        }
        return Ok(());
    }
    require_current_run(state, run)?;
    if state.settlements.contains_key(&execution)
        || state
            .active_dispatches
            .remove(&execution)
            .is_none_or(|dispatch| dispatch.run != run)
    {
        return Err(ReplayError::InvalidSettlement);
    }
    state.settlements.insert(execution, outcome_digest);
    state.settlement_runs.insert(execution, run);
    Ok(())
}

fn fold_safe_fault(state: &mut ReplayState, payload: RecordPayload) -> Result<(), ReplayError> {
    let RecordPayload::SafeFault {
        run,
        execution,
        encoded_fault,
    } = payload
    else {
        unreachable!("safe fault fold receives safe fault payload");
    };
    require_current_run(state, run)?;
    if execution.is_some_and(|execution| {
        !state.active_dispatches.contains_key(&execution)
            && !state.settlements.contains_key(&execution)
    }) {
        return Err(ReplayError::InvalidOrder);
    }
    crate::fault::EngineFault::decode_json(&encoded_fault).map_err(|_| ReplayError::UnsafeFault)?;
    state.safe_faults.push(encoded_fault);
    Ok(())
}

fn fold_effect_intent(state: &mut ReplayState, payload: RecordPayload) -> Result<(), ReplayError> {
    let RecordPayload::EffectIntent {
        run,
        effect,
        request_digest,
    } = payload
    else {
        unreachable!("effect intent fold receives effect intent payload");
    };
    require_current_run(state, run)?;
    if state.effects.contains_key(&effect) {
        return Err(ReplayError::InvalidOrder);
    }
    state.identities.next_effect = advance_identity(state.identities.next_effect, effect.get())?;
    state.effects.insert(
        effect,
        EffectState {
            run,
            effect,
            request_digest,
            receipt_digest: None,
        },
    );
    Ok(())
}

fn fold_effect_receipt(state: &mut ReplayState, payload: RecordPayload) -> Result<(), ReplayError> {
    let RecordPayload::EffectReceipt {
        run,
        effect,
        receipt_digest,
    } = payload
    else {
        unreachable!("effect receipt fold receives effect receipt payload");
    };
    require_current_run(state, run)?;
    let intent = state
        .effects
        .get_mut(&effect)
        .ok_or(ReplayError::InvalidOrder)?;
    if intent.receipt_digest.replace(receipt_digest).is_some() {
        return Err(ReplayError::InvalidOrder);
    }
    Ok(())
}

fn fold_terminal(state: &mut ReplayState, payload: RecordPayload) -> Result<(), ReplayError> {
    let RecordPayload::Terminal {
        run,
        outcome_digest,
    } = payload
    else {
        unreachable!("terminal fold receives terminal payload");
    };
    require_current_run(state, run)?;
    if state
        .effects
        .values()
        .any(|effect| effect.receipt_digest.is_none())
    {
        return Err(ReplayError::InvalidOrder);
    }
    if state.terminal_outcome.replace(outcome_digest).is_some() {
        return Err(ReplayError::PostTerminalRecord);
    }
    state.active_dispatches.clear();
    Ok(())
}

fn fold_cleanup_receipt(
    state: &mut ReplayState,
    cleanup_digest: CanonicalDigest,
) -> Result<(), ReplayError> {
    if state.terminal_outcome.is_none() {
        return Err(ReplayError::InvalidOrder);
    }
    state.cleanup_receipts.push(cleanup_digest);
    Ok(())
}

fn fold_verified_input(
    state: &mut ReplayState,
    payload: RecordPayload,
    position: Position,
) -> Result<(), ReplayError> {
    let RecordPayload::VerifiedInput {
        run,
        digest,
        canonical_bytes,
    } = payload
    else {
        unreachable!("verified input fold receives verified input payload");
    };
    require_current_run(state, run)?;
    if state
        .admission
        .as_ref()
        .is_none_or(|admission| admission.input_digest != digest)
        || state
            .verified_inputs
            .insert(
                run,
                VerifiedValue {
                    digest,
                    canonical_bytes,
                    position,
                },
            )
            .is_some()
    {
        return Err(ReplayError::InvalidOrder);
    }
    Ok(())
}

fn fold_verified_output(
    state: &mut ReplayState,
    payload: RecordPayload,
    position: Position,
) -> Result<(), ReplayError> {
    let RecordPayload::VerifiedOutput {
        run,
        execution,
        digest,
        canonical_bytes,
    } = payload
    else {
        unreachable!("verified output fold receives verified output payload");
    };
    require_current_run(state, run)?;
    if state.settlements.get(&execution) != Some(&digest)
        || state
            .verified_outputs
            .insert(
                execution,
                VerifiedValue {
                    digest,
                    canonical_bytes,
                    position,
                },
            )
            .is_some()
    {
        return Err(ReplayError::InvalidOrder);
    }
    Ok(())
}

fn fold_mutation_receipt(
    state: &mut ReplayState,
    pending_mutation: &mut Vec<RecordPayload>,
    receipt: MutationReceipt,
    position: Position,
) -> Result<(), ReplayError> {
    receipt
        .validate()
        .map_err(|_| ReplayError::ReceiptCorrupt)?;
    validate_receipt_binding(&receipt, state, pending_mutation)?;
    if receipt.committed_position != position
        || state
            .mutation_receipts
            .insert(receipt.idempotency_key.clone(), receipt)
            .is_some()
    {
        return Err(ReplayError::ReceiptCorrupt);
    }
    pending_mutation.clear();
    Ok(())
}

fn require_current_run(state: &ReplayState, run: RunSequence) -> Result<(), ReplayError> {
    match &state.admission {
        Some(admission) if admission.run == run => Ok(()),
        _ => Err(ReplayError::InvalidOrder),
    }
}

fn advance_identity(current: u64, observed: u64) -> Result<u64, ReplayError> {
    if observed != current {
        return Err(ReplayError::InvalidOrder);
    }
    observed
        .checked_add(1)
        .filter(|value| *value <= i64::MAX as u64)
        .ok_or(ReplayError::PositionOverflow)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplayError {
    Record(RecordError),
    Encoding,
    PositionMismatch,
    PositionOverflow,
    ReceiptCorrupt,
    InvalidOrder,
    InvalidSettlement,
    UnsafeFault,
    PostTerminalRecord,
}

impl fmt::Display for ReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Record(error) => write!(formatter, "{error}"),
            Self::Encoding => formatter.write_str("public replay state encoding failed"),
            Self::PositionMismatch => {
                formatter.write_str("replay position does not match coherent prefix")
            }
            Self::PositionOverflow => formatter.write_str("replay counter overflow"),
            Self::ReceiptCorrupt => formatter.write_str("mutation receipt is corrupt"),
            Self::InvalidOrder => formatter.write_str("ledger record order is invalid"),
            Self::InvalidSettlement => formatter.write_str("settlement violates first-wins"),
            Self::UnsafeFault => formatter.write_str("persisted fault is not a safe engine fault"),
            Self::PostTerminalRecord => {
                formatter.write_str("run-scoped record follows terminal state")
            }
        }
    }
}

impl Error for ReplayError {}

impl From<RecordError> for ReplayError {
    fn from(value: RecordError) -> Self {
        Self::Record(value)
    }
}
