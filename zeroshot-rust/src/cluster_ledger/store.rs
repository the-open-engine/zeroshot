use std::error::Error;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::record::{
    StoredRecord, MAX_APPEND_BATCH_BYTES, MAX_APPEND_RECORDS, MAX_RECORD_PAYLOAD_BYTES,
};

pub mod fake;
pub mod sqlite;

pub const MAX_IDENTIFIER_BYTES: usize = 256;
pub const MAX_DISCOVERY_PAGE: usize = 1_024;

macro_rules! bounded_identifier {
    ($name:ident, $label:literal) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, StoreError> {
                let value = value.into();
                validate_identifier(&value, $label)?;
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

bounded_identifier!(ResourceId, "resource");
bounded_identifier!(OwnerId, "owner");
bounded_identifier!(IdempotencyId, "idempotency");

fn validate_identifier(value: &str, label: &'static str) -> Result<(), StoreError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES || value.chars().any(char::is_control)
    {
        return Err(StoreError::InvalidIdentifier(label));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Position(u64);

impl Position {
    pub const ZERO: Self = Self(0);
    pub const MAX: Self = Self(i64::MAX as u64);

    pub fn new(value: u64) -> Result<Self, StoreError> {
        if value > Self::MAX.0 {
            return Err(StoreError::PositionOverflow);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn checked_add(self, amount: usize) -> Result<Self, StoreError> {
        let amount = u64::try_from(amount).map_err(|_| StoreError::PositionOverflow)?;
        Self::new(
            self.0
                .checked_add(amount)
                .ok_or(StoreError::PositionOverflow)?,
        )
    }
}

impl<'de> Deserialize<'de> for Position {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::new(u64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Fence {
    pub resource: ResourceId,
    pub owner: OwnerId,
    pub epoch: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MutationReceipt {
    pub idempotency_key: IdempotencyId,
    pub method: String,
    pub fingerprint: [u8; 32],
    pub response: Vec<u8>,
    pub committed_position: Position,
}

impl MutationReceipt {
    pub fn validate(&self) -> Result<(), StoreError> {
        validate_identifier(&self.method, "method")?;
        if self.committed_position == Position::ZERO {
            return Err(StoreError::Corrupt("zero receipt position"));
        }
        if self.response.len() > super::record::MAX_RECORD_PAYLOAD_BYTES {
            return Err(StoreError::ReceiptTooLarge);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppendBatch {
    pub records: Vec<StoredRecord>,
    pub receipt: Option<MutationReceipt>,
}

impl AppendBatch {
    pub fn new(
        records: Vec<StoredRecord>,
        receipt: Option<MutationReceipt>,
    ) -> Result<Self, StoreError> {
        let batch = Self { records, receipt };
        batch.validate()?;
        Ok(batch)
    }

    pub fn validate(&self) -> Result<(), StoreError> {
        if self.records.len() > MAX_APPEND_RECORDS
            || (self.records.is_empty() && self.receipt.is_none())
        {
            return Err(StoreError::BatchRecordBound);
        }
        let mut encoded_bytes = self.records.iter().try_fold(0_usize, |total, record| {
            if record.payload.len() > MAX_RECORD_PAYLOAD_BYTES {
                return Err(StoreError::BatchByteBound);
            }
            total
                .checked_add(record.encoded_len())
                .ok_or(StoreError::BatchByteBound)
        })?;
        if let Some(value) = &self.receipt {
            value.validate()?;
            encoded_bytes = encoded_bytes
                .checked_add(value.idempotency_key.as_str().len())
                .and_then(|total| total.checked_add(value.method.len()))
                .and_then(|total| total.checked_add(value.response.len()))
                .and_then(|total| total.checked_add(64))
                .ok_or(StoreError::BatchByteBound)?;
        }
        if encoded_bytes > MAX_APPEND_BATCH_BYTES {
            return Err(StoreError::BatchByteBound);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceInfo {
    pub resource: ResourceId,
    pub position: Position,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryPage {
    pub resources: Vec<ResourceInfo>,
    pub next_after: Option<ResourceId>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PrefixSnapshot {
    pub position: Position,
    pub records: Vec<StoredRecord>,
    pub receipts: Vec<MutationReceipt>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppendOutcome {
    Committed(MutationReceipt),
    Replayed(MutationReceipt),
    CommittedWithoutReceipt(Position),
}

#[derive(Clone)]
pub struct AppendGuard(Option<Arc<dyn Fn() -> bool + Send + Sync>>);

impl AppendGuard {
    #[must_use]
    pub const fn allow() -> Self {
        Self(None)
    }

    #[must_use]
    pub fn cancelled_when(check: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(check)))
    }

    pub fn check(&self) -> Result<(), StoreError> {
        if self.0.as_ref().is_some_and(|check| check()) {
            Err(StoreError::AppendCancelled)
        } else {
            Ok(())
        }
    }
}

impl Default for AppendGuard {
    fn default() -> Self {
        Self::allow()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailPoint {
    BeforeCommit,
    AfterCommitBeforeResponse,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreError {
    InvalidIdentifier(&'static str),
    InvalidLimit,
    ResourceExists,
    ResourceNotFound,
    FenceHeld,
    StaleFence,
    FenceExpired,
    PositionConflict {
        expected: Position,
        actual: Position,
    },
    PositionOverflow,
    BatchRecordBound,
    BatchByteBound,
    ReceiptTooLarge,
    IdempotencyConflict,
    AppendCancelled,
    Corrupt(&'static str),
    FailureInjected(FailPoint),
    Storage,
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentifier(label) => write!(formatter, "invalid {label} identifier"),
            Self::InvalidLimit => formatter.write_str("requested limit exceeds the fixed bound"),
            Self::ResourceExists => formatter.write_str("ledger resource already exists"),
            Self::ResourceNotFound => formatter.write_str("ledger resource does not exist"),
            Self::FenceHeld => formatter.write_str("ledger resource fence is held"),
            Self::StaleFence => formatter.write_str("ledger resource fence is stale"),
            Self::FenceExpired => formatter.write_str("ledger resource fence expired"),
            Self::PositionConflict { expected, actual } => write!(
                formatter,
                "ledger position conflict: expected {}, actual {}",
                expected.get(),
                actual.get()
            ),
            Self::PositionOverflow => formatter.write_str("ledger position exceeds SQLite range"),
            Self::BatchRecordBound => formatter.write_str("append batch record bound exceeded"),
            Self::BatchByteBound => formatter.write_str("append batch byte bound exceeded"),
            Self::ReceiptTooLarge => formatter.write_str("mutation receipt exceeds fixed bound"),
            Self::IdempotencyConflict => {
                formatter.write_str("idempotency key reused with a different mutation")
            }
            Self::AppendCancelled => {
                formatter.write_str("append was cancelled before its first durable write")
            }
            Self::Corrupt(reason) => write!(formatter, "ledger data is corrupt: {reason}"),
            Self::FailureInjected(point) => write!(formatter, "ledger failpoint: {point:?}"),
            Self::Storage => formatter.write_str("ledger storage operation failed"),
        }
    }
}

impl Error for StoreError {}

pub trait LedgerClock: Send + Sync {
    fn now_ms(&self) -> u64;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemLedgerClock;

impl LedgerClock for SystemLedgerClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock must not precede Unix epoch")
            .as_millis()
            .try_into()
            .expect("system clock milliseconds must fit in u64")
    }
}

#[async_trait]
pub trait LedgerStore: Send + Sync + 'static {
    async fn discover(
        &self,
        after: Option<&ResourceId>,
        limit: usize,
    ) -> Result<DiscoveryPage, StoreError>;
    async fn create(&self, resource: &ResourceId) -> Result<ResourceInfo, StoreError>;
    async fn create_fenced(
        &self,
        resource: &ResourceId,
        owner: &OwnerId,
        ttl_ms: u64,
    ) -> Result<(ResourceInfo, Fence), StoreError>;
    async fn open(&self, resource: &ResourceId) -> Result<ResourceInfo, StoreError>;
    async fn acquire_fence(
        &self,
        resource: &ResourceId,
        owner: &OwnerId,
        ttl_ms: u64,
    ) -> Result<Fence, StoreError>;
    async fn renew_fence(&self, fence: &Fence, ttl_ms: u64) -> Result<Fence, StoreError>;
    async fn check_fence(&self, fence: &Fence) -> Result<(), StoreError>;
    async fn read_prefix(
        &self,
        resource: &ResourceId,
        through: Option<Position>,
    ) -> Result<PrefixSnapshot, StoreError>;
    async fn read_range(
        &self,
        resource: &ResourceId,
        after: Position,
        limit: usize,
    ) -> Result<Vec<StoredRecord>, StoreError>;
    async fn compare_and_append(
        &self,
        resource: &ResourceId,
        fence: &Fence,
        expected: Position,
        batch: AppendBatch,
    ) -> Result<AppendOutcome, StoreError>;
    async fn compare_and_append_guarded(
        &self,
        resource: &ResourceId,
        fence: &Fence,
        expected: Position,
        batch: AppendBatch,
        guard: AppendGuard,
    ) -> Result<AppendOutcome, StoreError>;
    async fn lookup_receipt(
        &self,
        resource: &ResourceId,
        key: &IdempotencyId,
    ) -> Result<Option<MutationReceipt>, StoreError>;
    async fn wait_for_advance(
        &self,
        resource: &ResourceId,
        after: Position,
        deadline_ms: u64,
    ) -> Result<Position, StoreError>;
    async fn remove(
        &self,
        resource: &ResourceId,
        fence: &Fence,
        expected: Position,
    ) -> Result<(), StoreError>;
}
