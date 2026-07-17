//! Native durable authority: domain semantics over one backend-neutral ordered store.

use std::sync::Arc;

use openengine_cluster_protocol::{canonical_value_bytes, CompiledGraphIr, GraphSpec, StopMode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::fault::{
    EngineFault, EvidenceClass, FaultContext, FaultFactory, FaultModule, ModuleEvidence,
};
use crate::observability::{NoopObservationSink, ObservationSink};

pub mod adapters;
pub mod fold;
pub mod identity;
pub mod record;
pub mod sqlite;
pub mod store;
mod validation;

pub use fold::{fold_records, LedgerPhase, PublicClusterState};
pub use identity::{
    AbsoluteDeadline, ExecutionId, IdempotencyId, LedgerGeneration, LedgerRunId, NodeInstanceId,
    OwnerFence, OwnerId, Position, ResourceId,
};
pub use record::{AdmissionManifest, RecordPayload, TerminalOutcome};
pub use sqlite::SqliteLedgerStore;
pub use store::{Clock, LedgerStore, MemoryLedgerStore, OpaqueMutationReceipt, StoreError, SystemClock};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationIdentity {
    pub key: IdempotencyId,
    pub fingerprint: [u8; 32],
}

impl MutationIdentity {
    pub fn for_value(key: IdempotencyId, method: &str, value: &Value) -> Result<Self, LedgerError> {
        let envelope = serde_json::json!({ "method": method, "params": value });
        let bytes = canonical_value_bytes(&envelope).map_err(|_| LedgerError::InvalidMutation)?;
        Ok(Self {
            key,
            fingerprint: Sha256::digest(bytes).into(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct AdmissionRequest {
    pub graph: GraphSpec,
    pub compiled_ir: CompiledGraphIr,
    pub input: Value,
    pub deadline: AbsoluteDeadline,
    pub mutation: MutationIdentity,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmissionReceipt {
    pub generation: LedgerGeneration,
    pub run_id: LedgerRunId,
    pub at_position: Position,
    pub deduped: bool,
}

#[derive(Clone, Debug)]
pub struct DispatchRequest {
    pub turn_id: String,
    pub mutation: MutationIdentity,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DispatchReceipt {
    pub node_instance_id: NodeInstanceId,
    pub execution_id: ExecutionId,
    pub at_position: Position,
    pub deduped: bool,
}

#[derive(Clone, Debug)]
pub struct SettlementRequest {
    pub execution_id: ExecutionId,
    pub output: Value,
    pub mutation: MutationIdentity,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettlementReceipt {
    pub execution_id: ExecutionId,
    pub accepted: bool,
    pub terminalized: bool,
    pub at_position: Position,
    pub deduped: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MutationReceipt {
    pub at_position: Position,
    pub deduped: bool,
}

#[derive(Clone, Debug, Error)]
pub enum LedgerError {
    #[error("ledger resource already exists")]
    AlreadyExists,
    #[error("ledger resource does not exist")]
    NotFound,
    #[error("ledger owner fence is no longer authoritative")]
    FenceRejected,
    #[error("ledger compare-and-append position changed concurrently")]
    ConcurrentMutation,
    #[error("idempotency key conflicts with an earlier mutation")]
    IdempotencyConflict,
    #[error("ledger mutation is illegal for the current state")]
    IllegalTransition,
    #[error("ledger mutation or bound is invalid")]
    InvalidMutation,
    #[error("ledger mutation was cancelled before durable append")]
    Cancelled,
    #[error("ledger operation failed with a safe engine fault")]
    Fault(EngineFault),
}

pub struct ClusterLedger {
    store: Arc<dyn LedgerStore>,
    resource_id: ResourceId,
    fence: Mutex<OwnerFence>,
    observations: Arc<dyn ObservationSink>,
}

impl ClusterLedger {
    pub async fn create(
        store: Arc<dyn LedgerStore>,
        resource_id: ResourceId,
        owner: OwnerId,
        fence_ttl_millis: u64,
    ) -> Result<Self, LedgerError> {
        Self::create_with_observations(
            store,
            resource_id,
            owner,
            fence_ttl_millis,
            Arc::new(NoopObservationSink),
        )
        .await
    }

    pub async fn create_with_observations(
        store: Arc<dyn LedgerStore>,
        resource_id: ResourceId,
        owner: OwnerId,
        fence_ttl_millis: u64,
        observations: Arc<dyn ObservationSink>,
    ) -> Result<Self, LedgerError> {
        let provisional = Self::without_fence(store, resource_id, observations);
        provisional
            .store
            .create_resource(&provisional.resource_id)
            .await
            .map_err(|error| provisional.map_store_error(error, FaultContext::Admission))?;
        let fence = provisional
            .store
            .acquire_fence(&provisional.resource_id, &owner, fence_ttl_millis)
            .await
            .map_err(|error| provisional.map_store_error(error, FaultContext::Admission))?;
        Ok(Self {
            fence: Mutex::new(fence),
            ..provisional
        })
    }

    pub async fn open(
        store: Arc<dyn LedgerStore>,
        resource_id: ResourceId,
        owner: OwnerId,
        fence_ttl_millis: u64,
    ) -> Result<Self, LedgerError> {
        Self::open_with_observations(
            store,
            resource_id,
            owner,
            fence_ttl_millis,
            Arc::new(NoopObservationSink),
        )
        .await
    }

    pub async fn open_with_observations(
        store: Arc<dyn LedgerStore>,
        resource_id: ResourceId,
        owner: OwnerId,
        fence_ttl_millis: u64,
        observations: Arc<dyn ObservationSink>,
    ) -> Result<Self, LedgerError> {
        let provisional = Self::without_fence(store, resource_id, observations);
        provisional
            .store
            .open_resource(&provisional.resource_id)
            .await
            .map_err(|error| provisional.map_store_error(error, FaultContext::Recovery))?;
        let fence = provisional
            .store
            .acquire_fence(&provisional.resource_id, &owner, fence_ttl_millis)
            .await
            .map_err(|error| provisional.map_store_error(error, FaultContext::Recovery))?;
        let ledger = Self {
            fence: Mutex::new(fence),
            ..provisional
        };
        ledger.replay().await?;
        Ok(ledger)
    }

    fn without_fence(
        store: Arc<dyn LedgerStore>,
        resource_id: ResourceId,
        observations: Arc<dyn ObservationSink>,
    ) -> Self {
        Self {
            store,
            resource_id,
            fence: Mutex::new(OwnerFence {
                owner: OwnerId::new("uninitialized").expect("static owner identity must be valid"),
                epoch: 0,
                expires_at_unix_millis: 0,
            }),
            observations,
        }
    }

    #[must_use]
    pub fn resource_id(&self) -> &ResourceId {
        &self.resource_id
    }

    pub async fn renew_fence(&self, ttl_millis: u64) -> Result<OwnerFence, LedgerError> {
        let mut fence = self.fence.lock().await;
        let renewed = self
            .store
            .renew_fence(&self.resource_id, &fence, ttl_millis)
            .await
            .map_err(|error| self.map_store_error(error, FaultContext::Recovery))?;
        *fence = renewed.clone();
        Ok(renewed)
    }

    pub async fn replay(&self) -> Result<PublicClusterState, LedgerError> {
        let (_, state) = self.load_prefix().await?;
        Ok(state)
    }

    async fn load_prefix(
        &self,
    ) -> Result<(store::CoherentPrefix, PublicClusterState), LedgerError> {
        let prefix = self
            .store
            .read_prefix(&self.resource_id)
            .await
            .map_err(|error| self.map_store_error(error, FaultContext::Recovery))?;
        if prefix.end.get()
            != u64::try_from(prefix.records.len()).map_err(|_| LedgerError::InvalidMutation)?
        {
            return Err(self.integrity_fault());
        }
        let state = fold::fold_records(&self.resource_id, &prefix.records)
            .map_err(|_| self.integrity_fault())?;
        if prefix.receipts != state.mutation_receipts {
            return Err(self.integrity_fault());
        }
        Ok((prefix, state))
    }

    pub async fn admit(&self, request: AdmissionRequest) -> Result<AdmissionReceipt, LedgerError> {
        let state = self.replay().await?;
        if let Some(receipt) =
            self.replay_receipt::<AdmissionReceipt>(&state, "admit", &request.mutation)?
        {
            return Ok(with_admission_deduped(receipt));
        }
        if !matches!(state.phase, LedgerPhase::Empty | LedgerPhase::Running)
            || (state.phase == LedgerPhase::Running
                && (state.suspended || !state.active_dispatches.is_empty()))
        {
            return Err(LedgerError::IllegalTransition);
        }
        let generation = state
            .generation()
            .map_or_else(|| LedgerGeneration::new(1), LedgerGeneration::checked_next)
            .map_err(|_| LedgerError::InvalidMutation)?;
        let manifest = admission_manifest(&request)?;
        let run_id = validation::run_id(generation, &manifest.graph_digest)
            .map_err(|_| LedgerError::InvalidMutation)?;
        let provisional = AdmissionReceipt {
            generation,
            run_id: run_id.clone(),
            at_position: Position::ZERO,
            deduped: false,
        };
        let outcome = self
            .append_mutation(
                &state,
                record::MutationKind::Admit,
                request.mutation,
                vec![RecordPayload::Admission {
                    generation,
                    run_id,
                    graph: Box::new(request.graph),
                    compiled_ir: Box::new(request.compiled_ir),
                    input: request.input,
                    manifest,
                }],
                provisional,
            )
            .await?;
        Ok(outcome)
    }

    pub async fn dispatch(&self, request: DispatchRequest) -> Result<DispatchReceipt, LedgerError> {
        let state = self.replay().await?;
        if let Some(receipt) =
            self.replay_receipt::<DispatchReceipt>(&state, "dispatch", &request.mutation)?
        {
            return Ok(with_dispatch_deduped(receipt));
        }
        if state.phase != LedgerPhase::Running
            || state.suspended
            || !valid_component(&request.turn_id)
        {
            return Err(LedgerError::IllegalTransition);
        }
        let generation = state.generation().ok_or(LedgerError::IllegalTransition)?;
        let ordinal = state
            .at_position
            .get()
            .checked_add(1)
            .ok_or(LedgerError::InvalidMutation)?;
        let node_instance_id = NodeInstanceId::new(format!("node-{}-{ordinal}", generation.get()))
            .map_err(|_| LedgerError::InvalidMutation)?;
        let execution_id = ExecutionId::new(format!("execution-{}-{ordinal}", generation.get()))
            .map_err(|_| LedgerError::InvalidMutation)?;
        let provisional = DispatchReceipt {
            node_instance_id: node_instance_id.clone(),
            execution_id: execution_id.clone(),
            at_position: Position::ZERO,
            deduped: false,
        };
        self.append_mutation(
            &state,
            record::MutationKind::Dispatch,
            request.mutation,
            vec![RecordPayload::Dispatch {
                node_instance_id,
                execution_id,
                turn_id: request.turn_id,
            }],
            provisional,
        )
        .await
    }

    pub async fn settle(
        &self,
        request: SettlementRequest,
    ) -> Result<SettlementReceipt, LedgerError> {
        let state = self.replay().await?;
        if let Some(receipt) =
            self.replay_receipt::<SettlementReceipt>(&state, "settle", &request.mutation)?
        {
            return Ok(with_settlement_deduped(receipt));
        }
        if state.phase != LedgerPhase::Running {
            return Err(LedgerError::IllegalTransition);
        }
        let accepted = !state.settlements.contains_key(&request.execution_id);
        if accepted && !state.active_dispatches.contains_key(&request.execution_id) {
            return Err(LedgerError::IllegalTransition);
        }
        let terminalized = accepted
            && state.stop_mode == Some(StopMode::Drain)
            && state.active_dispatches.len() == 1;
        let provisional = SettlementReceipt {
            execution_id: request.execution_id.clone(),
            accepted,
            terminalized,
            at_position: Position::ZERO,
            deduped: false,
        };
        let mut payloads = vec![RecordPayload::Settlement {
            execution_id: request.execution_id,
            output: request.output,
        }];
        if terminalized {
            payloads.push(RecordPayload::Terminal {
                outcome: TerminalOutcome::Stopped,
            });
        }
        self.append_mutation(
            &state,
            record::MutationKind::Settle,
            request.mutation,
            payloads,
            provisional,
        )
        .await
    }

    pub async fn persist_safe_fault(
        &self,
        execution_id: Option<ExecutionId>,
        fault: &EngineFault,
        outcome: TerminalOutcome,
        mutation: MutationIdentity,
    ) -> Result<MutationReceipt, LedgerError> {
        let encoded_fault = fault
            .encode_json()
            .map_err(|_| LedgerError::InvalidMutation)?;
        if outcome == TerminalOutcome::Succeeded {
            return Err(LedgerError::InvalidMutation);
        }
        self.append_simple(record::MutationKind::SafeFault, mutation, |state| {
            if state.phase != LedgerPhase::Running
                || execution_id
                    .as_ref()
                    .is_some_and(|id| !state.active_dispatches.contains_key(id))
            {
                return Err(LedgerError::IllegalTransition);
            }
            let mut payloads = vec![RecordPayload::SafeFault {
                execution_id,
                encoded_fault,
                consequence: outcome,
            }];
            payloads.extend(
                state
                    .active_dispatches
                    .keys()
                    .cloned()
                    .map(|execution_id| RecordPayload::Void { execution_id }),
            );
            payloads.push(RecordPayload::Terminal { outcome });
            Ok(payloads)
        })
        .await
    }

    pub async fn record_effect_intent(
        &self,
        execution_id: ExecutionId,
        effect_id: String,
        request_digest: String,
        mutation: MutationIdentity,
    ) -> Result<MutationReceipt, LedgerError> {
        self.append_simple(record::MutationKind::EffectIntent, mutation, |state| {
            if state.phase != LedgerPhase::Running
                || !state.active_dispatches.contains_key(&execution_id)
                || !valid_component(&effect_id)
                || !valid_digest(&request_digest)
                || state.effects.contains_key(&effect_id)
            {
                return Err(LedgerError::IllegalTransition);
            }
            Ok(vec![RecordPayload::EffectIntent {
                execution_id,
                effect_id,
                request_digest,
            }])
        })
        .await
    }

    pub async fn reconcile_effect(
        &self,
        effect_id: String,
        reconciliation_digest: String,
        mutation: MutationIdentity,
    ) -> Result<MutationReceipt, LedgerError> {
        self.append_simple(record::MutationKind::EffectReceipt, mutation, |state| {
            if !matches!(state.phase, LedgerPhase::Running | LedgerPhase::Terminal)
                || !valid_digest(&reconciliation_digest)
                || state.effects.get(&effect_id).is_none_or(|effect| {
                    effect
                        .reconciliation_digest
                        .as_ref()
                        .is_some_and(|existing| existing != &reconciliation_digest)
                })
            {
                return Err(LedgerError::IllegalTransition);
            }
            Ok(vec![RecordPayload::EffectReceipt {
                effect_id,
                reconciliation_digest,
            }])
        })
        .await
    }

    pub async fn terminalize(
        &self,
        outcome: TerminalOutcome,
        mutation: MutationIdentity,
    ) -> Result<MutationReceipt, LedgerError> {
        self.append_simple(record::MutationKind::Terminalize, mutation, |state| {
            if state.phase != LedgerPhase::Running || !state.active_dispatches.is_empty() {
                return Err(LedgerError::IllegalTransition);
            }
            Ok(vec![RecordPayload::Terminal { outcome }])
        })
        .await
    }

    pub async fn record_cleanup(
        &self,
        cleanup_resource_id: String,
        reconciliation_digest: String,
        mutation: MutationIdentity,
    ) -> Result<MutationReceipt, LedgerError> {
        self.append_simple(record::MutationKind::Cleanup, mutation, |state| {
            if state.phase != LedgerPhase::Terminal
                || cleanup_resource_id.is_empty()
                || !valid_component(&cleanup_resource_id)
                || !valid_digest(&reconciliation_digest)
                || state
                    .cleanup_receipts
                    .get(&cleanup_resource_id)
                    .is_some_and(|existing| existing != &reconciliation_digest)
            {
                return Err(LedgerError::IllegalTransition);
            }
            Ok(vec![RecordPayload::CleanupReceipt {
                resource_id: cleanup_resource_id,
                reconciliation_digest,
            }])
        })
        .await
    }

    pub async fn remove_terminal(&self) -> Result<(), LedgerError> {
        let state = self.replay().await?;
        if state.phase != LedgerPhase::Terminal
            || state
                .effects
                .values()
                .any(|effect| effect.reconciliation_digest.is_none())
        {
            return Err(LedgerError::IllegalTransition);
        }
        let fence = self.fence.lock().await.clone();
        self.store
            .remove_resource(&self.resource_id, &fence, state.at_position)
            .await
            .map_err(|error| self.map_store_error(error, FaultContext::Cleanup))
    }

    async fn append_simple<F>(
        &self,
        kind: record::MutationKind,
        mutation: MutationIdentity,
        payloads: F,
    ) -> Result<MutationReceipt, LedgerError>
    where
        F: FnOnce(&PublicClusterState) -> Result<Vec<RecordPayload>, LedgerError>,
    {
        let state = self.replay().await?;
        if let Some(receipt) =
            self.replay_receipt::<MutationReceipt>(&state, kind.method(), &mutation)?
        {
            return Ok(MutationReceipt {
                deduped: true,
                ..receipt
            });
        }
        let payloads = payloads(&state)?;
        self.append_mutation(
            &state,
            kind,
            mutation,
            payloads,
            MutationReceipt {
                at_position: Position::ZERO,
                deduped: false,
            },
        )
        .await
    }

    fn replay_receipt<T: DeserializeOwned>(
        &self,
        state: &PublicClusterState,
        method: &str,
        mutation: &MutationIdentity,
    ) -> Result<Option<T>, LedgerError> {
        let Some(receipt) = state.mutation_receipts.get(&mutation.key) else {
            return Ok(None);
        };
        if receipt.method != method || receipt.fingerprint != mutation.fingerprint {
            return Err(LedgerError::IdempotencyConflict);
        }
        serde_json::from_slice(&receipt.value)
            .map(Some)
            .map_err(|_| self.integrity_fault())
    }

    async fn append_mutation<T>(
        &self,
        state: &PublicClusterState,
        kind: record::MutationKind,
        mutation: MutationIdentity,
        payloads: Vec<RecordPayload>,
        receipt: T,
    ) -> Result<T, LedgerError>
    where
        T: Serialize + DeserializeOwned + ReceiptPosition,
    {
        self.append_mutation_guarded(state, kind, mutation, payloads, receipt, || Ok(()))
            .await
    }

    async fn append_mutation_guarded<T, G>(
        &self,
        state: &PublicClusterState,
        kind: record::MutationKind,
        mutation: MutationIdentity,
        mut payloads: Vec<RecordPayload>,
        mut receipt: T,
        before_append: G,
    ) -> Result<T, LedgerError>
    where
        T: Serialize + DeserializeOwned + ReceiptPosition,
        G: FnOnce() -> Result<(), LedgerError>,
    {
        let (prefix, current_state) = self.load_prefix().await?;
        if prefix.end != state.at_position {
            if let Some(mut committed) =
                self.replay_receipt::<T>(&current_state, kind.method(), &mutation)?
            {
                committed.mark_deduped();
                return Ok(committed);
            }
            return Err(LedgerError::ConcurrentMutation);
        }
        let receipt_position = state
            .at_position
            .get()
            .checked_add(u64::try_from(payloads.len()).map_err(|_| LedgerError::InvalidMutation)?)
            .and_then(|position| position.checked_add(1))
            .ok_or(LedgerError::InvalidMutation)?;
        receipt.set_position(
            Position::new(receipt_position).map_err(|_| LedgerError::InvalidMutation)?,
        );
        let receipt_value =
            serde_json::to_value(&receipt).map_err(|_| LedgerError::InvalidMutation)?;
        let receipt_bytes =
            canonical_value_bytes(&receipt_value).map_err(|_| LedgerError::InvalidMutation)?;
        let closed_receipt = kind
            .close(&receipt_bytes)
            .map_err(|_| LedgerError::InvalidMutation)?;
        let opaque = OpaqueMutationReceipt {
            key: mutation.key.clone(),
            method: kind.method().to_owned(),
            fingerprint: mutation.fingerprint,
            value: receipt_bytes.clone(),
            at_position: Position::new(receipt_position)
                .map_err(|_| LedgerError::InvalidMutation)?,
        };
        opaque
            .validate()
            .map_err(|_| LedgerError::InvalidMutation)?;
        payloads.push(RecordPayload::MutationReceipt {
            key: mutation.key,
            fingerprint: mutation.fingerprint,
            receipt: closed_receipt,
        });
        let mut records = Vec::with_capacity(payloads.len());
        let mut sequence = state.at_position;
        let mut previous_hash = prefix
            .records
            .last()
            .map_or([0; 32], |record| record.record_hash);
        for payload in payloads {
            sequence = sequence
                .checked_next()
                .map_err(|_| LedgerError::InvalidMutation)?;
            let record = record::LedgerRecord::new(
                self.resource_id.clone(),
                sequence,
                &payload,
                previous_hash,
            )
            .map_err(|_| LedgerError::InvalidMutation)?;
            previous_hash = record.record_hash;
            records.push(record);
        }
        let mut candidate = prefix.records;
        candidate.extend(records.iter().cloned());
        fold::fold_records(&self.resource_id, &candidate)
            .map_err(|_| LedgerError::IllegalTransition)?;
        let fence = self.fence.lock().await.clone();
        before_append()?;
        let outcome = self
            .store
            .compare_and_append(
                &self.resource_id,
                store::AppendRequest {
                    expected_position: state.at_position,
                    fence,
                    records,
                    receipt: Some(opaque.clone()),
                },
            )
            .await
            .map_err(|error| self.map_store_error(error, FaultContext::Execution))?;
        let committed = outcome.receipt.ok_or_else(|| self.integrity_fault())?;
        if committed.method != opaque.method
            || committed.fingerprint != opaque.fingerprint
            || committed.key != opaque.key
        {
            return Err(self.integrity_fault());
        }
        if !outcome.replayed && (outcome.position != sequence || committed.value != opaque.value) {
            return Err(self.integrity_fault());
        }
        let mut decoded: T =
            serde_json::from_slice(&committed.value).map_err(|_| self.integrity_fault())?;
        if outcome.replayed {
            decoded.mark_deduped();
        }
        Ok(decoded)
    }

    fn map_store_error(&self, error: StoreError, context: FaultContext) -> LedgerError {
        match error {
            StoreError::AlreadyExists => LedgerError::AlreadyExists,
            StoreError::NotFound => LedgerError::NotFound,
            StoreError::FenceRejected => LedgerError::FenceRejected,
            StoreError::PositionConflict { .. } => LedgerError::ConcurrentMutation,
            StoreError::ReceiptConflict => LedgerError::IdempotencyConflict,
            StoreError::BoundExceeded(_) => LedgerError::InvalidMutation,
            StoreError::Corrupt => self.integrity_fault(),
            StoreError::StorageUnavailable => {
                LedgerError::Fault(FaultFactory::new(self.observations.as_ref()).create(
                    ModuleEvidence::new(FaultModule::Storage, context, EvidenceClass::Unavailable),
                ))
            }
        }
    }

    fn integrity_fault(&self) -> LedgerError {
        LedgerError::Fault(FaultFactory::new(self.observations.as_ref()).create(
            ModuleEvidence::new(
                FaultModule::Storage,
                FaultContext::Recovery,
                EvidenceClass::IntegrityFailure,
            ),
        ))
    }
}

trait ReceiptPosition {
    fn set_position(&mut self, position: Position);
    fn mark_deduped(&mut self);
}

impl ReceiptPosition for AdmissionReceipt {
    fn set_position(&mut self, position: Position) {
        self.at_position = position;
    }

    fn mark_deduped(&mut self) {
        self.deduped = true;
    }
}

impl ReceiptPosition for DispatchReceipt {
    fn set_position(&mut self, position: Position) {
        self.at_position = position;
    }

    fn mark_deduped(&mut self) {
        self.deduped = true;
    }
}

impl ReceiptPosition for SettlementReceipt {
    fn set_position(&mut self, position: Position) {
        self.at_position = position;
    }

    fn mark_deduped(&mut self) {
        self.deduped = true;
    }
}

impl ReceiptPosition for MutationReceipt {
    fn set_position(&mut self, position: Position) {
        self.at_position = position;
    }

    fn mark_deduped(&mut self) {
        self.deduped = true;
    }
}

fn admission_manifest(request: &AdmissionRequest) -> Result<AdmissionManifest, LedgerError> {
    validation::admission_manifest(
        &request.graph,
        &request.compiled_ir,
        &request.input,
        request.deadline,
    )
    .map_err(|_| LedgerError::InvalidMutation)
}

fn with_admission_deduped(receipt: AdmissionReceipt) -> AdmissionReceipt {
    AdmissionReceipt {
        deduped: true,
        ..receipt
    }
}

fn with_dispatch_deduped(receipt: DispatchReceipt) -> DispatchReceipt {
    DispatchReceipt {
        deduped: true,
        ..receipt
    }
}

fn with_settlement_deduped(receipt: SettlementReceipt) -> SettlementReceipt {
    SettlementReceipt {
        deduped: true,
        ..receipt
    }
}

use validation::{valid_component, valid_digest};
