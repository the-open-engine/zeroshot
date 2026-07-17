use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use fs2::FileExt;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};
use tokio::sync::Notify;

use super::{
    AppendBatch, AppendGuard, AppendOutcome, DiscoveryPage, FailPoint, Fence, IdempotencyId,
    LedgerClock, LedgerStore, MutationReceipt, OwnerId, Position, PrefixSnapshot, ResourceId,
    ResourceInfo, StoreError, SystemLedgerClock, MAX_DISCOVERY_PAGE,
};
use crate::cluster_ledger::record::{StoredRecord, MAX_RANGE_RECORDS};

mod operations;
mod queries;
mod schema;

pub use operations::database_path;
pub use schema::{APPLICATION_ID, SCHEMA_VERSION};
use queries::{query_fence, query_receipt, query_receipts, query_records, to_sql_i64};

static NEXT_CREATE_FILE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct SqliteLedgerStore {
    root: Arc<PathBuf>,
    clock: Arc<dyn LedgerClock>,
    writer: Arc<Mutex<()>>,
    notifications: Arc<Mutex<BTreeMap<ResourceId, Arc<Notify>>>>,
    next_failpoint: Arc<Mutex<Option<FailPoint>>>,
}

impl SqliteLedgerStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        Self::with_clock(root, SystemLedgerClock)
    }

    pub fn with_clock(
        root: impl Into<PathBuf>,
        clock: impl LedgerClock + 'static,
    ) -> Result<Self, StoreError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|_| StoreError::Storage)?;
        Ok(Self {
            root: Arc::new(root),
            clock: Arc::new(clock),
            writer: Arc::new(Mutex::new(())),
            notifications: Arc::new(Mutex::new(BTreeMap::new())),
            next_failpoint: Arc::new(Mutex::new(None)),
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        self.root.as_ref()
    }

    #[must_use]
    pub fn path_for(&self, resource: &ResourceId) -> PathBuf {
        database_path(&self.root, resource)
    }

    pub fn fail_next(&self, point: FailPoint) {
        *self
            .next_failpoint
            .lock()
            .expect("SQLite failpoint mutex must not be poisoned") = Some(point);
    }

    fn take_failpoint(&self, point: FailPoint) -> bool {
        let mut value = self
            .next_failpoint
            .lock()
            .expect("SQLite failpoint mutex must not be poisoned");
        if *value == Some(point) {
            *value = None;
            true
        } else {
            false
        }
    }

    fn notify_for(&self, resource: &ResourceId) -> Arc<Notify> {
        self.notifications
            .lock()
            .expect("SQLite notification mutex must not be poisoned")
            .entry(resource.clone())
            .or_default()
            .clone()
    }

    fn connect_existing(&self, resource: &ResourceId) -> Result<Connection, StoreError> {
        let path = self.path_for(resource);
        if !path.is_file() {
            return Err(StoreError::ResourceNotFound);
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| StoreError::Storage)?;
        configure_connection(&connection)?;
        validate_schema(&connection, resource)?;
        Ok(connection)
    }

    fn position(transaction: &Transaction<'_>) -> Result<Position, StoreError> {
        let (count, maximum): (i64, Option<i64>) = transaction
            .query_row("SELECT count(*), max(sequence) FROM records", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|_| StoreError::Storage)?;
        let maximum = maximum.unwrap_or(0);
        if count != maximum {
            return Err(StoreError::Corrupt("record sequence gap"));
        }
        u64::try_from(maximum)
            .map_err(|_| StoreError::Corrupt("negative record position"))
            .and_then(Position::new)
    }

    fn check_fence_tx(
        &self,
        transaction: &Transaction<'_>,
        fence: &Fence,
    ) -> Result<(), StoreError> {
        let current = query_fence(transaction)?.ok_or(StoreError::StaleFence)?;
        if current != *fence {
            return Err(StoreError::StaleFence);
        }
        if current.expires_at_ms <= self.clock.now_ms() {
            return Err(StoreError::FenceExpired);
        }
        Ok(())
    }

    fn initial_fence(
        &self,
        resource: &ResourceId,
        owner: &OwnerId,
        ttl_ms: u64,
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
            epoch: 1,
            expires_at_ms,
        })
    }

    fn create_resource(
        &self,
        resource: &ResourceId,
        initial_fence: Option<&Fence>,
    ) -> Result<ResourceInfo, StoreError> {
        let _writer = self
            .writer
            .lock()
            .expect("SQLite writer mutex must not be poisoned");
        let path = self.path_for(resource);
        let _resource_lock = lock_resource_file(&path)?;
        create_or_reinitialize_database(&path, resource, initial_fence)?;
        Ok(ResourceInfo {
            resource: resource.clone(),
            position: Position::ZERO,
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
        if self.take_failpoint(FailPoint::BeforeCommit) {
            return Err(StoreError::FailureInjected(FailPoint::BeforeCommit));
        }
        let _writer = self
            .writer
            .lock()
            .expect("SQLite writer mutex must not be poisoned");
        let mut connection = self.connect_existing(resource)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| StoreError::Storage)?;
        self.check_fence_tx(&transaction, fence)?;
        if let Some(outcome) = operations::existing_receipt_outcome(&transaction, &batch)? {
            transaction.commit().map_err(|_| StoreError::Storage)?;
            return Ok(outcome);
        }
        let committed_position =
            operations::validate_append_transaction(&transaction, resource, expected, &batch)?;
        guard.check()?;
        operations::insert_records(&transaction, &batch.records)?;
        let outcome = operations::insert_receipt(&transaction, batch.receipt, committed_position)?;
        transaction.commit().map_err(|_| StoreError::Storage)?;
        self.notify_for(resource).notify_waiters();
        if self.take_failpoint(FailPoint::AfterCommitBeforeResponse) {
            return Err(StoreError::FailureInjected(
                FailPoint::AfterCommitBeforeResponse,
            ));
        }
        Ok(outcome)
    }

    pub fn settings(&self, resource: &ResourceId) -> Result<SqliteSettings, StoreError> {
        let connection = self.connect_existing(resource)?;
        let journal_mode: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .map_err(|_| StoreError::Storage)?;
        let synchronous: i64 = connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .map_err(|_| StoreError::Storage)?;
        let foreign_keys: i64 = connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .map_err(|_| StoreError::Storage)?;
        let busy_timeout: i64 = connection
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .map_err(|_| StoreError::Storage)?;
        let application_id: i64 = connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .map_err(|_| StoreError::Storage)?;
        let user_version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|_| StoreError::Storage)?;
        Ok(SqliteSettings {
            journal_mode,
            synchronous,
            foreign_keys,
            busy_timeout_ms: busy_timeout,
            application_id,
            schema_version: user_version,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqliteSettings {
    pub journal_mode: String,
    pub synchronous: i64,
    pub foreign_keys: i64,
    pub busy_timeout_ms: i64,
    pub application_id: i64,
    pub schema_version: i64,
}

#[async_trait]
impl LedgerStore for SqliteLedgerStore {
    async fn discover(
        &self,
        after: Option<&ResourceId>,
        limit: usize,
    ) -> Result<DiscoveryPage, StoreError> {
        if limit == 0 || limit > MAX_DISCOVERY_PAGE {
            return Err(StoreError::InvalidLimit);
        }
        let entries = fs::read_dir(self.root.as_ref()).map_err(|_| StoreError::Storage)?;
        let mut resources = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|_| StoreError::Storage)?;
            if let Some(info) = discover_database(self.root.as_ref(), &entry, after)? {
                resources.push(info);
            }
        }
        resources.sort_by(|left, right| left.resource.cmp(&right.resource));
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
        self.create_resource(resource, None)
    }

    async fn create_fenced(
        &self,
        resource: &ResourceId,
        owner: &OwnerId,
        ttl_ms: u64,
    ) -> Result<(ResourceInfo, Fence), StoreError> {
        let fence = self.initial_fence(resource, owner, ttl_ms)?;
        let info = self.create_resource(resource, Some(&fence))?;
        Ok((info, fence))
    }

    async fn open(&self, resource: &ResourceId) -> Result<ResourceInfo, StoreError> {
        let mut connection = self.connect_existing(resource)?;
        let transaction = connection.transaction().map_err(|_| StoreError::Storage)?;
        let position = Self::position(&transaction)?;
        transaction.commit().map_err(|_| StoreError::Storage)?;
        Ok(ResourceInfo {
            resource: resource.clone(),
            position,
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
        let _writer = self
            .writer
            .lock()
            .expect("SQLite writer mutex must not be poisoned");
        let now = self.clock.now_ms();
        let expires_at_ms = now
            .checked_add(ttl_ms)
            .ok_or(StoreError::PositionOverflow)?;
        if expires_at_ms > i64::MAX as u64 {
            return Err(StoreError::PositionOverflow);
        }
        let mut connection = self.connect_existing(resource)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| StoreError::Storage)?;
        let fence = acquire_fence_transaction(&transaction, resource, owner, now, expires_at_ms)?;
        transaction.commit().map_err(|_| StoreError::Storage)?;
        Ok(fence)
    }

    async fn renew_fence(&self, fence: &Fence, ttl_ms: u64) -> Result<Fence, StoreError> {
        if ttl_ms == 0 {
            return Err(StoreError::FenceExpired);
        }
        let _writer = self
            .writer
            .lock()
            .expect("SQLite writer mutex must not be poisoned");
        let expires_at_ms = self
            .clock
            .now_ms()
            .checked_add(ttl_ms)
            .ok_or(StoreError::PositionOverflow)?;
        if expires_at_ms > i64::MAX as u64 {
            return Err(StoreError::PositionOverflow);
        }
        let mut connection = self.connect_existing(&fence.resource)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| StoreError::Storage)?;
        self.check_fence_tx(&transaction, fence)?;
        transaction
            .execute(
                "UPDATE fence SET expires_at_ms = ?1 WHERE singleton = 1",
                [to_sql_i64(expires_at_ms)?],
            )
            .map_err(|_| StoreError::Storage)?;
        transaction.commit().map_err(|_| StoreError::Storage)?;
        Ok(Fence {
            expires_at_ms,
            ..fence.clone()
        })
    }

    async fn check_fence(&self, fence: &Fence) -> Result<(), StoreError> {
        let mut connection = self.connect_existing(&fence.resource)?;
        let transaction = connection.transaction().map_err(|_| StoreError::Storage)?;
        self.check_fence_tx(&transaction, fence)?;
        transaction.commit().map_err(|_| StoreError::Storage)
    }

    async fn read_prefix(
        &self,
        resource: &ResourceId,
        through: Option<Position>,
    ) -> Result<PrefixSnapshot, StoreError> {
        let mut connection = self.connect_existing(resource)?;
        let transaction = connection.transaction().map_err(|_| StoreError::Storage)?;
        let full_position = Self::position(&transaction)?;
        let position = through.unwrap_or(full_position);
        if position > full_position {
            return Err(StoreError::InvalidLimit);
        }
        let records = query_records(
            &transaction,
            resource,
            Position::ZERO,
            usize::try_from(position.get()).map_err(|_| StoreError::InvalidLimit)?,
        )?;
        let receipts = query_receipts(&transaction, Some(position))?;
        transaction.commit().map_err(|_| StoreError::Storage)?;
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
        let mut connection = self.connect_existing(resource)?;
        let transaction = connection.transaction().map_err(|_| StoreError::Storage)?;
        let position = Self::position(&transaction)?;
        if after > position {
            return Err(StoreError::PositionConflict {
                expected: after,
                actual: position,
            });
        }
        let records = query_records(&transaction, resource, after, limit)?;
        transaction.commit().map_err(|_| StoreError::Storage)?;
        Ok(records)
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
        let mut connection = self.connect_existing(resource)?;
        let transaction = connection.transaction().map_err(|_| StoreError::Storage)?;
        let receipt = query_receipt(&transaction, key)?;
        transaction.commit().map_err(|_| StoreError::Storage)?;
        Ok(receipt)
    }

    async fn wait_for_advance(
        &self,
        resource: &ResourceId,
        after: Position,
        deadline_ms: u64,
    ) -> Result<Position, StoreError> {
        loop {
            let notified = self.notify_for(resource).notified_owned();
            let position = self.open(resource).await?.position;
            if position > after || self.clock.now_ms() >= deadline_ms {
                return Ok(position);
            }
            let reread = self.open(resource).await?.position;
            if reread > after {
                return Ok(reread);
            }
            tokio::select! {
                () = notified => {}
                () = tokio::time::sleep(Duration::from_millis(1)) => {}
            }
        }
    }

    async fn remove(
        &self,
        resource: &ResourceId,
        fence: &Fence,
        expected: Position,
    ) -> Result<(), StoreError> {
        if self.take_failpoint(FailPoint::BeforeCommit) {
            return Err(StoreError::FailureInjected(FailPoint::BeforeCommit));
        }
        let _writer = self
            .writer
            .lock()
            .expect("SQLite writer mutex must not be poisoned");
        let path = self.path_for(resource);
        let _resource_lock = lock_resource_file(&path)?;
        let mut connection = self.connect_existing(resource)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| StoreError::Storage)?;
        self.check_fence_tx(&transaction, fence)?;
        let actual = Self::position(&transaction)?;
        if actual != expected {
            return Err(StoreError::PositionConflict { expected, actual });
        }
        write_removal_tombstone(&transaction, resource, expected)?;
        transaction.commit().map_err(|_| StoreError::Storage)?;
        drop(connection);
        remove_database_files(&path)?;
        self.notifications
            .lock()
            .expect("SQLite notification mutex must not be poisoned")
            .remove(resource);
        if self.take_failpoint(FailPoint::AfterCommitBeforeResponse) {
            return Err(StoreError::FailureInjected(
                FailPoint::AfterCommitBeforeResponse,
            ));
        }
        Ok(())
    }
}

fn acquire_fence_transaction(
    transaction: &Transaction<'_>,
    resource: &ResourceId,
    owner: &OwnerId,
    now: u64,
    expires_at_ms: u64,
) -> Result<Fence, StoreError> {
    let current = query_fence(transaction)?;
    if current
        .as_ref()
        .is_some_and(|fence| fence.expires_at_ms > now)
    {
        return Err(StoreError::FenceHeld);
    }
    let epoch = current.map_or(Ok(1_u64), |fence| {
        fence
            .epoch
            .checked_add(1)
            .ok_or(StoreError::PositionOverflow)
    })?;
    transaction
        .execute(
            "INSERT INTO fence(singleton, owner_id, epoch, expires_at_ms)
             VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(singleton) DO UPDATE SET
                owner_id = excluded.owner_id,
                epoch = excluded.epoch,
                expires_at_ms = excluded.expires_at_ms",
            params![
                owner.as_str(),
                to_sql_i64(epoch)?,
                to_sql_i64(expires_at_ms)?
            ],
        )
        .map_err(|_| StoreError::Storage)?;
    Ok(Fence {
        resource: resource.clone(),
        owner: owner.clone(),
        epoch,
        expires_at_ms,
    })
}

fn write_removal_tombstone(
    transaction: &Transaction<'_>,
    resource: &ResourceId,
    position: Position,
) -> Result<(), StoreError> {
    transaction
        .execute_batch(
            "DELETE FROM receipts;
             DELETE FROM records;
             DELETE FROM fence;
             DELETE FROM removal_tombstone;",
        )
        .map_err(|_| StoreError::Storage)?;
    transaction
        .execute(
            "INSERT INTO removal_tombstone(singleton, resource_id, removed_position)
             VALUES (1, ?1, ?2)",
            params![resource.as_str(), to_sql_i64(position.get())?],
        )
        .map_err(|_| StoreError::Storage)?;
    transaction
        .execute("DELETE FROM metadata", [])
        .map_err(|_| StoreError::Storage)?;
    Ok(())
}

fn discover_database(
    root: &Path,
    entry: &fs::DirEntry,
    after: Option<&ResourceId>,
) -> Result<Option<ResourceInfo>, StoreError> {
    let path = entry.path();
    if path.extension().and_then(|value| value.to_str()) != Some("sqlite3") {
        return Ok(None);
    }
    if !entry
        .file_type()
        .map_err(|_| StoreError::Storage)?
        .is_file()
    {
        return Err(StoreError::Corrupt("database entry type"));
    }
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| StoreError::Storage)?;
    configure_connection(&connection)?;
    validate_versions(&connection)?;
    let identity = discover_resource(&connection)?;
    let resource = identity.resource();
    if database_path(root, &resource) != path {
        return Err(StoreError::Corrupt("database filename"));
    }
    if matches!(identity, DatabaseIdentity::Removed(_)) {
        return Ok(None);
    }
    validate_schema(&connection, &resource)?;
    let (count, maximum): (i64, Option<i64>) = connection
        .query_row("SELECT count(*), max(sequence) FROM records", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|_| StoreError::Storage)?;
    if count != maximum.unwrap_or(0) {
        return Err(StoreError::Corrupt("record sequence gap"));
    }
    if after.is_some_and(|after| resource <= *after) {
        return Ok(None);
    }
    Ok(Some(ResourceInfo {
        resource,
        position: Position::new(
            u64::try_from(count).map_err(|_| StoreError::Corrupt("negative record count"))?,
        )?,
    }))
}

fn initialize_database(
    path: &Path,
    resource: &ResourceId,
    initial_fence: Option<&Fence>,
) -> Result<(), StoreError> {
    let mut connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| StoreError::Storage)?;
    configure_connection(&connection)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| StoreError::Storage)?;
    let table_count: i64 = transaction
        .query_row(
            "SELECT count(*) FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| StoreError::Storage)?;
    if table_count == 0 {
        initialize_fresh_database(&transaction, resource, initial_fence)?;
    } else {
        reinitialize_removed_database(&transaction, resource, initial_fence)?;
    }
    transaction.commit().map_err(|_| StoreError::Storage)
}

fn create_or_reinitialize_database(
    path: &Path,
    resource: &ResourceId,
    initial_fence: Option<&Fence>,
) -> Result<(), StoreError> {
    if path.exists() {
        return initialize_database(path, resource, initial_fence);
    }
    let temporary_path = temporary_database_path(path);
    let initialized = initialize_database(&temporary_path, resource, initial_fence);
    if let Err(error) = initialized {
        let _ = remove_database_files(&temporary_path);
        return Err(error);
    }
    match fs::hard_link(&temporary_path, path) {
        Ok(()) => {
            remove_database_files(&temporary_path)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            remove_database_files(&temporary_path)?;
            initialize_database(path, resource, initial_fence)
        }
        Err(_) => {
            let _ = remove_database_files(&temporary_path);
            Err(StoreError::Storage)
        }
    }
}

fn lock_resource_file(path: &Path) -> Result<File, StoreError> {
    let lock_path = PathBuf::from(format!("{}.lock", path.display()));
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|_| StoreError::Storage)?;
    FileExt::lock_exclusive(&file).map_err(|_| StoreError::Storage)?;
    Ok(file)
}

fn temporary_database_path(path: &Path) -> PathBuf {
    let sequence = NEXT_CREATE_FILE.fetch_add(1, Ordering::Relaxed);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    PathBuf::from(format!(
        "{}.creating-{}-{timestamp}-{sequence}",
        path.display(),
        std::process::id()
    ))
}

fn initialize_fresh_database(
    transaction: &Transaction<'_>,
    resource: &ResourceId,
    initial_fence: Option<&Fence>,
) -> Result<(), StoreError> {
    transaction
        .pragma_update(None, "application_id", APPLICATION_ID)
        .map_err(|_| StoreError::Storage)?;
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|_| StoreError::Storage)?;
    transaction
        .execute_batch(schema::CREATE_SCHEMA)
        .map_err(|_| StoreError::Storage)?;
    insert_resource_identity(transaction, resource, initial_fence)
}

fn reinitialize_removed_database(
    transaction: &Transaction<'_>,
    resource: &ResourceId,
    initial_fence: Option<&Fence>,
) -> Result<(), StoreError> {
    validate_versions(transaction)?;
    match discover_resource(transaction)? {
        DatabaseIdentity::Live(stored) if stored == *resource => {
            return Err(StoreError::ResourceExists);
        }
        DatabaseIdentity::Removed(stored) if stored == *resource => {}
        DatabaseIdentity::Live(_) | DatabaseIdentity::Removed(_) => {
            return Err(StoreError::Corrupt("resource identity"));
        }
    }
    transaction
        .execute("DELETE FROM removal_tombstone", [])
        .map_err(|_| StoreError::Storage)?;
    insert_resource_identity(transaction, resource, initial_fence)
}

fn insert_resource_identity(
    transaction: &Transaction<'_>,
    resource: &ResourceId,
    initial_fence: Option<&Fence>,
) -> Result<(), StoreError> {
    transaction
        .execute(
            "INSERT INTO metadata(singleton, resource_id) VALUES (1, ?1)",
            [resource.as_str()],
        )
        .map_err(|_| StoreError::Storage)?;
    if let Some(fence) = initial_fence {
        insert_initial_fence(transaction, fence)?;
    }
    Ok(())
}

fn insert_initial_fence(transaction: &Transaction<'_>, fence: &Fence) -> Result<(), StoreError> {
    transaction
        .execute(
            "INSERT INTO fence(singleton, owner_id, epoch, expires_at_ms)
             VALUES (1, ?1, ?2, ?3)",
            params![
                fence.owner.as_str(),
                to_sql_i64(fence.epoch)?,
                to_sql_i64(fence.expires_at_ms)?
            ],
        )
        .map_err(|_| StoreError::Storage)?;
    Ok(())
}

fn remove_database_files(path: &Path) -> Result<(), StoreError> {
    for candidate in [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ] {
        match fs::remove_file(candidate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(StoreError::Storage),
        }
    }
    Ok(())
}

fn configure_connection(connection: &Connection) -> Result<(), StoreError> {
    connection
        .busy_timeout(Duration::from_millis(5_000))
        .map_err(|_| StoreError::Storage)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(|_| StoreError::Storage)?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(|_| StoreError::Storage)?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|_| StoreError::Storage)?;
    Ok(())
}

fn validate_schema(connection: &Connection, resource: &ResourceId) -> Result<(), StoreError> {
    validate_versions(connection)?;
    let stored: Option<String> = connection
        .query_row(
            "SELECT resource_id FROM metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| StoreError::Storage)?;
    match stored {
        Some(stored) if stored == resource.as_str() => {
            if removal_tombstone(connection)?.is_some() {
                Err(StoreError::Corrupt("live resource has removal tombstone"))
            } else {
                Ok(())
            }
        }
        Some(_) => Err(StoreError::Corrupt("resource identity")),
        None => {
            verify_removed_database(connection, resource)?;
            Err(StoreError::ResourceNotFound)
        }
    }
}

fn validate_versions(connection: &Connection) -> Result<(), StoreError> {
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|_| StoreError::Storage)?;
    let schema_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| StoreError::Storage)?;
    if application_id != i64::from(APPLICATION_ID) {
        return Err(StoreError::Corrupt("application version"));
    }
    if schema_version != SCHEMA_VERSION {
        return Err(StoreError::Corrupt("schema version"));
    }
    Ok(())
}

enum DatabaseIdentity {
    Live(ResourceId),
    Removed(ResourceId),
}

impl DatabaseIdentity {
    fn resource(&self) -> ResourceId {
        match self {
            Self::Live(resource) | Self::Removed(resource) => resource.clone(),
        }
    }
}

fn discover_resource(connection: &Connection) -> Result<DatabaseIdentity, StoreError> {
    let stored: Option<String> = connection
        .query_row(
            "SELECT resource_id FROM metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| StoreError::Storage)?;
    if let Some(stored) = stored {
        if removal_tombstone(connection)?.is_some() {
            return Err(StoreError::Corrupt("live resource has removal tombstone"));
        }
        return ResourceId::new(stored)
            .map(DatabaseIdentity::Live)
            .map_err(|_| StoreError::Corrupt("resource identifier"));
    }
    let Some((resource, _)) = removal_tombstone(connection)? else {
        return Err(StoreError::Corrupt("missing resource identity"));
    };
    verify_removed_database(connection, &resource)?;
    Ok(DatabaseIdentity::Removed(resource))
}

fn removal_tombstone(
    connection: &Connection,
) -> Result<Option<(ResourceId, Position)>, StoreError> {
    connection
        .query_row(
            "SELECT resource_id, removed_position
             FROM removal_tombstone WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|_| StoreError::Storage)?
        .map(|(resource, position)| {
            Ok((
                ResourceId::new(resource)
                    .map_err(|_| StoreError::Corrupt("removed resource identifier"))?,
                Position::new(
                    u64::try_from(position).map_err(|_| StoreError::Corrupt("removed position"))?,
                )?,
            ))
        })
        .transpose()
}

fn verify_removed_database(
    connection: &Connection,
    resource: &ResourceId,
) -> Result<(), StoreError> {
    let tombstone =
        removal_tombstone(connection)?.ok_or(StoreError::Corrupt("missing resource identity"))?;
    if tombstone.0 != *resource {
        return Err(StoreError::Corrupt("removed resource identity"));
    }
    for table in ["records", "receipts", "fence", "metadata"] {
        let query = format!("SELECT count(*) FROM {table}");
        let count: i64 = connection
            .query_row(&query, [], |row| row.get(0))
            .map_err(|_| StoreError::Storage)?;
        if count != 0 {
            return Err(StoreError::Corrupt("nonempty removed resource"));
        }
    }
    Ok(())
}
