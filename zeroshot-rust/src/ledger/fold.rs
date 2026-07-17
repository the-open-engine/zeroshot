use std::collections::{BTreeMap, BTreeSet};

use openengine_cluster_protocol::{
    diff_compiled_graphs, CompiledGraphIr, DispatchState, GraphSpec, Labels, LogLevel,
    OperationalStatus, Phase, StopMode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::fault::EngineFault;

use super::identity::{
    ExecutionId, IdempotencyId, LedgerGeneration, LedgerRunId, NodeInstanceId, Position, ResourceId,
};
use super::record::{
    AdmissionManifest, ClosedMutationReceipt, LedgerRecord, RecordPayload, TerminalOutcome,
};
use super::store::{OpaqueMutationReceipt, MAX_RECEIPT_BYTES};
use super::validation::{admission_manifest, run_id as expected_run_id, valid_component, valid_digest};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerPhase {
    #[default]
    Empty,
    Running,
    Terminal,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmittedRun {
    pub generation: LedgerGeneration,
    pub run_id: LedgerRunId,
    pub graph: GraphSpec,
    pub compiled_ir: CompiledGraphIr,
    pub input: Value,
    pub manifest: AdmissionManifest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActiveDispatch {
    pub node_instance_id: NodeInstanceId,
    pub execution_id: ExecutionId,
    pub turn_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptedSettlement {
    pub execution_id: ExecutionId,
    pub turn_id: String,
    pub output: Value,
    pub at_position: Position,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VoidDispatch {
    pub execution_id: ExecutionId,
    pub turn_id: String,
    pub at_position: Position,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EffectState {
    pub execution_id: ExecutionId,
    pub request_digest: String,
    pub reconciliation_digest: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicClusterState {
    pub resource_id: ResourceId,
    pub at_position: Position,
    pub phase: LedgerPhase,
    pub admitted: Option<AdmittedRun>,
    pub admission_position: Option<Position>,
    pub active_dispatches: BTreeMap<ExecutionId, ActiveDispatch>,
    pub settlements: BTreeMap<ExecutionId, AcceptedSettlement>,
    pub voided_dispatches: BTreeMap<ExecutionId, VoidDispatch>,
    pub safe_faults: Vec<Vec<u8>>,
    pub effects: BTreeMap<String, EffectState>,
    pub cleanup_receipts: BTreeMap<String, String>,
    pub terminal_outcome: Option<TerminalOutcome>,
    pub labels: Option<Labels>,
    pub log_level: Option<LogLevel>,
    pub suspended: bool,
    pub stop_mode: Option<StopMode>,
    pub mutation_receipts: BTreeMap<IdempotencyId, OpaqueMutationReceipt>,
}

impl PublicClusterState {
    #[must_use]
    pub fn empty(resource_id: ResourceId) -> Self {
        Self {
            resource_id,
            at_position: Position::ZERO,
            phase: LedgerPhase::Empty,
            admitted: None,
            admission_position: None,
            active_dispatches: BTreeMap::new(),
            settlements: BTreeMap::new(),
            voided_dispatches: BTreeMap::new(),
            safe_faults: Vec::new(),
            effects: BTreeMap::new(),
            cleanup_receipts: BTreeMap::new(),
            terminal_outcome: None,
            labels: None,
            log_level: None,
            suspended: false,
            stop_mode: None,
            mutation_receipts: BTreeMap::new(),
        }
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, FoldError> {
        let value = serde_json::to_value(self).map_err(|_| FoldError::InvalidPayload)?;
        openengine_cluster_protocol::canonical_value_bytes(&value)
            .map_err(|_| FoldError::InvalidPayload)
    }

    #[must_use]
    pub fn generation(&self) -> Option<LedgerGeneration> {
        self.admitted.as_ref().map(|run| run.generation)
    }

    #[must_use]
    pub fn run_id(&self) -> Option<&LedgerRunId> {
        self.admitted.as_ref().map(|run| &run.run_id)
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum FoldError {
    #[error("ledger prefix contains a gap, identity mismatch, or invalid hash chain")]
    InvalidOrder,
    #[error("ledger record payload is invalid, noncanonical, unknown, or mismatched")]
    InvalidPayload,
    #[error("ledger prefix describes an impossible domain transition")]
    ImpossibleTransition,
    #[error("ledger mutation receipt is corrupt or conflicts with committed history")]
    CorruptReceipt,
}

pub fn fold_records(
    resource_id: &ResourceId,
    records: &[LedgerRecord],
) -> Result<PublicClusterState, FoldError> {
    let mut context = FoldContext::new(resource_id.clone());
    for record in records {
        context.apply_record(resource_id, record)?;
    }
    context.finish()
}

struct FoldContext {
    state: PublicClusterState,
    previous_hash: [u8; 32],
    expected: Position,
    allocated_nodes: BTreeSet<NodeInstanceId>,
    pending_fault_consequence: Option<TerminalOutcome>,
    pending_payloads: Vec<RecordPayload>,
    mutation_start: Option<PublicClusterState>,
}

impl FoldContext {
    fn new(resource_id: ResourceId) -> Self {
        Self {
            state: PublicClusterState::empty(resource_id),
            previous_hash: [0; 32],
            expected: Position::ZERO,
            allocated_nodes: BTreeSet::new(),
            pending_fault_consequence: None,
            pending_payloads: Vec::new(),
            mutation_start: None,
        }
    }

    fn apply_record(
        &mut self,
        resource_id: &ResourceId,
        record: &LedgerRecord,
    ) -> Result<(), FoldError> {
        self.expected = self
            .expected
            .checked_next()
            .map_err(|_| FoldError::InvalidOrder)?;
        if &record.resource_id != resource_id
            || record.sequence != self.expected
            || record.previous_hash != self.previous_hash
        {
            return Err(FoldError::InvalidOrder);
        }
        let payload = record
            .decode_payload()
            .map_err(|_| FoldError::InvalidPayload)?;
        self.validate_record_phase(&payload)?;
        if let RecordPayload::MutationReceipt {
            key,
            fingerprint,
            receipt,
        } = payload
        {
            self.apply_receipt(key, fingerprint, receipt, record)?;
            return Ok(());
        }
        if self.pending_payloads.is_empty() {
            self.mutation_start = Some(self.state.clone());
        }
        self.pending_payloads.push(payload.clone());
        self.apply_payload(payload, record.sequence)?;
        self.advance(record);
        Ok(())
    }

    fn validate_record_phase(&self, payload: &RecordPayload) -> Result<(), FoldError> {
        if self.pending_fault_consequence.is_some()
            && !matches!(
                payload,
                RecordPayload::Void { .. } | RecordPayload::Terminal { .. }
            )
        {
            return Err(FoldError::ImpossibleTransition);
        }
        if self.state.phase == LedgerPhase::Terminal
            && !matches!(
                payload,
                RecordPayload::EffectReceipt { .. }
                    | RecordPayload::CleanupReceipt { .. }
                    | RecordPayload::MutationReceipt { .. }
            )
        {
            return Err(FoldError::ImpossibleTransition);
        }
        Ok(())
    }

    fn apply_receipt(
        &mut self,
        key: IdempotencyId,
        fingerprint: [u8; 32],
        receipt: ClosedMutationReceipt,
        record: &LedgerRecord,
    ) -> Result<(), FoldError> {
        validate_mutation_receipt(
            self.mutation_start.as_ref().unwrap_or(&self.state),
            &self.state,
            &self.pending_payloads,
            &receipt,
            record.sequence,
        )?;
        let value = receipt
            .encode_value()
            .map_err(|_| FoldError::CorruptReceipt)?;
        if value.len() > MAX_RECEIPT_BYTES {
            return Err(FoldError::CorruptReceipt);
        }
        let opaque = OpaqueMutationReceipt {
            key: key.clone(),
            method: receipt.kind().method().to_owned(),
            fingerprint,
            value,
            at_position: record.sequence,
        };
        opaque.validate().map_err(|_| FoldError::CorruptReceipt)?;
        if self.state.mutation_receipts.insert(key, opaque).is_some() {
            return Err(FoldError::CorruptReceipt);
        }
        self.pending_payloads.clear();
        self.mutation_start = None;
        self.advance(record);
        Ok(())
    }

    fn apply_payload(
        &mut self,
        payload: RecordPayload,
        position: Position,
    ) -> Result<(), FoldError> {
        match payload {
            RecordPayload::Admission {
                generation,
                run_id,
                graph,
                compiled_ir,
                input,
                manifest,
            } => self.apply_admission(
                AdmissionRecordData {
                    generation,
                    run_id,
                    graph: *graph,
                    compiled_ir: *compiled_ir,
                    input,
                    manifest,
                },
                position,
            ),
            RecordPayload::Dispatch {
                node_instance_id,
                execution_id,
                turn_id,
            } => self.apply_dispatch(node_instance_id, execution_id, turn_id, position),
            RecordPayload::Settlement {
                execution_id,
                output,
            } => self.apply_settlement(execution_id, output, position),
            RecordPayload::Void { execution_id } => self.apply_void(execution_id, position),
            RecordPayload::SafeFault {
                execution_id,
                encoded_fault,
                consequence,
            } => self.apply_safe_fault(execution_id, encoded_fault, consequence),
            RecordPayload::EffectIntent {
                execution_id,
                effect_id,
                request_digest,
            } => self.apply_effect_intent(execution_id, effect_id, request_digest),
            RecordPayload::EffectReceipt {
                effect_id,
                reconciliation_digest,
            } => self.apply_effect_receipt(effect_id, reconciliation_digest),
            RecordPayload::LifecycleUpdate {
                labels,
                log_level,
                suspended,
            } => self.apply_lifecycle_update(labels, log_level, suspended),
            RecordPayload::StopRequested {
                accepted_mode,
                effective_mode,
            } => self.apply_stop(accepted_mode, effective_mode),
            RecordPayload::Terminal { outcome } => self.apply_terminal(outcome),
            RecordPayload::CleanupReceipt {
                resource_id,
                reconciliation_digest,
            } => self.apply_cleanup(resource_id, reconciliation_digest),
            RecordPayload::MutationReceipt { .. } => unreachable!("receipts handled above"),
        }
    }

    fn apply_admission(
        &mut self,
        admission: AdmissionRecordData,
        position: Position,
    ) -> Result<(), FoldError> {
        let AdmissionRecordData {
            generation,
            run_id,
            graph,
            compiled_ir,
            input,
            manifest,
        } = admission;
        let expected_generation = self
            .state
            .generation()
            .map_or_else(|| LedgerGeneration::new(1), LedgerGeneration::checked_next)
            .map_err(|_| FoldError::ImpossibleTransition)?;
        let expected_manifest = admission_manifest(&graph, &compiled_ir, &input, manifest.deadline)
            .map_err(|_| FoldError::InvalidPayload)?;
        let allocated_run_id = expected_run_id(generation, &expected_manifest.graph_digest)
            .map_err(|_| FoldError::InvalidPayload)?;
        if !matches!(self.state.phase, LedgerPhase::Empty | LedgerPhase::Running)
            || generation != expected_generation
            || run_id != allocated_run_id
            || manifest != expected_manifest
            || (self.state.phase == LedgerPhase::Running
                && (self.state.suspended || !self.state.active_dispatches.is_empty()))
        {
            return Err(FoldError::ImpossibleTransition);
        }
        self.reset_run_state();
        self.state.phase = LedgerPhase::Running;
        self.state.admission_position = Some(position);
        self.state.admitted = Some(AdmittedRun {
            generation,
            run_id,
            graph,
            compiled_ir,
            input,
            manifest,
        });
        Ok(())
    }

    fn reset_run_state(&mut self) {
        self.allocated_nodes.clear();
        self.state.active_dispatches.clear();
        self.state.settlements.clear();
        self.state.voided_dispatches.clear();
        self.state.safe_faults.clear();
        self.state
            .effects
            .retain(|_, effect| effect.reconciliation_digest.is_none());
        self.state.cleanup_receipts.clear();
        self.state.terminal_outcome = None;
        self.state.labels = None;
        self.state.log_level = None;
        self.state.suspended = false;
        self.state.stop_mode = None;
    }

    fn apply_dispatch(
        &mut self,
        node_instance_id: NodeInstanceId,
        execution_id: ExecutionId,
        turn_id: String,
        position: Position,
    ) -> Result<(), FoldError> {
        require_running(&self.state)?;
        let generation = self
            .state
            .generation()
            .ok_or(FoldError::ImpossibleTransition)?;
        let expected_node =
            NodeInstanceId::new(format!("node-{}-{}", generation.get(), position.get()))
                .map_err(|_| FoldError::ImpossibleTransition)?;
        let expected_execution =
            ExecutionId::new(format!("execution-{}-{}", generation.get(), position.get()))
                .map_err(|_| FoldError::ImpossibleTransition)?;
        if self.state.suspended
            || !valid_component(&turn_id)
            || node_instance_id != expected_node
            || execution_id != expected_execution
            || self.state.active_dispatches.contains_key(&execution_id)
            || self.state.settlements.contains_key(&execution_id)
            || self.state.voided_dispatches.contains_key(&execution_id)
            || !self.allocated_nodes.insert(node_instance_id.clone())
        {
            return Err(FoldError::ImpossibleTransition);
        }
        self.state.active_dispatches.insert(
            execution_id.clone(),
            ActiveDispatch {
                node_instance_id,
                execution_id,
                turn_id,
            },
        );
        Ok(())
    }

    fn apply_settlement(
        &mut self,
        execution_id: ExecutionId,
        output: Value,
        position: Position,
    ) -> Result<(), FoldError> {
        require_running(&self.state)?;
        if self.state.settlements.contains_key(&execution_id) {
            return Ok(());
        }
        let dispatch = self
            .state
            .active_dispatches
            .remove(&execution_id)
            .ok_or(FoldError::ImpossibleTransition)?;
        self.state.settlements.insert(
            execution_id.clone(),
            AcceptedSettlement {
                execution_id,
                turn_id: dispatch.turn_id,
                output,
                at_position: position,
            },
        );
        Ok(())
    }

    fn apply_void(
        &mut self,
        execution_id: ExecutionId,
        position: Position,
    ) -> Result<(), FoldError> {
        require_running(&self.state)?;
        if self.pending_fault_consequence.is_none() && self.state.stop_mode != Some(StopMode::Force)
        {
            return Err(FoldError::ImpossibleTransition);
        }
        let dispatch = self
            .state
            .active_dispatches
            .remove(&execution_id)
            .ok_or(FoldError::ImpossibleTransition)?;
        self.state.voided_dispatches.insert(
            execution_id.clone(),
            VoidDispatch {
                execution_id,
                turn_id: dispatch.turn_id,
                at_position: position,
            },
        );
        Ok(())
    }

    fn apply_safe_fault(
        &mut self,
        execution_id: Option<ExecutionId>,
        encoded_fault: Vec<u8>,
        consequence: TerminalOutcome,
    ) -> Result<(), FoldError> {
        require_running(&self.state)?;
        EngineFault::decode_json(&encoded_fault).map_err(|_| FoldError::InvalidPayload)?;
        if consequence == TerminalOutcome::Succeeded {
            return Err(FoldError::ImpossibleTransition);
        }
        if execution_id
            .as_ref()
            .is_some_and(|id| !self.state.active_dispatches.contains_key(id))
        {
            return Err(FoldError::ImpossibleTransition);
        }
        self.state.safe_faults.push(encoded_fault);
        self.pending_fault_consequence = Some(consequence);
        Ok(())
    }

    fn apply_effect_intent(
        &mut self,
        execution_id: ExecutionId,
        effect_id: String,
        request_digest: String,
    ) -> Result<(), FoldError> {
        require_running(&self.state)?;
        if !valid_component(&effect_id)
            || !valid_digest(&request_digest)
            || !self.state.active_dispatches.contains_key(&execution_id)
            || self.state.effects.contains_key(&effect_id)
        {
            return Err(FoldError::ImpossibleTransition);
        }
        self.state.effects.insert(
            effect_id,
            EffectState {
                execution_id,
                request_digest,
                reconciliation_digest: None,
            },
        );
        Ok(())
    }

    fn apply_effect_receipt(
        &mut self,
        effect_id: String,
        reconciliation_digest: String,
    ) -> Result<(), FoldError> {
        if !matches!(
            self.state.phase,
            LedgerPhase::Running | LedgerPhase::Terminal
        ) {
            return Err(FoldError::ImpossibleTransition);
        }
        if !valid_digest(&reconciliation_digest) {
            return Err(FoldError::ImpossibleTransition);
        }
        let effect = self
            .state
            .effects
            .get_mut(&effect_id)
            .ok_or(FoldError::ImpossibleTransition)?;
        match &effect.reconciliation_digest {
            Some(existing) if existing != &reconciliation_digest => {
                return Err(FoldError::ImpossibleTransition);
            }
            Some(_) => {}
            None => effect.reconciliation_digest = Some(reconciliation_digest),
        }
        Ok(())
    }

    fn apply_lifecycle_update(
        &mut self,
        labels: Option<Labels>,
        log_level: Option<LogLevel>,
        suspended: Option<bool>,
    ) -> Result<(), FoldError> {
        require_running(&self.state)?;
        if self.state.stop_mode.is_some()
            || (labels.is_none() && log_level.is_none() && suspended.is_none())
        {
            return Err(FoldError::ImpossibleTransition);
        }
        if let Some(labels) = labels {
            self.state.labels = Some(labels);
        }
        if let Some(log_level) = log_level {
            self.state.log_level = Some(log_level);
        }
        if let Some(suspended) = suspended {
            self.state.suspended = suspended;
        }
        Ok(())
    }

    fn apply_stop(
        &mut self,
        accepted_mode: StopMode,
        effective_mode: StopMode,
    ) -> Result<(), FoldError> {
        require_running(&self.state)?;
        let expected = match (self.state.stop_mode, accepted_mode) {
            (None, mode) => mode,
            (Some(StopMode::Drain), StopMode::Drain) => StopMode::Drain,
            (Some(StopMode::Drain), StopMode::Force) => StopMode::Force,
            _ => return Err(FoldError::ImpossibleTransition),
        };
        if effective_mode != expected {
            return Err(FoldError::ImpossibleTransition);
        }
        self.state.stop_mode = Some(effective_mode);
        self.state.suspended = true;
        Ok(())
    }

    fn apply_terminal(&mut self, outcome: TerminalOutcome) -> Result<(), FoldError> {
        require_running(&self.state)?;
        if self
            .pending_fault_consequence
            .is_some_and(|expected| expected != outcome)
            || !self.state.active_dispatches.is_empty()
        {
            return Err(FoldError::ImpossibleTransition);
        }
        self.pending_fault_consequence = None;
        self.state.phase = LedgerPhase::Terminal;
        self.state.terminal_outcome = Some(outcome);
        Ok(())
    }

    fn apply_cleanup(
        &mut self,
        resource_id: String,
        reconciliation_digest: String,
    ) -> Result<(), FoldError> {
        if self.state.phase != LedgerPhase::Terminal
            || !valid_component(&resource_id)
            || !valid_digest(&reconciliation_digest)
            || self
                .state
                .cleanup_receipts
                .get(&resource_id)
                .is_some_and(|existing| existing != &reconciliation_digest)
        {
            return Err(FoldError::ImpossibleTransition);
        }
        self.state
            .cleanup_receipts
            .entry(resource_id)
            .or_insert(reconciliation_digest);
        Ok(())
    }

    fn advance(&mut self, record: &LedgerRecord) {
        self.state.at_position = record.sequence;
        self.previous_hash = record.record_hash;
    }

    fn finish(self) -> Result<PublicClusterState, FoldError> {
        if self.pending_fault_consequence.is_some() || !self.pending_payloads.is_empty() {
            Err(FoldError::ImpossibleTransition)
        } else {
            Ok(self.state)
        }
    }
}

struct AdmissionRecordData {
    generation: LedgerGeneration,
    run_id: LedgerRunId,
    graph: GraphSpec,
    compiled_ir: CompiledGraphIr,
    input: Value,
    manifest: AdmissionManifest,
}

fn require_running(state: &PublicClusterState) -> Result<(), FoldError> {
    if state.phase == LedgerPhase::Running {
        Ok(())
    } else {
        Err(FoldError::ImpossibleTransition)
    }
}

fn validate_mutation_receipt(
    before: &PublicClusterState,
    after: &PublicClusterState,
    payloads: &[RecordPayload],
    receipt: &ClosedMutationReceipt,
    position: Position,
) -> Result<(), FoldError> {
    if receipt.at_position() != position || receipt_is_deduped(receipt) {
        return Err(FoldError::CorruptReceipt);
    }
    let valid = match receipt {
        ClosedMutationReceipt::Admit(value) => validate_admit_receipt(value, payloads, after),
        ClosedMutationReceipt::Apply(value) => {
            validate_apply_receipt(value, before, after, payloads)
        }
        ClosedMutationReceipt::Dispatch(value) => validate_dispatch_receipt(value, payloads, after),
        ClosedMutationReceipt::Settle(value) => {
            validate_settlement_receipt(value, before, after, payloads)
        }
        ClosedMutationReceipt::SafeFault(_) => validate_safe_fault_receipt(before, after, payloads),
        ClosedMutationReceipt::EffectIntent(_) => validate_effect_intent_receipt(after, payloads),
        ClosedMutationReceipt::EffectReceipt(_) => validate_effect_receipt(after, payloads),
        ClosedMutationReceipt::Update(value) => {
            validate_update_receipt(value, before, after, payloads, position)
        }
        ClosedMutationReceipt::Stop(value) => {
            validate_stop_receipt(value, before, after, payloads, position)
        }
        ClosedMutationReceipt::Terminalize(_) => validate_terminal_receipt(before, after, payloads),
        ClosedMutationReceipt::Cleanup(_) => validate_cleanup_receipt(after, payloads),
    };
    if valid {
        Ok(())
    } else {
        Err(FoldError::CorruptReceipt)
    }
}

fn validate_admit_receipt(
    receipt: &super::AdmissionReceipt,
    payloads: &[RecordPayload],
    after: &PublicClusterState,
) -> bool {
    let [
        RecordPayload::Admission {
            generation, run_id, ..
        },
    ] = payloads
    else {
        return false;
    };
    receipt.generation == *generation
        && receipt.run_id == *run_id
        && after.phase == LedgerPhase::Running
        && after.generation() == Some(*generation)
        && after.run_id() == Some(run_id)
}

fn validate_apply_receipt(
    receipt: &super::record::ApplyMutationReceipt,
    before: &PublicClusterState,
    after: &PublicClusterState,
    payloads: &[RecordPayload],
) -> bool {
    if !apply_identity_matches(receipt, after) {
        return false;
    }
    match payloads {
        [] => {
            before.phase == LedgerPhase::Running && before == after && receipt.result.diff.is_none()
        }
        [RecordPayload::Admission { .. }] => {
            let Some(admitted) = after.admitted.as_ref() else {
                return false;
            };
            let Ok(expected) = diff_compiled_graphs(
                before.admitted.as_ref().map(|run| &run.compiled_ir),
                &admitted.compiled_ir,
            ) else {
                return false;
            };
            receipt.result.diff.as_ref() == Some(&expected)
        }
        _ => false,
    }
}

fn apply_identity_matches(
    receipt: &super::record::ApplyMutationReceipt,
    state: &PublicClusterState,
) -> bool {
    receipt.result.phase == Phase::Running
        && receipt.result.generation.map(|value| value.get())
            == state.generation().map(LedgerGeneration::get)
        && receipt.result.run_id.as_ref().map(|value| value.as_str())
            == state.run_id().map(LedgerRunId::as_str)
}

fn validate_dispatch_receipt(
    receipt: &super::DispatchReceipt,
    payloads: &[RecordPayload],
    after: &PublicClusterState,
) -> bool {
    let [
        RecordPayload::Dispatch {
            node_instance_id,
            execution_id,
            turn_id,
        },
    ] = payloads
    else {
        return false;
    };
    receipt.node_instance_id == *node_instance_id
        && receipt.execution_id == *execution_id
        && after
            .active_dispatches
            .get(execution_id)
            .is_some_and(|dispatch| {
                dispatch.node_instance_id == *node_instance_id && dispatch.turn_id == *turn_id
            })
}

fn validate_settlement_receipt(
    receipt: &super::SettlementReceipt,
    before: &PublicClusterState,
    after: &PublicClusterState,
    payloads: &[RecordPayload],
) -> bool {
    let Some(RecordPayload::Settlement { execution_id, .. }) = payloads.first() else {
        return false;
    };
    let accepted = before.active_dispatches.contains_key(execution_id);
    if receipt.execution_id != *execution_id || receipt.accepted != accepted {
        return false;
    }
    match payloads {
        [RecordPayload::Settlement { .. }] => {
            !receipt.terminalized
                && after.phase == LedgerPhase::Running
                && (accepted || before.settlements == after.settlements)
        }
        [
            RecordPayload::Settlement { .. },
            RecordPayload::Terminal { outcome },
        ] => {
            accepted
                && receipt.terminalized
                && *outcome == TerminalOutcome::Stopped
                && before.stop_mode == Some(StopMode::Drain)
                && before.active_dispatches.len() == 1
                && after.phase == LedgerPhase::Terminal
                && after.terminal_outcome == Some(TerminalOutcome::Stopped)
        }
        _ => false,
    }
}

fn validate_safe_fault_receipt(
    before: &PublicClusterState,
    after: &PublicClusterState,
    payloads: &[RecordPayload],
) -> bool {
    let Some(RecordPayload::SafeFault { consequence, .. }) = payloads.first() else {
        return false;
    };
    let Some(RecordPayload::Terminal { outcome }) = payloads.last() else {
        return false;
    };
    *consequence == *outcome
        && void_set(&payloads[1..payloads.len() - 1])
            == Some(before.active_dispatches.keys().cloned().collect())
        && after.phase == LedgerPhase::Terminal
        && after.terminal_outcome == Some(*outcome)
        && after.active_dispatches.is_empty()
}

fn validate_effect_intent_receipt(after: &PublicClusterState, payloads: &[RecordPayload]) -> bool {
    let [
        RecordPayload::EffectIntent {
            execution_id,
            effect_id,
            request_digest,
        },
    ] = payloads
    else {
        return false;
    };
    after.effects.get(effect_id).is_some_and(|effect| {
        effect.execution_id == *execution_id && effect.request_digest == *request_digest
    })
}

fn validate_effect_receipt(after: &PublicClusterState, payloads: &[RecordPayload]) -> bool {
    let [
        RecordPayload::EffectReceipt {
            effect_id,
            reconciliation_digest,
        },
    ] = payloads
    else {
        return false;
    };
    after
        .effects
        .get(effect_id)
        .is_some_and(|effect| effect.reconciliation_digest.as_ref() == Some(reconciliation_digest))
}

fn validate_update_receipt(
    receipt: &super::record::UpdateMutationReceipt,
    before: &PublicClusterState,
    after: &PublicClusterState,
    payloads: &[RecordPayload],
    position: Position,
) -> bool {
    matches!(payloads, [RecordPayload::LifecycleUpdate { .. }])
        && before.phase == LedgerPhase::Running
        && before.stop_mode.is_none()
        && result_identity_matches(
            receipt.result.generation.get(),
            receipt.result.run_id.as_str(),
            after,
        )
        && receipt.result.phase == Phase::Running
        && expected_operational(after).as_ref() == Some(&receipt.result.operational)
        && cursor_matches(receipt.result.at_cursor.as_str(), position)
}

fn validate_stop_receipt(
    receipt: &super::record::StopMutationReceipt,
    before: &PublicClusterState,
    after: &PublicClusterState,
    payloads: &[RecordPayload],
    position: Position,
) -> bool {
    let Some(RecordPayload::StopRequested {
        accepted_mode,
        effective_mode,
    }) = payloads.first()
    else {
        return false;
    };
    let terminal_required =
        *effective_mode == StopMode::Force || before.active_dispatches.is_empty();
    receipt.result.accepted_mode == *accepted_mode
        && receipt.result.effective_mode == *effective_mode
        && stop_payloads_match(before, payloads, *effective_mode, terminal_required)
        && result_identity_matches(
            receipt.result.generation.get(),
            receipt.result.run_id.as_str(),
            after,
        )
        && receipt.result.phase == protocol_phase(after.phase)
        && expected_operational(after).as_ref() == Some(&receipt.result.operational)
        && cursor_matches(receipt.result.at_cursor.as_str(), position)
}

fn stop_payloads_match(
    before: &PublicClusterState,
    payloads: &[RecordPayload],
    effective_mode: StopMode,
    terminal_required: bool,
) -> bool {
    let terminal_present = matches!(
        payloads.last(),
        Some(RecordPayload::Terminal {
            outcome: TerminalOutcome::Stopped
        })
    );
    let body_end = payloads.len().saturating_sub(usize::from(terminal_present));
    let voids = void_set(&payloads[1..body_end]);
    terminal_present == terminal_required
        && match effective_mode {
            StopMode::Drain => voids == Some(BTreeSet::new()),
            StopMode::Force => voids == Some(before.active_dispatches.keys().cloned().collect()),
        }
}

fn void_set(payloads: &[RecordPayload]) -> Option<BTreeSet<ExecutionId>> {
    let mut executions = BTreeSet::new();
    for payload in payloads {
        let RecordPayload::Void { execution_id } = payload else {
            return None;
        };
        if !executions.insert(execution_id.clone()) {
            return None;
        }
    }
    Some(executions)
}

fn validate_terminal_receipt(
    before: &PublicClusterState,
    after: &PublicClusterState,
    payloads: &[RecordPayload],
) -> bool {
    let [RecordPayload::Terminal { outcome }] = payloads else {
        return false;
    };
    before.active_dispatches.is_empty()
        && after.phase == LedgerPhase::Terminal
        && after.terminal_outcome == Some(*outcome)
}

fn validate_cleanup_receipt(after: &PublicClusterState, payloads: &[RecordPayload]) -> bool {
    let [
        RecordPayload::CleanupReceipt {
            resource_id,
            reconciliation_digest,
        },
    ] = payloads
    else {
        return false;
    };
    after.cleanup_receipts.get(resource_id) == Some(reconciliation_digest)
}

fn result_identity_matches(generation: u64, run_id: &str, state: &PublicClusterState) -> bool {
    Some(generation) == state.generation().map(LedgerGeneration::get)
        && Some(run_id) == state.run_id().map(LedgerRunId::as_str)
}

fn expected_operational(state: &PublicClusterState) -> Option<OperationalStatus> {
    let dispatch_state = match state.phase {
        LedgerPhase::Empty | LedgerPhase::Terminal => DispatchState::Stopped,
        LedgerPhase::Running => match state.stop_mode {
            Some(StopMode::Drain) => DispatchState::Draining,
            Some(StopMode::Force) => DispatchState::ForceStopping,
            None if state.suspended => DispatchState::Suspended,
            None => DispatchState::Active,
        },
    };
    Some(OperationalStatus {
        labels: state.labels.clone().unwrap_or_default(),
        log_level: state.log_level.unwrap_or_default(),
        dispatch_state,
        stop_mode: state.stop_mode,
        in_flight: u32::try_from(state.active_dispatches.len()).ok()?,
    })
}

fn protocol_phase(phase: LedgerPhase) -> Phase {
    match phase {
        LedgerPhase::Empty => Phase::Empty,
        LedgerPhase::Running => Phase::Running,
        LedgerPhase::Terminal => Phase::Finished,
    }
}

fn cursor_matches(cursor: &str, position: Position) -> bool {
    cursor == format!("ledger-{}", position.get())
}

fn receipt_is_deduped(receipt: &ClosedMutationReceipt) -> bool {
    match receipt {
        ClosedMutationReceipt::Admit(value) => value.deduped,
        ClosedMutationReceipt::Apply(value) => value.result.deduped,
        ClosedMutationReceipt::Dispatch(value) => value.deduped,
        ClosedMutationReceipt::Settle(value) => value.deduped,
        ClosedMutationReceipt::SafeFault(value)
        | ClosedMutationReceipt::EffectIntent(value)
        | ClosedMutationReceipt::EffectReceipt(value)
        | ClosedMutationReceipt::Terminalize(value)
        | ClosedMutationReceipt::Cleanup(value) => value.deduped,
        ClosedMutationReceipt::Update(value) => value.result.deduped,
        ClosedMutationReceipt::Stop(value) => value.result.deduped,
    }
}
