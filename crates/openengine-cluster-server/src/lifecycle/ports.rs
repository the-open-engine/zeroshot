//! Atomic operational lifecycle store contract.

use async_trait::async_trait;
use openengine_cluster_protocol::{
    ApplyResult, Cursor, DispatchState, Generation, IdempotencyKey, Labels, LogLevel,
    OperationalStatus, RequestFingerprint, RunId, StopMode, StopParams, StopResult, UpdateParams,
    UpdateResult,
};
use serde_json::Value;

use crate::admission::{CancellationObserver, StoreError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationReceipt {
    Apply(ApplyResult),
    Update(UpdateResult),
    Stop(StopResult),
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LeaseId(String);

impl LeaseId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TurnId(String);

impl TurnId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatchPermit {
    pub lease_id: LeaseId,
    pub turn_id: TurnId,
    pub cancellation: CancellationObserver,
    pub at_cursor: Cursor,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedCompletion {
    pub lease_id: LeaseId,
    pub output: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionResult {
    pub turn_id: TurnId,
    pub at_cursor: Cursor,
    pub terminalized: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedTurn {
    pub turn_id: TurnId,
    pub output: Value,
    pub cursor: Cursor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoidTurn {
    pub turn_id: TurnId,
    pub cursor: Cursor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleRecord {
    pub cursor: Cursor,
    pub event: LifecycleEvent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LifecycleEvent {
    Dispatched {
        turn_id: TurnId,
    },
    Updated {
        labels: Option<Labels>,
        log_level: Option<LogLevel>,
        suspended: Option<bool>,
    },
    StopRequested {
        accepted_mode: StopMode,
        effective_mode: StopMode,
    },
    Verified {
        turn_id: TurnId,
    },
    Void {
        turn_id: TurnId,
    },
    Finished {
        mode: StopMode,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LifecycleSnapshot {
    pub operational: Option<OperationalStatus>,
    pub latest_cursor: Option<Cursor>,
    pub records: Vec<LifecycleRecord>,
    pub verified_turns: Vec<VerifiedTurn>,
    pub void_turns: Vec<VoidTurn>,
}

impl LifecycleSnapshot {
    #[must_use]
    pub fn dispatch_state(&self) -> Option<DispatchState> {
        self.operational
            .as_ref()
            .map(|status| status.dispatch_state)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateProposal {
    pub params: UpdateParams,
    pub fingerprint: RequestFingerprint,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StopProposal {
    pub params: StopParams,
    pub fingerprint: RequestFingerprint,
}

#[async_trait]
pub trait LifecycleStore: Send + Sync {
    async fn read_lifecycle_snapshot(&self) -> Result<LifecycleSnapshot, StoreError>;

    async fn update_lifecycle(&self, proposal: UpdateProposal) -> Result<UpdateResult, StoreError>;

    async fn stop_lifecycle(&self, proposal: StopProposal) -> Result<StopResult, StoreError>;

    async fn acquire_dispatch(&self, turn_id: TurnId) -> Result<DispatchPermit, StoreError>;

    async fn complete_dispatch(
        &self,
        completion: VerifiedCompletion,
    ) -> Result<CompletionResult, StoreError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleIdentity {
    pub generation: Generation,
    pub run_id: RunId,
    pub idempotency_key: IdempotencyKey,
}
