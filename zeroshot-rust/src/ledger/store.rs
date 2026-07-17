use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use super::identity::{IdempotencyId, OwnerFence, OwnerId, Position, ResourceId};
use super::record::{
    LedgerRecord, MAX_APPEND_BYTES, MAX_APPEND_RECORDS, MAX_DISCOVERY_RESOURCES, MAX_RANGE_RECORDS,
};
use super::validation::valid_component;

pub const MAX_RECEIPT_BYTES: usize = 1024 * 1024;

pub trait Clock: Send + Sync {
    fn now_unix_millis(&self) -> u64;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_millis(&self) -> u64 {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        u64::try_from(millis).unwrap_or(u64::MAX)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceMetadata {
    pub resource_id: ResourceId,
    pub position: Position,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourcePage {
    pub resources: Vec<ResourceMetadata>,
    pub next_after: Option<ResourceId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoherentPrefix {
    pub end: Position,
    pub records: Vec<LedgerRecord>,
    pub receipts: BTreeMap<IdempotencyId, OpaqueMutationReceipt>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpaqueMutationReceipt {
    pub key: IdempotencyId,
    pub method: String,
    pub fingerprint: [u8; 32],
    pub value: Vec<u8>,
    pub at_position: Position,
}

impl OpaqueMutationReceipt {
    pub fn validate(&self) -> Result<(), StoreError> {
        if self.at_position == Position::ZERO {
            return Err(StoreError::Corrupt);
        }
        if !valid_component(&self.method) {
            return Err(StoreError::BoundExceeded(
                "receipt method must be non-control UTF-8 at most 256 bytes",
            ));
        }
        if self.value.len() > MAX_RECEIPT_BYTES {
            return Err(StoreError::BoundExceeded("receipt exceeds 1 MiB"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct AppendRequest {
    pub expected_position: Position,
    pub fence: OwnerFence,
    pub records: Vec<LedgerRecord>,
    pub receipt: Option<OpaqueMutationReceipt>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppendOutcome {
    pub position: Position,
    pub receipt: Option<OpaqueMutationReceipt>,
    pub replayed: bool,
}

#[derive(Clone, Debug, thiserror::Error, Eq, PartialEq)]
pub enum StoreError {
    #[error("ledger resource already exists")]
    AlreadyExists,
    #[error("ledger resource does not exist")]
    NotFound,
    #[error("ledger resource position conflict; current position is {current:?}")]
    PositionConflict { current: Position },
    #[error("owner fence is absent, stale, mismatched, or expired")]
    FenceRejected,
    #[error("idempotency key was reused with a different method or fingerprint")]
    ReceiptConflict,
    #[error("ledger bound exceeded: {0}")]
    BoundExceeded(&'static str),
    #[error("ledger data failed closed integrity validation")]
    Corrupt,
    #[error("ledger storage operation failed")]
    StorageUnavailable,
}

#[async_trait]
pub trait LedgerStore: Send + Sync + 'static {
    async fn list_resources(
        &self,
        after: Option<&ResourceId>,
        limit: usize,
    ) -> Result<ResourcePage, StoreError>;
    async fn create_resource(
        &self,
        resource_id: &ResourceId,
    ) -> Result<ResourceMetadata, StoreError>;
    async fn open_resource(&self, resource_id: &ResourceId)
    -> Result<ResourceMetadata, StoreError>;
    async fn acquire_fence(
        &self,
        resource_id: &ResourceId,
        owner: &OwnerId,
        ttl_millis: u64,
    ) -> Result<OwnerFence, StoreError>;
    async fn renew_fence(
        &self,
        resource_id: &ResourceId,
        fence: &OwnerFence,
        ttl_millis: u64,
    ) -> Result<OwnerFence, StoreError>;
    async fn validate_fence(
        &self,
        resource_id: &ResourceId,
        fence: &OwnerFence,
    ) -> Result<(), StoreError>;
    async fn read_prefix(&self, resource_id: &ResourceId) -> Result<CoherentPrefix, StoreError>;
    async fn read_range(
        &self,
        resource_id: &ResourceId,
        after: Position,
        limit: usize,
    ) -> Result<CoherentPrefix, StoreError>;
    async fn read_receipt(
        &self,
        resource_id: &ResourceId,
        key: &IdempotencyId,
    ) -> Result<Option<OpaqueMutationReceipt>, StoreError>;
    async fn compare_and_append(
        &self,
        resource_id: &ResourceId,
        request: AppendRequest,
    ) -> Result<AppendOutcome, StoreError>;
    async fn wait_for_advancement(
        &self,
        resource_id: &ResourceId,
        after: Position,
    ) -> Result<Position, StoreError>;
    async fn remove_resource(
        &self,
        resource_id: &ResourceId,
        fence: &OwnerFence,
        expected_position: Position,
    ) -> Result<(), StoreError>;
}

#[derive(Default)]
struct MemoryResource {
    records: Vec<LedgerRecord>,
    receipts: BTreeMap<IdempotencyId, OpaqueMutationReceipt>,
    receipt_positions: BTreeMap<Position, IdempotencyId>,
    fence: Option<OwnerFence>,
    next_fence_epoch: u64,
    notify: Arc<Notify>,
}

#[derive(Clone)]
pub struct MemoryLedgerStore {
    clock: Arc<dyn Clock>,
    resources: Arc<Mutex<BTreeMap<ResourceId, MemoryResource>>>,
}

impl MemoryLedgerStore {
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            resources: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<ResourceId, MemoryResource>>, StoreError> {
        self.resources
            .lock()
            .map_err(|_| StoreError::StorageUnavailable)
    }
}

#[async_trait]
impl LedgerStore for MemoryLedgerStore {
    async fn list_resources(
        &self,
        after: Option<&ResourceId>,
        limit: usize,
    ) -> Result<ResourcePage, StoreError> {
        validate_discovery_limit(limit)?;
        let resources = self.lock()?;
        let matches = resources
            .iter()
            .filter(|(id, _)| after.is_none_or(|after| *id > after))
            .map(|(id, resource)| {
                Ok(ResourceMetadata {
                    resource_id: id.clone(),
                    position: position_for_len(resource.records.len())?,
                })
            })
            .take(limit.saturating_add(1))
            .collect::<Result<Vec<_>, _>>()?;
        let mut matches = matches;
        let next_after = if matches.len() > limit {
            matches.truncate(limit);
            matches.last().map(|resource| resource.resource_id.clone())
        } else {
            None
        };
        Ok(ResourcePage {
            resources: matches,
            next_after,
        })
    }

    async fn create_resource(
        &self,
        resource_id: &ResourceId,
    ) -> Result<ResourceMetadata, StoreError> {
        let mut resources = self.lock()?;
        if resources.contains_key(resource_id) {
            return Err(StoreError::AlreadyExists);
        }
        resources.insert(resource_id.clone(), MemoryResource::default());
        Ok(ResourceMetadata {
            resource_id: resource_id.clone(),
            position: Position::ZERO,
        })
    }

    async fn open_resource(
        &self,
        resource_id: &ResourceId,
    ) -> Result<ResourceMetadata, StoreError> {
        let resources = self.lock()?;
        let resource = resources.get(resource_id).ok_or(StoreError::NotFound)?;
        Ok(ResourceMetadata {
            resource_id: resource_id.clone(),
            position: position_for_len(resource.records.len())?,
        })
    }

    async fn acquire_fence(
        &self,
        resource_id: &ResourceId,
        owner: &OwnerId,
        ttl_millis: u64,
    ) -> Result<OwnerFence, StoreError> {
        let mut resources = self.lock()?;
        let resource = resources.get_mut(resource_id).ok_or(StoreError::NotFound)?;
        let now = self.clock.now_unix_millis();
        let expires_at = expiration(now, ttl_millis)?;
        if resource
            .fence
            .as_ref()
            .is_some_and(|fence| !fence.is_expired_at(now) && fence.owner != *owner)
        {
            return Err(StoreError::FenceRejected);
        }
        resource.next_fence_epoch = resource
            .next_fence_epoch
            .checked_add(1)
            .ok_or(StoreError::BoundExceeded("fence epoch overflow"))?;
        let fence = OwnerFence {
            owner: owner.clone(),
            epoch: resource.next_fence_epoch,
            expires_at_unix_millis: expires_at,
        };
        resource.fence = Some(fence.clone());
        Ok(fence)
    }

    async fn renew_fence(
        &self,
        resource_id: &ResourceId,
        fence: &OwnerFence,
        ttl_millis: u64,
    ) -> Result<OwnerFence, StoreError> {
        let mut resources = self.lock()?;
        let resource = resources.get_mut(resource_id).ok_or(StoreError::NotFound)?;
        let now = self.clock.now_unix_millis();
        let expires_at = expiration(now, ttl_millis)?;
        validate_memory_fence(resource, fence, now)?;
        let renewed = OwnerFence {
            expires_at_unix_millis: expires_at,
            ..fence.clone()
        };
        resource.fence = Some(renewed.clone());
        Ok(renewed)
    }

    async fn validate_fence(
        &self,
        resource_id: &ResourceId,
        fence: &OwnerFence,
    ) -> Result<(), StoreError> {
        let resources = self.lock()?;
        let resource = resources.get(resource_id).ok_or(StoreError::NotFound)?;
        validate_memory_fence(resource, fence, self.clock.now_unix_millis())
    }

    async fn read_prefix(&self, resource_id: &ResourceId) -> Result<CoherentPrefix, StoreError> {
        let resources = self.lock()?;
        let resource = resources.get(resource_id).ok_or(StoreError::NotFound)?;
        let records = resource.records.clone();
        validate_prefix(resource_id, &records)?;
        validate_memory_receipt_set(resource)?;
        Ok(CoherentPrefix {
            end: position_for_len(records.len())?,
            records,
            receipts: resource.receipts.clone(),
        })
    }

    async fn read_range(
        &self,
        resource_id: &ResourceId,
        after: Position,
        limit: usize,
    ) -> Result<CoherentPrefix, StoreError> {
        validate_range_limit(limit)?;
        let resources = self.lock()?;
        let resource = resources.get(resource_id).ok_or(StoreError::NotFound)?;
        let start = usize::try_from(after.get()).map_err(|_| StoreError::Corrupt)?;
        if start > resource.records.len() {
            return Err(StoreError::Corrupt);
        }
        let end = start.saturating_add(limit).min(resource.records.len());
        let records = resource.records[start..end].to_vec();
        let previous = start
            .checked_sub(1)
            .and_then(|index| resource.records.get(index));
        if let Some(previous) = previous {
            if previous.resource_id != *resource_id || previous.sequence != after {
                return Err(StoreError::Corrupt);
            }
            previous
                .validate_integrity()
                .map_err(|_| StoreError::Corrupt)?;
        }
        validate_append_chain(resource_id, after, previous, &records)?;
        let end_position = position_for_len(end)?;
        let mut receipts = BTreeMap::new();
        for (_, key) in resource.receipt_positions.range((
            std::ops::Bound::Excluded(after),
            std::ops::Bound::Included(end_position),
        )) {
            let receipt = resource.receipts.get(key).ok_or(StoreError::Corrupt)?;
            validate_memory_receipt(resource, receipt)?;
            if receipts.insert(key.clone(), receipt.clone()).is_some() {
                return Err(StoreError::Corrupt);
            }
        }
        Ok(CoherentPrefix {
            end: end_position,
            records,
            receipts,
        })
    }

    async fn read_receipt(
        &self,
        resource_id: &ResourceId,
        key: &IdempotencyId,
    ) -> Result<Option<OpaqueMutationReceipt>, StoreError> {
        let resources = self.lock()?;
        let resource = resources.get(resource_id).ok_or(StoreError::NotFound)?;
        resource
            .receipts
            .get(key)
            .map(|receipt| {
                validate_memory_receipt(resource, receipt)?;
                Ok(receipt.clone())
            })
            .transpose()
    }

    async fn compare_and_append(
        &self,
        resource_id: &ResourceId,
        request: AppendRequest,
    ) -> Result<AppendOutcome, StoreError> {
        validate_append_request(resource_id, &request)?;
        let notify;
        let outcome = {
            let mut resources = self.lock()?;
            let resource = resources.get_mut(resource_id).ok_or(StoreError::NotFound)?;
            validate_memory_fence(resource, &request.fence, self.clock.now_unix_millis())?;
            if let Some(receipt) = &request.receipt {
                if let Some(existing) = resource.receipts.get(&receipt.key) {
                    validate_memory_receipt(resource, existing)?;
                    if existing.method != receipt.method
                        || existing.fingerprint != receipt.fingerprint
                    {
                        return Err(StoreError::ReceiptConflict);
                    }
                    return Ok(AppendOutcome {
                        position: position_for_len(resource.records.len())?,
                        receipt: Some(existing.clone()),
                        replayed: true,
                    });
                }
            }
            let current = position_for_len(resource.records.len())?;
            if current != request.expected_position {
                return Err(StoreError::PositionConflict { current });
            }
            validate_append_chain(
                resource_id,
                current,
                resource.records.last(),
                &request.records,
            )?;
            if request.receipt.as_ref().is_some_and(|receipt| {
                resource
                    .receipt_positions
                    .contains_key(&receipt.at_position)
            }) {
                return Err(StoreError::Corrupt);
            }
            resource.records.extend(request.records);
            if let Some(receipt) = request.receipt.clone() {
                resource
                    .receipt_positions
                    .insert(receipt.at_position, receipt.key.clone());
                resource.receipts.insert(receipt.key.clone(), receipt);
            }
            notify = Arc::clone(&resource.notify);
            AppendOutcome {
                position: position_for_len(resource.records.len())?,
                receipt: request.receipt,
                replayed: false,
            }
        };
        notify.notify_waiters();
        Ok(outcome)
    }

    async fn wait_for_advancement(
        &self,
        resource_id: &ResourceId,
        after: Position,
    ) -> Result<Position, StoreError> {
        loop {
            let (current, notified) = {
                let resources = self.lock()?;
                let resource = resources.get(resource_id).ok_or(StoreError::NotFound)?;
                (
                    position_for_len(resource.records.len())?,
                    Arc::clone(&resource.notify).notified_owned(),
                )
            };
            if current > after {
                return Ok(current);
            }
            tokio::pin!(notified);
            notified.as_mut().enable();
            let reread = self.open_resource(resource_id).await?.position;
            if reread > after {
                return Ok(reread);
            }
            notified.await;
        }
    }

    async fn remove_resource(
        &self,
        resource_id: &ResourceId,
        fence: &OwnerFence,
        expected_position: Position,
    ) -> Result<(), StoreError> {
        let mut resources = self.lock()?;
        let resource = resources.get(resource_id).ok_or(StoreError::NotFound)?;
        validate_memory_fence(resource, fence, self.clock.now_unix_millis())?;
        let current = position_for_len(resource.records.len())?;
        if current != expected_position {
            return Err(StoreError::PositionConflict { current });
        }
        let removed = resources.remove(resource_id).ok_or(StoreError::NotFound)?;
        removed.notify.notify_waiters();
        Ok(())
    }
}

pub(crate) fn validate_discovery_limit(limit: usize) -> Result<(), StoreError> {
    if limit == 0 || limit > MAX_DISCOVERY_RESOURCES {
        Err(StoreError::BoundExceeded(
            "discovery page must contain 1..=1024 resources",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn validate_range_limit(limit: usize) -> Result<(), StoreError> {
    if limit == 0 || limit > MAX_RANGE_RECORDS {
        Err(StoreError::BoundExceeded(
            "range read must request 1..=4096 records",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn validate_append_request(
    resource_id: &ResourceId,
    request: &AppendRequest,
) -> Result<(), StoreError> {
    if request.records.is_empty() || request.records.len() > MAX_APPEND_RECORDS {
        return Err(StoreError::BoundExceeded(
            "append batch must contain 1..=1024 records",
        ));
    }
    let total = request.records.iter().try_fold(0usize, |total, record| {
        let encoded = serde_json::to_vec(record).map_err(|_| StoreError::Corrupt)?;
        total
            .checked_add(encoded.len())
            .ok_or(StoreError::BoundExceeded("append byte count overflow"))
    })?;
    if total > MAX_APPEND_BYTES {
        return Err(StoreError::BoundExceeded("append batch exceeds 8 MiB"));
    }
    if request
        .records
        .iter()
        .any(|record| &record.resource_id != resource_id)
    {
        return Err(StoreError::Corrupt);
    }
    if let Some(receipt) = &request.receipt {
        receipt.validate()?;
        if request.records.last().map(|record| record.sequence) != Some(receipt.at_position) {
            return Err(StoreError::Corrupt);
        }
    }
    Ok(())
}

pub(crate) fn validate_append_chain(
    resource_id: &ResourceId,
    current: Position,
    previous: Option<&LedgerRecord>,
    records: &[LedgerRecord],
) -> Result<(), StoreError> {
    let mut expected = current;
    let mut previous_hash = previous.map_or([0; 32], |record| record.record_hash);
    for record in records {
        expected = expected.checked_next().map_err(|_| StoreError::Corrupt)?;
        if &record.resource_id != resource_id
            || record.sequence != expected
            || record.previous_hash != previous_hash
        {
            return Err(StoreError::Corrupt);
        }
        record
            .validate_integrity()
            .map_err(|_| StoreError::Corrupt)?;
        previous_hash = record.record_hash;
    }
    Ok(())
}

pub(crate) fn validate_prefix(
    resource_id: &ResourceId,
    records: &[LedgerRecord],
) -> Result<(), StoreError> {
    validate_append_chain(resource_id, Position::ZERO, None, records)
}

fn position_for_len(len: usize) -> Result<Position, StoreError> {
    Position::new(u64::try_from(len).map_err(|_| StoreError::Corrupt)?)
        .map_err(|_| StoreError::Corrupt)
}

fn expiration(now: u64, ttl_millis: u64) -> Result<u64, StoreError> {
    if ttl_millis == 0 {
        return Err(StoreError::BoundExceeded("fence TTL must be positive"));
    }
    now.checked_add(ttl_millis)
        .ok_or(StoreError::BoundExceeded("fence expiry overflow"))
}

fn validate_memory_fence(
    resource: &MemoryResource,
    supplied: &OwnerFence,
    now: u64,
) -> Result<(), StoreError> {
    let current = resource.fence.as_ref().ok_or(StoreError::FenceRejected)?;
    if current.owner != supplied.owner
        || current.epoch != supplied.epoch
        || current.expires_at_unix_millis != supplied.expires_at_unix_millis
        || current.is_expired_at(now)
    {
        return Err(StoreError::FenceRejected);
    }
    Ok(())
}

fn validate_memory_receipt(
    resource: &MemoryResource,
    receipt: &OpaqueMutationReceipt,
) -> Result<(), StoreError> {
    let index = receipt
        .at_position
        .get()
        .checked_sub(1)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(StoreError::Corrupt)?;
    if resource.records.get(index).map(|record| record.sequence) != Some(receipt.at_position) {
        return Err(StoreError::Corrupt);
    }
    if resource.receipt_positions.get(&receipt.at_position) != Some(&receipt.key) {
        return Err(StoreError::Corrupt);
    }
    Ok(())
}

fn validate_memory_receipt_set(resource: &MemoryResource) -> Result<(), StoreError> {
    if resource.receipts.len() != resource.receipt_positions.len() {
        return Err(StoreError::Corrupt);
    }
    let mut positions = std::collections::BTreeSet::new();
    for receipt in resource.receipts.values() {
        receipt.validate()?;
        validate_memory_receipt(resource, receipt)?;
        if !positions.insert(receipt.at_position) {
            return Err(StoreError::Corrupt);
        }
    }
    Ok(())
}

#[allow(dead_code)]
type Notifications = HashMap<ResourceId, Arc<Notify>>;
