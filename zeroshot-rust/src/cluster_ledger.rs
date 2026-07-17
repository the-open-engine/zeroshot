use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::fault::{
    EngineFault, EvidenceClass, FaultContext, FaultFactory, FaultModule, ModuleEvidence,
};
use crate::observability::{NoopObservationSink, ObservationSink};

pub mod adapters;
pub mod mutations;
pub mod record;
pub mod replay;
pub mod store;

pub use mutations::{
    AdmissionRequest, CommitResult, DispatchAllocation, SafeFaultConsequence, SafeFaultResult,
    SettlementResult,
};
pub use record::{CanonicalDigest, EffectId, ExecutionId, GenerationId, NodeInstanceId, RunSequence};
pub use replay::{replay, ReplayState};
pub use store::{LedgerStore, OwnerId, ResourceId};

use record::{RecordPayload, StoredRecord};
use replay::ReplayError;
use store::{
    AppendBatch, AppendGuard, AppendOutcome, Fence, IdempotencyId, MutationReceipt, StoreError,
};

#[derive(Clone)]
pub struct ClusterLedger {
    store: Arc<dyn LedgerStore>,
    resource: ResourceId,
    fence: Arc<Mutex<Fence>>,
    observations: Arc<dyn ObservationSink>,
}

pub(crate) struct MutationIdentity {
    key: IdempotencyId,
    method: &'static str,
    fingerprint: [u8; 32],
}

impl MutationIdentity {
    pub(crate) const fn new(
        key: IdempotencyId,
        method: &'static str,
        fingerprint: [u8; 32],
    ) -> Self {
        Self {
            key,
            method,
            fingerprint,
        }
    }
}

struct PreparedCommit {
    key: IdempotencyId,
    method: &'static str,
    fingerprint: [u8; 32],
    receipt: MutationReceipt,
    batch: AppendBatch,
}

impl ClusterLedger {
    pub async fn create(
        store: Arc<dyn LedgerStore>,
        resource: ResourceId,
        owner: OwnerId,
        fence_ttl_ms: u64,
    ) -> Result<Self, LedgerError> {
        Self::create_with_observations(
            store,
            resource,
            owner,
            fence_ttl_ms,
            Arc::new(NoopObservationSink),
        )
        .await
    }

    pub async fn create_with_observations(
        store: Arc<dyn LedgerStore>,
        resource: ResourceId,
        owner: OwnerId,
        fence_ttl_ms: u64,
        observations: Arc<dyn ObservationSink>,
    ) -> Result<Self, LedgerError> {
        let (_, fence) = store
            .create_fenced(&resource, &owner, fence_ttl_ms)
            .await
            .map_err(|error| {
                map_store_error(observations.as_ref(), FaultContext::Admission, error)
            })?;
        Ok(Self {
            store,
            resource,
            fence: Arc::new(Mutex::new(fence)),
            observations,
        })
    }

    pub async fn open(
        store: Arc<dyn LedgerStore>,
        resource: ResourceId,
        owner: OwnerId,
        fence_ttl_ms: u64,
    ) -> Result<Self, LedgerError> {
        Self::open_with_observations(
            store,
            resource,
            owner,
            fence_ttl_ms,
            Arc::new(NoopObservationSink),
        )
        .await
    }

    pub async fn open_with_observations(
        store: Arc<dyn LedgerStore>,
        resource: ResourceId,
        owner: OwnerId,
        fence_ttl_ms: u64,
        observations: Arc<dyn ObservationSink>,
    ) -> Result<Self, LedgerError> {
        store.open(&resource).await.map_err(|error| {
            map_store_error(observations.as_ref(), FaultContext::Recovery, error)
        })?;
        let snapshot = store.read_prefix(&resource, None).await.map_err(|error| {
            map_store_error(observations.as_ref(), FaultContext::Recovery, error)
        })?;
        replay::replay(&snapshot, &resource).map_err(|error| {
            let fault = FaultFactory::new(observations.as_ref()).create(ModuleEvidence::new(
                FaultModule::Storage,
                FaultContext::Recovery,
                EvidenceClass::IntegrityFailure,
            ));
            LedgerError {
                kind: LedgerErrorKind::Replay(error),
                fault,
            }
        })?;
        let fence = store
            .acquire_fence(&resource, &owner, fence_ttl_ms)
            .await
            .map_err(|error| {
                map_store_error(observations.as_ref(), FaultContext::Recovery, error)
            })?;
        Ok(Self {
            store,
            resource,
            fence: Arc::new(Mutex::new(fence)),
            observations,
        })
    }

    #[must_use]
    pub fn resource(&self) -> &ResourceId {
        &self.resource
    }

    #[must_use]
    pub fn fence(&self) -> Fence {
        self.fence
            .lock()
            .expect("cluster ledger fence mutex must not be poisoned")
            .clone()
    }

    pub async fn renew_fence(&self, ttl_ms: u64) -> Result<Fence, LedgerError> {
        let current = self.fence();
        let renewed = self
            .store
            .renew_fence(&current, ttl_ms)
            .await
            .map_err(|error| self.store_error(FaultContext::Recovery, error))?;
        *self
            .fence
            .lock()
            .expect("cluster ledger fence mutex must not be poisoned") = renewed.clone();
        Ok(renewed)
    }

    pub async fn state(&self) -> Result<ReplayState, LedgerError> {
        self.read_state(FaultContext::Recovery).await
    }

    pub(crate) async fn validated_state(
        &self,
        context: FaultContext,
    ) -> Result<ReplayState, LedgerError> {
        self.store
            .check_fence(&self.fence())
            .await
            .map_err(|error| self.store_error(context, error))?;
        self.read_state(context).await
    }

    async fn read_state(&self, context: FaultContext) -> Result<ReplayState, LedgerError> {
        let snapshot = self
            .store
            .read_prefix(&self.resource, None)
            .await
            .map_err(|error| self.store_error(context, error))?;
        replay::replay(&snapshot, &self.resource).map_err(|error| self.replay_error(context, error))
    }

    pub async fn remove_terminal(&self) -> Result<(), LedgerError> {
        let state = self.state().await?;
        if state.terminal_outcome.is_none() {
            return Err(self.domain_error(FaultContext::Cleanup, LedgerErrorKind::TerminalRequired));
        }
        self.store
            .remove(&self.resource, &self.fence(), state.position)
            .await
            .map_err(|error| self.store_error(FaultContext::Cleanup, error))
    }

    pub(crate) async fn commit<T>(
        &self,
        context: FaultContext,
        validated_state: &ReplayState,
        mutation: MutationIdentity,
        payloads: Vec<RecordPayload>,
        response: &T,
    ) -> Result<CommitResult<T>, LedgerError>
    where
        T: DeserializeOwned + Serialize,
    {
        self.commit_guarded(
            context,
            validated_state,
            mutation,
            payloads,
            response,
            AppendGuard::allow(),
        )
        .await
    }

    pub(crate) async fn commit_guarded<T>(
        &self,
        context: FaultContext,
        validated_state: &ReplayState,
        mutation: MutationIdentity,
        payloads: Vec<RecordPayload>,
        response: &T,
        guard: AppendGuard,
    ) -> Result<CommitResult<T>, LedgerError>
    where
        T: DeserializeOwned + Serialize,
    {
        let prepared =
            self.prepare_commit(context, validated_state, mutation, payloads, response)?;
        let outcome = self
            .store
            .compare_and_append_guarded(
                &self.resource,
                &self.fence(),
                validated_state.position,
                prepared.batch.clone(),
                guard,
            )
            .await;
        self.resolve_commit(context, prepared, outcome).await
    }

    fn prepare_commit<T>(
        &self,
        context: FaultContext,
        validated_state: &ReplayState,
        mutation: MutationIdentity,
        payloads: Vec<RecordPayload>,
        response: &T,
    ) -> Result<PreparedCommit, LedgerError>
    where
        T: Serialize,
    {
        let MutationIdentity {
            key,
            method,
            fingerprint,
        } = mutation;
        let (mut records, mut position, previous_hash) =
            self.build_mutation_records(context, validated_state, payloads)?;
        let encoded_response = serde_json::to_vec(response)
            .map_err(|_| self.domain_error(context, LedgerErrorKind::Encoding))?;
        position = position
            .checked_add(1)
            .map_err(|_| self.domain_error(context, LedgerErrorKind::BoundViolation))?;
        let receipt = MutationReceipt {
            idempotency_key: key.clone(),
            method: method.to_owned(),
            fingerprint,
            response: encoded_response,
            committed_position: position,
        };
        let receipt_record = StoredRecord::build(
            self.resource.clone(),
            position,
            &RecordPayload::MutationReceipt {
                receipt: receipt.clone(),
            },
            previous_hash,
        )
        .map_err(|_| self.domain_error(context, LedgerErrorKind::BoundViolation))?;
        records.push(receipt_record);
        let batch = AppendBatch::new(records, Some(receipt.clone()))
            .map_err(|_| self.domain_error(context, LedgerErrorKind::BoundViolation))?;
        Ok(PreparedCommit {
            key,
            method,
            fingerprint,
            receipt,
            batch,
        })
    }

    fn build_mutation_records(
        &self,
        context: FaultContext,
        validated_state: &ReplayState,
        payloads: Vec<RecordPayload>,
    ) -> Result<(Vec<StoredRecord>, store::Position, [u8; 32]), LedgerError> {
        let mut position = validated_state.position;
        let mut previous_hash = validated_state.last_hash;
        let mut records = Vec::with_capacity(payloads.len().saturating_add(1));
        for payload in payloads {
            position = position
                .checked_add(1)
                .map_err(|_| self.domain_error(context, LedgerErrorKind::BoundViolation))?;
            let record =
                StoredRecord::build(self.resource.clone(), position, &payload, previous_hash)
                    .map_err(|_| self.domain_error(context, LedgerErrorKind::BoundViolation))?;
            previous_hash = record.record_hash;
            records.push(record);
        }
        Ok((records, position, previous_hash))
    }

    async fn resolve_commit<T>(
        &self,
        context: FaultContext,
        prepared: PreparedCommit,
        outcome: Result<AppendOutcome, StoreError>,
    ) -> Result<CommitResult<T>, LedgerError>
    where
        T: DeserializeOwned,
    {
        let PreparedCommit {
            key,
            method,
            fingerprint,
            receipt,
            ..
        } = prepared;
        match outcome {
            Ok(AppendOutcome::Committed(committed)) if committed == receipt => {
                self.decode_receipt(context, committed, method, fingerprint, false)
            }
            Ok(AppendOutcome::Committed(_)) => {
                Err(self.domain_error(context, LedgerErrorKind::ReceiptCorrupt))
            }
            Ok(AppendOutcome::Replayed(committed)) => {
                let recovered = self
                    .validated_receipt(context, &key)
                    .await?
                    .ok_or_else(|| self.domain_error(context, LedgerErrorKind::ReceiptCorrupt))?;
                if recovered != committed {
                    return Err(self.domain_error(context, LedgerErrorKind::ReceiptCorrupt));
                }
                self.decode_receipt(context, recovered, method, fingerprint, true)
            }
            Ok(AppendOutcome::CommittedWithoutReceipt(_)) => {
                Err(self.domain_error(context, LedgerErrorKind::ReceiptCorrupt))
            }
            Err(StoreError::FailureInjected(store::FailPoint::AfterCommitBeforeResponse)) => {
                let recovered = self
                    .validated_receipt(context, &key)
                    .await?
                    .ok_or_else(|| self.domain_error(context, LedgerErrorKind::ReceiptCorrupt))?;
                self.decode_receipt(context, recovered, method, fingerprint, true)
            }
            Err(StoreError::IdempotencyConflict) => {
                Err(self.domain_error(context, LedgerErrorKind::IdempotencyConflict))
            }
            Err(StoreError::PositionConflict { .. }) => {
                Err(self.domain_error(context, LedgerErrorKind::InvalidLifecycle))
            }
            Err(error) => Err(self.store_error(context, error)),
        }
    }

    fn decode_receipt<T>(
        &self,
        context: FaultContext,
        receipt: MutationReceipt,
        method: &str,
        fingerprint: [u8; 32],
        replayed: bool,
    ) -> Result<CommitResult<T>, LedgerError>
    where
        T: DeserializeOwned,
    {
        if receipt.method != method || receipt.fingerprint != fingerprint {
            return Err(self.domain_error(context, LedgerErrorKind::IdempotencyConflict));
        }
        let value = serde_json::from_slice(&receipt.response)
            .map_err(|_| self.domain_error(context, LedgerErrorKind::ReceiptCorrupt))?;
        Ok(CommitResult {
            value,
            position: receipt.committed_position,
            replayed,
        })
    }

    pub(crate) fn existing_receipt<T>(
        &self,
        context: FaultContext,
        state: &ReplayState,
        key: &IdempotencyId,
        method: &str,
        fingerprint: [u8; 32],
    ) -> Result<Option<CommitResult<T>>, LedgerError>
    where
        T: DeserializeOwned,
    {
        state
            .mutation_receipts
            .get(key)
            .cloned()
            .map(|receipt| self.decode_receipt(context, receipt, method, fingerprint, true))
            .transpose()
    }

    pub(crate) async fn receipt(
        &self,
        context: FaultContext,
        key: &IdempotencyId,
    ) -> Result<Option<MutationReceipt>, LedgerError> {
        self.validated_receipt(context, key).await
    }

    async fn validated_receipt(
        &self,
        context: FaultContext,
        key: &IdempotencyId,
    ) -> Result<Option<MutationReceipt>, LedgerError> {
        Ok(self
            .validated_state(context)
            .await?
            .mutation_receipts
            .get(key)
            .cloned())
    }

    pub(crate) fn domain_error(&self, context: FaultContext, kind: LedgerErrorKind) -> LedgerError {
        let evidence = match kind {
            LedgerErrorKind::BoundViolation => EvidenceClass::ResourceExhausted,
            LedgerErrorKind::IdempotencyConflict
            | LedgerErrorKind::InvalidLifecycle
            | LedgerErrorKind::InvalidSettlement
            | LedgerErrorKind::TerminalRequired => EvidenceClass::InvariantViolation,
            LedgerErrorKind::ReceiptCorrupt | LedgerErrorKind::Encoding => {
                EvidenceClass::IntegrityFailure
            }
            LedgerErrorKind::Storage(_) | LedgerErrorKind::Replay(_) => {
                EvidenceClass::IntegrityFailure
            }
        };
        let module = if matches!(
            kind,
            LedgerErrorKind::Storage(_) | LedgerErrorKind::ReceiptCorrupt
        ) {
            FaultModule::Storage
        } else {
            FaultModule::Engine
        };
        let fault = FaultFactory::new(self.observations.as_ref())
            .create(ModuleEvidence::new(module, context, evidence));
        LedgerError { kind, fault }
    }

    fn store_error(&self, context: FaultContext, error: StoreError) -> LedgerError {
        map_store_error(self.observations.as_ref(), context, error)
    }

    fn replay_error(&self, context: FaultContext, error: ReplayError) -> LedgerError {
        let fault = FaultFactory::new(self.observations.as_ref()).create(ModuleEvidence::new(
            FaultModule::Storage,
            context,
            EvidenceClass::IntegrityFailure,
        ));
        LedgerError {
            kind: LedgerErrorKind::Replay(error),
            fault,
        }
    }
}

fn map_store_error(
    observations: &dyn ObservationSink,
    context: FaultContext,
    error: StoreError,
) -> LedgerError {
    let class = match error {
        StoreError::BatchRecordBound
        | StoreError::BatchByteBound
        | StoreError::ReceiptTooLarge
        | StoreError::InvalidLimit
        | StoreError::PositionOverflow => EvidenceClass::ResourceExhausted,
        StoreError::FenceHeld | StoreError::FenceExpired | StoreError::StaleFence => {
            EvidenceClass::Unavailable
        }
        StoreError::ResourceNotFound | StoreError::ResourceExists => EvidenceClass::Unavailable,
        StoreError::Storage | StoreError::FailureInjected(_) => EvidenceClass::Unavailable,
        _ => EvidenceClass::IntegrityFailure,
    };
    let fault = FaultFactory::new(observations).create(ModuleEvidence::new(
        FaultModule::Storage,
        context,
        class,
    ));
    LedgerError {
        kind: LedgerErrorKind::Storage(error),
        fault,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LedgerErrorKind {
    Storage(StoreError),
    Replay(ReplayError),
    BoundViolation,
    IdempotencyConflict,
    InvalidLifecycle,
    InvalidSettlement,
    TerminalRequired,
    ReceiptCorrupt,
    Encoding,
}

pub struct LedgerError {
    kind: LedgerErrorKind,
    fault: EngineFault,
}

impl LedgerError {
    #[must_use]
    pub const fn kind(&self) -> &LedgerErrorKind {
        &self.kind
    }

    #[must_use]
    pub const fn fault(&self) -> &EngineFault {
        &self.fault
    }
}

impl fmt::Debug for LedgerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LedgerError")
            .field("kind", &self.kind)
            .field("fault_code", &self.fault.code())
            .finish()
    }
}

impl fmt::Display for LedgerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cluster ledger operation failed: {:?}",
            self.kind
        )
    }
}

impl Error for LedgerError {}
