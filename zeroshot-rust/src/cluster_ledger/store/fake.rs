use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::Notify;

use super::{
    AppendBatch, AppendGuard, AppendOutcome, DiscoveryPage, FailPoint, Fence, IdempotencyId,
    LedgerClock, LedgerStore, MutationReceipt, OwnerId, Position, PrefixSnapshot, ResourceId,
    ResourceInfo, StoreError, MAX_DISCOVERY_PAGE,
};
use crate::cluster_ledger::record::{StoredRecord, MAX_RANGE_RECORDS};

#[derive(Clone, Debug)]
pub struct ManualLedgerClock {
    now_ms: Arc<AtomicU64>,
}

impl ManualLedgerClock {
    #[must_use]
    pub fn new(now_ms: u64) -> Self {
        Self {
            now_ms: Arc::new(AtomicU64::new(now_ms)),
        }
    }

    pub fn set(&self, now_ms: u64) {
        self.now_ms.store(now_ms, Ordering::Release);
    }

    pub fn advance(&self, delta_ms: u64) -> Result<u64, StoreError> {
        self.now_ms
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(delta_ms)
            })
            .map(|previous| previous + delta_ms)
            .map_err(|_| StoreError::PositionOverflow)
    }
}

impl LedgerClock for ManualLedgerClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::Acquire)
    }
}

#[derive(Default)]
struct ResourceData {
    records: Vec<StoredRecord>,
    receipts: BTreeMap<IdempotencyId, MutationReceipt>,
    fence: Option<Fence>,
    last_fence_epoch: u64,
    notify: Arc<Notify>,
}

#[derive(Default)]
struct FakeState {
    resources: BTreeMap<ResourceId, ResourceData>,
    next_failpoint: Option<FailPoint>,
}

#[derive(Clone)]
pub struct FakeLedgerStore {
    state: Arc<Mutex<FakeState>>,
    clock: Arc<dyn LedgerClock>,
}

impl FakeLedgerStore {
    #[must_use]
    pub fn new(clock: impl LedgerClock + 'static) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeState::default())),
            clock: Arc::new(clock),
        }
    }

    #[must_use]
    pub fn with_shared_clock(clock: Arc<dyn LedgerClock>) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeState::default())),
            clock,
        }
    }

    #[must_use]
    pub fn restart(&self) -> Self {
        self.clone()
    }

    pub fn fail_next(&self, point: FailPoint) {
        self.state
            .lock()
            .expect("fake ledger mutex must not be poisoned")
            .next_failpoint = Some(point);
    }

    fn take_failpoint(state: &mut FakeState, point: FailPoint) -> bool {
        if state.next_failpoint == Some(point) {
            state.next_failpoint = None;
            true
        } else {
            false
        }
    }

    fn validate_fence(
        clock: &dyn LedgerClock,
        data: &ResourceData,
        fence: &Fence,
    ) -> Result<(), StoreError> {
        let current = data.fence.as_ref().ok_or(StoreError::StaleFence)?;
        if current != fence {
            return Err(StoreError::StaleFence);
        }
        if current.expires_at_ms <= clock.now_ms() {
            return Err(StoreError::FenceExpired);
        }
        Ok(())
    }

    fn fence(
        &self,
        resource: &ResourceId,
        owner: &OwnerId,
        ttl_ms: u64,
        epoch: u64,
    ) -> Result<Fence, StoreError> {
        let expires_at_ms = self
            .clock
            .now_ms()
            .checked_add(ttl_ms)
            .filter(|expires| ttl_ms != 0 && *expires <= i64::MAX as u64)
            .ok_or(if ttl_ms == 0 {
                StoreError::FenceExpired
            } else {
                StoreError::PositionOverflow
            })?;
        Ok(Fence {
            resource: resource.clone(),
            owner: owner.clone(),
            epoch,
            expires_at_ms,
        })
    }

    fn append(
        &self,
        resource: &ResourceId,
        fence: &Fence,
        expected: Position,
        batch: AppendBatch,
        guard: &AppendGuard,
    ) -> Result<AppendOutcome, StoreError> {
        batch.validate()?;
        let mut state = self
            .state
            .lock()
            .expect("fake ledger mutex must not be poisoned");
        if Self::take_failpoint(&mut state, FailPoint::BeforeCommit) {
            return Err(StoreError::FailureInjected(FailPoint::BeforeCommit));
        }
        let data = state
            .resources
            .get_mut(resource)
            .ok_or(StoreError::ResourceNotFound)?;
        Self::validate_fence(self.clock.as_ref(), data, fence)?;
        if let Some(outcome) = existing_outcome(data, &batch)? {
            return Ok(outcome);
        }
        validate_append(data, resource, expected, &batch)?;
        guard.check()?;
        let outcome = apply_append(data, expected, batch)?;
        if Self::take_failpoint(&mut state, FailPoint::AfterCommitBeforeResponse) {
            return Err(StoreError::FailureInjected(
                FailPoint::AfterCommitBeforeResponse,
            ));
        }
        Ok(outcome)
    }
}

fn existing_outcome(
    data: &ResourceData,
    batch: &AppendBatch,
) -> Result<Option<AppendOutcome>, StoreError> {
    let Some(receipt) = &batch.receipt else {
        return Ok(None);
    };
    let Some(existing) = data.receipts.get(&receipt.idempotency_key) else {
        return Ok(None);
    };
    if existing.method == receipt.method && existing.fingerprint == receipt.fingerprint {
        Ok(Some(AppendOutcome::Replayed(existing.clone())))
    } else {
        Err(StoreError::IdempotencyConflict)
    }
}

fn validate_append(
    data: &ResourceData,
    resource: &ResourceId,
    expected: Position,
    batch: &AppendBatch,
) -> Result<(), StoreError> {
    let actual = Position::new(
        u64::try_from(data.records.len()).map_err(|_| StoreError::PositionOverflow)?,
    )?;
    if actual != expected {
        return Err(StoreError::PositionConflict { expected, actual });
    }
    let committed_position = expected.checked_add(batch.records.len())?;
    for (offset, record) in batch.records.iter().enumerate() {
        let expected_sequence = expected.checked_add(offset + 1)?;
        if record.resource != *resource || record.sequence != expected_sequence {
            return Err(StoreError::Corrupt("append record identity"));
        }
    }
    if batch
        .receipt
        .as_ref()
        .is_some_and(|receipt| receipt.committed_position != committed_position)
    {
        return Err(StoreError::Corrupt("receipt position"));
    }
    Ok(())
}

fn apply_append(
    data: &mut ResourceData,
    expected: Position,
    batch: AppendBatch,
) -> Result<AppendOutcome, StoreError> {
    let committed_position = expected.checked_add(batch.records.len())?;
    data.records.extend(batch.records);
    let outcome = if let Some(receipt) = batch.receipt {
        data.receipts
            .insert(receipt.idempotency_key.clone(), receipt.clone());
        AppendOutcome::Committed(receipt)
    } else {
        AppendOutcome::CommittedWithoutReceipt(committed_position)
    };
    data.notify.notify_waiters();
    Ok(outcome)
}

#[async_trait]
impl LedgerStore for FakeLedgerStore {
    async fn discover(
        &self,
        after: Option<&ResourceId>,
        limit: usize,
    ) -> Result<DiscoveryPage, StoreError> {
        if limit == 0 || limit > MAX_DISCOVERY_PAGE {
            return Err(StoreError::InvalidLimit);
        }
        let state = self
            .state
            .lock()
            .expect("fake ledger mutex must not be poisoned");
        let mut resources = state
            .resources
            .iter()
            .filter(|(resource, _)| after.is_none_or(|after| *resource > after))
            .take(limit + 1)
            .map(|(resource, data)| {
                Ok(ResourceInfo {
                    resource: resource.clone(),
                    position: Position::new(
                        u64::try_from(data.records.len())
                            .map_err(|_| StoreError::PositionOverflow)?,
                    )?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        let next_after = if resources.len() > limit {
            resources.truncate(limit);
            resources.last().map(|info| info.resource.clone())
        } else {
            None
        };
        Ok(DiscoveryPage {
            resources,
            next_after,
        })
    }

    async fn create(&self, resource: &ResourceId) -> Result<ResourceInfo, StoreError> {
        let mut state = self
            .state
            .lock()
            .expect("fake ledger mutex must not be poisoned");
        if state.resources.contains_key(resource) {
            return Err(StoreError::ResourceExists);
        }
        state
            .resources
            .insert(resource.clone(), ResourceData::default());
        Ok(ResourceInfo {
            resource: resource.clone(),
            position: Position::ZERO,
        })
    }

    async fn create_fenced(
        &self,
        resource: &ResourceId,
        owner: &OwnerId,
        ttl_ms: u64,
    ) -> Result<(ResourceInfo, Fence), StoreError> {
        let fence = self.fence(resource, owner, ttl_ms, 1)?;
        let mut state = self
            .state
            .lock()
            .expect("fake ledger mutex must not be poisoned");
        if state.resources.contains_key(resource) {
            return Err(StoreError::ResourceExists);
        }
        let data = ResourceData {
            fence: Some(fence.clone()),
            last_fence_epoch: 1,
            ..ResourceData::default()
        };
        state.resources.insert(resource.clone(), data);
        Ok((
            ResourceInfo {
                resource: resource.clone(),
                position: Position::ZERO,
            },
            fence,
        ))
    }

    async fn open(&self, resource: &ResourceId) -> Result<ResourceInfo, StoreError> {
        let state = self
            .state
            .lock()
            .expect("fake ledger mutex must not be poisoned");
        let data = state
            .resources
            .get(resource)
            .ok_or(StoreError::ResourceNotFound)?;
        Ok(ResourceInfo {
            resource: resource.clone(),
            position: Position::new(
                u64::try_from(data.records.len()).map_err(|_| StoreError::PositionOverflow)?,
            )?,
        })
    }

    async fn acquire_fence(
        &self,
        resource: &ResourceId,
        owner: &OwnerId,
        ttl_ms: u64,
    ) -> Result<Fence, StoreError> {
        if ttl_ms == 0 {
            return Err(StoreError::FenceExpired);
        }
        let now = self.clock.now_ms();
        let expires_at_ms = now
            .checked_add(ttl_ms)
            .ok_or(StoreError::PositionOverflow)?;
        if expires_at_ms > i64::MAX as u64 {
            return Err(StoreError::PositionOverflow);
        }
        let mut state = self
            .state
            .lock()
            .expect("fake ledger mutex must not be poisoned");
        let data = state
            .resources
            .get_mut(resource)
            .ok_or(StoreError::ResourceNotFound)?;
        if data
            .fence
            .as_ref()
            .is_some_and(|fence| fence.expires_at_ms > now)
        {
            return Err(StoreError::FenceHeld);
        }
        data.last_fence_epoch = data
            .last_fence_epoch
            .checked_add(1)
            .ok_or(StoreError::PositionOverflow)?;
        let fence = Fence {
            resource: resource.clone(),
            owner: owner.clone(),
            epoch: data.last_fence_epoch,
            expires_at_ms,
        };
        data.fence = Some(fence.clone());
        Ok(fence)
    }

    async fn renew_fence(&self, fence: &Fence, ttl_ms: u64) -> Result<Fence, StoreError> {
        if ttl_ms == 0 {
            return Err(StoreError::FenceExpired);
        }
        let now = self.clock.now_ms();
        let expires_at_ms = now
            .checked_add(ttl_ms)
            .ok_or(StoreError::PositionOverflow)?;
        if expires_at_ms > i64::MAX as u64 {
            return Err(StoreError::PositionOverflow);
        }
        let mut state = self
            .state
            .lock()
            .expect("fake ledger mutex must not be poisoned");
        let data = state
            .resources
            .get_mut(&fence.resource)
            .ok_or(StoreError::ResourceNotFound)?;
        Self::validate_fence(self.clock.as_ref(), data, fence)?;
        let renewed = Fence {
            expires_at_ms,
            ..fence.clone()
        };
        data.fence = Some(renewed.clone());
        Ok(renewed)
    }

    async fn check_fence(&self, fence: &Fence) -> Result<(), StoreError> {
        let state = self
            .state
            .lock()
            .expect("fake ledger mutex must not be poisoned");
        let data = state
            .resources
            .get(&fence.resource)
            .ok_or(StoreError::ResourceNotFound)?;
        Self::validate_fence(self.clock.as_ref(), data, fence)
    }

    async fn read_prefix(
        &self,
        resource: &ResourceId,
        through: Option<Position>,
    ) -> Result<PrefixSnapshot, StoreError> {
        let state = self
            .state
            .lock()
            .expect("fake ledger mutex must not be poisoned");
        let data = state
            .resources
            .get(resource)
            .ok_or(StoreError::ResourceNotFound)?;
        let full_position = Position::new(
            u64::try_from(data.records.len()).map_err(|_| StoreError::PositionOverflow)?,
        )?;
        let position = through.unwrap_or(full_position);
        if position > full_position {
            return Err(StoreError::InvalidLimit);
        }
        let length = usize::try_from(position.get()).map_err(|_| StoreError::InvalidLimit)?;
        let records = data.records[..length].to_vec();
        let receipts = data
            .receipts
            .values()
            .filter(|receipt| receipt.committed_position <= position)
            .cloned()
            .collect();
        Ok(PrefixSnapshot {
            position,
            records,
            receipts,
        })
    }

    async fn read_range(
        &self,
        resource: &ResourceId,
        after: Position,
        limit: usize,
    ) -> Result<Vec<StoredRecord>, StoreError> {
        if limit == 0 || limit > MAX_RANGE_RECORDS {
            return Err(StoreError::InvalidLimit);
        }
        let state = self
            .state
            .lock()
            .expect("fake ledger mutex must not be poisoned");
        let data = state
            .resources
            .get(resource)
            .ok_or(StoreError::ResourceNotFound)?;
        let start = usize::try_from(after.get()).map_err(|_| StoreError::InvalidLimit)?;
        if start > data.records.len() {
            return Err(StoreError::PositionConflict {
                expected: after,
                actual: Position::new(
                    u64::try_from(data.records.len()).map_err(|_| StoreError::PositionOverflow)?,
                )?,
            });
        }
        Ok(data.records[start..].iter().take(limit).cloned().collect())
    }

    async fn compare_and_append(
        &self,
        resource: &ResourceId,
        fence: &Fence,
        expected: Position,
        batch: AppendBatch,
    ) -> Result<AppendOutcome, StoreError> {
        self.append(resource, fence, expected, batch, &AppendGuard::allow())
    }

    async fn compare_and_append_guarded(
        &self,
        resource: &ResourceId,
        fence: &Fence,
        expected: Position,
        batch: AppendBatch,
        guard: AppendGuard,
    ) -> Result<AppendOutcome, StoreError> {
        self.append(resource, fence, expected, batch, &guard)
    }

    async fn lookup_receipt(
        &self,
        resource: &ResourceId,
        key: &IdempotencyId,
    ) -> Result<Option<MutationReceipt>, StoreError> {
        let state = self
            .state
            .lock()
            .expect("fake ledger mutex must not be poisoned");
        let data = state
            .resources
            .get(resource)
            .ok_or(StoreError::ResourceNotFound)?;
        Ok(data.receipts.get(key).cloned())
    }

    async fn wait_for_advance(
        &self,
        resource: &ResourceId,
        after: Position,
        deadline_ms: u64,
    ) -> Result<Position, StoreError> {
        loop {
            let (position, notified) = {
                let state = self
                    .state
                    .lock()
                    .expect("fake ledger mutex must not be poisoned");
                let data = state
                    .resources
                    .get(resource)
                    .ok_or(StoreError::ResourceNotFound)?;
                let notified = data.notify.clone().notified_owned();
                let position = Position::new(
                    u64::try_from(data.records.len()).map_err(|_| StoreError::PositionOverflow)?,
                )?;
                (position, notified)
            };
            if position > after || self.clock.now_ms() >= deadline_ms {
                return Ok(position);
            }
            let reread = self.open(resource).await?.position;
            if reread > after {
                return Ok(reread);
            }
            tokio::select! {
                () = notified => {}
                () = tokio::time::sleep(std::time::Duration::from_millis(1)) => {}
            }
        }
    }

    async fn remove(
        &self,
        resource: &ResourceId,
        fence: &Fence,
        expected: Position,
    ) -> Result<(), StoreError> {
        let mut state = self
            .state
            .lock()
            .expect("fake ledger mutex must not be poisoned");
        if Self::take_failpoint(&mut state, FailPoint::BeforeCommit) {
            return Err(StoreError::FailureInjected(FailPoint::BeforeCommit));
        }
        let data = state
            .resources
            .get(resource)
            .ok_or(StoreError::ResourceNotFound)?;
        Self::validate_fence(self.clock.as_ref(), data, fence)?;
        let actual = Position::new(
            u64::try_from(data.records.len()).map_err(|_| StoreError::PositionOverflow)?,
        )?;
        if actual != expected {
            return Err(StoreError::PositionConflict { expected, actual });
        }
        state.resources.remove(resource);
        if Self::take_failpoint(&mut state, FailPoint::AfterCommitBeforeResponse) {
            return Err(StoreError::FailureInjected(
                FailPoint::AfterCommitBeforeResponse,
            ));
        }
        Ok(())
    }
}

impl Default for FakeLedgerStore {
    fn default() -> Self {
        Self::new(ManualLedgerClock::new(0))
    }
}
