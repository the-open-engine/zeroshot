use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use super::identity::{IdempotencyId, OwnerFence, OwnerId, Position, ResourceId};
use super::record::{LedgerRecord, RecordFamily, RecordKind};
use super::store::{
    validate_append_chain, validate_append_request, validate_discovery_limit, validate_prefix,
    validate_range_limit, AppendOutcome, AppendRequest, Clock, CoherentPrefix, LedgerStore,
    OpaqueMutationReceipt, ResourceMetadata, ResourcePage, StoreError,
};

const APPLICATION_ID: i32 = 0x4f45_4c47;
const SCHEMA_VERSION: i32 = 1;
const MAX_DISCOVERY_SCAN_ENTRIES: usize = 4096;

#[derive(Clone)]
pub struct SqliteLedgerStore {
    root: Arc<PathBuf>,
    clock: Arc<dyn Clock>,
    notifications: Arc<Mutex<HashMap<ResourceId, Arc<Notify>>>>,
}

impl SqliteLedgerStore {
    pub fn new(root: impl Into<PathBuf>, clock: Arc<dyn Clock>) -> Result<Self, StoreError> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|_| StoreError::StorageUnavailable)?;
        Ok(Self {
            root: Arc::new(root),
            clock,
            notifications: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    #[must_use]
    pub fn database_path(&self, resource_id: &ResourceId) -> PathBuf {
        self.root.join(database_name(resource_id))
    }

    pub async fn settings(&self, resource_id: &ResourceId) -> Result<SqliteSettings, StoreError> {
        let path = self.database_path(resource_id);
        blocking(move || {
            let connection = open_existing(&path)?;
            let journal_mode: String = connection
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .map_err(storage_error)?;
            let synchronous: i64 = connection
                .query_row("PRAGMA synchronous", [], |row| row.get(0))
                .map_err(storage_error)?;
            let foreign_keys: i64 = connection
                .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
                .map_err(storage_error)?;
            let busy_timeout_millis: i64 = connection
                .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
                .map_err(storage_error)?;
            Ok(SqliteSettings {
                journal_mode,
                synchronous,
                foreign_keys: foreign_keys != 0,
                busy_timeout_millis,
            })
        })
        .await
    }

    fn notification(&self, resource_id: &ResourceId) -> Result<Arc<Notify>, StoreError> {
        let mut notifications = self
            .notifications
            .lock()
            .map_err(|_| StoreError::StorageUnavailable)?;
        Ok(Arc::clone(
            notifications
                .entry(resource_id.clone())
                .or_insert_with(|| Arc::new(Notify::new())),
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqliteSettings {
    pub journal_mode: String,
    pub synchronous: i64,
    pub foreign_keys: bool,
    pub busy_timeout_millis: i64,
}

#[async_trait]
impl LedgerStore for SqliteLedgerStore {
    async fn list_resources(
        &self,
        after: Option<&ResourceId>,
        limit: usize,
    ) -> Result<ResourcePage, StoreError> {
        validate_discovery_limit(limit)?;
        let root = Arc::clone(&self.root);
        let after = after.cloned();
        blocking(move || {
            let entries =
                std::fs::read_dir(root.as_ref()).map_err(|_| StoreError::StorageUnavailable)?;
            let mut resources = BTreeMap::new();
            let mut scanned = 0usize;
            for entry in entries {
                scanned = scanned
                    .checked_add(1)
                    .ok_or(StoreError::BoundExceeded("discovery scan count overflow"))?;
                if scanned > MAX_DISCOVERY_SCAN_ENTRIES {
                    return Err(StoreError::BoundExceeded(
                        "discovery scan exceeds 4096 directory entries",
                    ));
                }
                let entry = entry.map_err(|_| StoreError::StorageUnavailable)?;
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("sqlite") {
                    continue;
                }
                let connection = open_existing(&path)?;
                let metadata = match read_metadata(&connection) {
                    Ok(metadata) => metadata,
                    Err(StoreError::NotFound) => continue,
                    Err(error) => return Err(error),
                };
                if database_name(&metadata.resource_id)
                    != path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .ok_or(StoreError::Corrupt)?
                {
                    return Err(StoreError::Corrupt);
                }
                if after
                    .as_ref()
                    .is_none_or(|after| metadata.resource_id > *after)
                {
                    resources.insert(metadata.resource_id.clone(), metadata);
                    if resources.len() > limit.saturating_add(1) {
                        let largest = resources
                            .keys()
                            .next_back()
                            .cloned()
                            .ok_or(StoreError::Corrupt)?;
                        resources.remove(&largest);
                    }
                }
            }
            let mut resources = resources.into_values().collect::<Vec<_>>();
            let next_after = if resources.len() > limit {
                resources.truncate(limit);
                resources
                    .last()
                    .map(|resource| resource.resource_id.clone())
            } else {
                None
            };
            Ok(ResourcePage {
                resources,
                next_after,
            })
        })
        .await
    }

    async fn create_resource(
        &self,
        resource_id: &ResourceId,
    ) -> Result<ResourceMetadata, StoreError> {
        let path = self.database_path(resource_id);
        let resource_id = resource_id.clone();
        let result = blocking(move || create_database(&path, &resource_id)).await?;
        let _ = self.notification(&result.resource_id)?;
        Ok(result)
    }

    async fn open_resource(
        &self,
        resource_id: &ResourceId,
    ) -> Result<ResourceMetadata, StoreError> {
        let path = self.database_path(resource_id);
        let expected = resource_id.clone();
        blocking(move || {
            let connection = open_existing(&path)?;
            let metadata = read_metadata(&connection)?;
            if metadata.resource_id != expected {
                return Err(StoreError::Corrupt);
            }
            Ok(metadata)
        })
        .await
    }

    async fn acquire_fence(
        &self,
        resource_id: &ResourceId,
        owner: &OwnerId,
        ttl_millis: u64,
    ) -> Result<OwnerFence, StoreError> {
        if ttl_millis == 0 {
            return Err(StoreError::BoundExceeded("fence TTL must be positive"));
        }
        let clock = Arc::clone(&self.clock);
        let path = self.database_path(resource_id);
        let expected = resource_id.clone();
        let owner = owner.clone();
        blocking(move || {
            let mut connection = open_existing(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage_error)?;
            let now = clock.now_unix_millis();
            let expiry = fence_expiry(now, ttl_millis)?;
            ensure_resource(&transaction, &expected)?;
            let current = read_fence(&transaction)?;
            if current
                .as_ref()
                .is_some_and(|fence| !fence.is_expired_at(now) && fence.owner != owner)
            {
                return Err(StoreError::FenceRejected);
            }
            let current_epoch: i64 = transaction
                .query_row(
                    "SELECT next_fence_epoch FROM resource WHERE resource_id = ?1",
                    [expected.as_str()],
                    |row| row.get(0),
                )
                .map_err(storage_error)?;
            let epoch = current_epoch
                .checked_add(1)
                .ok_or(StoreError::BoundExceeded("fence epoch overflow"))?;
            transaction
                .execute(
                    "UPDATE resource SET next_fence_epoch = ?1 WHERE resource_id = ?2",
                    params![epoch, expected.as_str()],
                )
                .map_err(storage_error)?;
            transaction
                .execute(
                    "INSERT INTO fence(singleton, resource_id, owner, epoch, expires_at) \
                     VALUES(1, ?1, ?2, ?3, ?4) \
                     ON CONFLICT(singleton) DO UPDATE SET owner=excluded.owner, \
                     epoch=excluded.epoch, expires_at=excluded.expires_at",
                    params![
                        expected.as_str(),
                        owner.as_str(),
                        epoch,
                        u64_to_i64(expiry)?
                    ],
                )
                .map_err(storage_error)?;
            transaction.commit().map_err(storage_error)?;
            Ok(OwnerFence {
                owner,
                epoch: u64::try_from(epoch).map_err(|_| StoreError::Corrupt)?,
                expires_at_unix_millis: expiry,
            })
        })
        .await
    }

    async fn renew_fence(
        &self,
        resource_id: &ResourceId,
        fence: &OwnerFence,
        ttl_millis: u64,
    ) -> Result<OwnerFence, StoreError> {
        if ttl_millis == 0 {
            return Err(StoreError::BoundExceeded("fence TTL must be positive"));
        }
        let clock = Arc::clone(&self.clock);
        let path = self.database_path(resource_id);
        let expected = resource_id.clone();
        let fence = fence.clone();
        blocking(move || {
            let mut connection = open_existing(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage_error)?;
            let now = clock.now_unix_millis();
            let expiry = fence_expiry(now, ttl_millis)?;
            ensure_resource(&transaction, &expected)?;
            validate_sqlite_fence(&transaction, &fence, now)?;
            transaction
                .execute(
                    "UPDATE fence SET expires_at = ?1 WHERE singleton = 1",
                    [u64_to_i64(expiry)?],
                )
                .map_err(storage_error)?;
            transaction.commit().map_err(storage_error)?;
            Ok(OwnerFence {
                expires_at_unix_millis: expiry,
                ..fence
            })
        })
        .await
    }

    async fn validate_fence(
        &self,
        resource_id: &ResourceId,
        fence: &OwnerFence,
    ) -> Result<(), StoreError> {
        let clock = Arc::clone(&self.clock);
        let path = self.database_path(resource_id);
        let expected = resource_id.clone();
        let fence = fence.clone();
        blocking(move || {
            let mut connection = open_existing(&path)?;
            let transaction = connection.transaction().map_err(storage_error)?;
            ensure_resource(&transaction, &expected)?;
            let now = clock.now_unix_millis();
            validate_sqlite_fence(&transaction, &fence, now)?;
            transaction.commit().map_err(storage_error)
        })
        .await
    }

    async fn read_prefix(&self, resource_id: &ResourceId) -> Result<CoherentPrefix, StoreError> {
        let path = self.database_path(resource_id);
        let expected = resource_id.clone();
        blocking(move || read_records(&path, &expected, Position::ZERO, None)).await
    }

    async fn read_range(
        &self,
        resource_id: &ResourceId,
        after: Position,
        limit: usize,
    ) -> Result<CoherentPrefix, StoreError> {
        validate_range_limit(limit)?;
        let path = self.database_path(resource_id);
        let expected = resource_id.clone();
        blocking(move || read_records(&path, &expected, after, Some(limit))).await
    }

    async fn read_receipt(
        &self,
        resource_id: &ResourceId,
        key: &IdempotencyId,
    ) -> Result<Option<OpaqueMutationReceipt>, StoreError> {
        let path = self.database_path(resource_id);
        let expected = resource_id.clone();
        let key = key.clone();
        blocking(move || {
            let connection = open_existing(&path)?;
            ensure_resource(&connection, &expected)?;
            read_receipt_row(&connection, &expected, &key)
        })
        .await
    }

    async fn compare_and_append(
        &self,
        resource_id: &ResourceId,
        request: AppendRequest,
    ) -> Result<AppendOutcome, StoreError> {
        validate_append_request(resource_id, &request)?;
        let clock = Arc::clone(&self.clock);
        let path = self.database_path(resource_id);
        let expected_resource = resource_id.clone();
        let outcome = blocking(move || {
            compare_and_append_database(&path, &expected_resource, request, clock.as_ref())
        })
        .await?;
        self.notification(resource_id)?.notify_waiters();
        Ok(outcome)
    }

    async fn wait_for_advancement(
        &self,
        resource_id: &ResourceId,
        after: Position,
    ) -> Result<Position, StoreError> {
        loop {
            let current = self.open_resource(resource_id).await?.position;
            if current > after {
                return Ok(current);
            }
            let notified = self.notification(resource_id)?.notified_owned();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let reread = self.open_resource(resource_id).await?.position;
            if reread > after {
                return Ok(reread);
            }
            let _ = tokio::time::timeout(Duration::from_millis(50), notified.as_mut()).await;
        }
    }

    async fn remove_resource(
        &self,
        resource_id: &ResourceId,
        fence: &OwnerFence,
        expected_position: Position,
    ) -> Result<(), StoreError> {
        let clock = Arc::clone(&self.clock);
        let path = self.database_path(resource_id);
        let expected = resource_id.clone();
        let fence = fence.clone();
        let removable_path = path.clone();
        blocking(move || {
            let mut connection = open_existing(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage_error)?;
            ensure_resource(&transaction, &expected)?;
            let now = clock.now_unix_millis();
            validate_sqlite_fence(&transaction, &fence, now)?;
            let metadata = read_metadata(&transaction)?;
            if metadata.position != expected_position {
                return Err(StoreError::PositionConflict {
                    current: metadata.position,
                });
            }
            transaction
                .execute(
                    "DELETE FROM resource WHERE resource_id = ?1",
                    [expected.as_str()],
                )
                .map_err(storage_error)?;
            transaction.commit().map_err(storage_error)?;
            drop(connection);
            remove_database_files(&removable_path)
        })
        .await?;
        if let Some(notification) = self
            .notifications
            .lock()
            .map_err(|_| StoreError::StorageUnavailable)?
            .remove(resource_id)
        {
            notification.notify_waiters();
        }
        Ok(())
    }
}

fn compare_and_append_database(
    path: &Path,
    resource_id: &ResourceId,
    request: AppendRequest,
    clock: &dyn Clock,
) -> Result<AppendOutcome, StoreError> {
    let mut connection = open_existing(path)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    ensure_resource(&transaction, resource_id)?;
    let now = clock.now_unix_millis();
    validate_sqlite_fence(&transaction, &request.fence, now)?;
    let metadata = read_metadata(&transaction)?;
    let tail = validate_authoritative_tail(&transaction, resource_id, &metadata)?;
    if let Some(outcome) = replayed_append(&transaction, resource_id, &metadata, &request)? {
        return Ok(outcome);
    }
    if metadata.position != request.expected_position {
        return Err(StoreError::PositionConflict {
            current: metadata.position,
        });
    }
    let new_position = append_record_batch(
        &transaction,
        resource_id,
        &metadata,
        tail.as_ref(),
        &request.records,
    )?;
    if let Some(receipt) = &request.receipt {
        insert_receipt(&transaction, resource_id, receipt)?;
    }
    transaction.commit().map_err(storage_error)?;
    Ok(AppendOutcome {
        position: new_position,
        receipt: request.receipt,
        replayed: false,
    })
}

fn replayed_append(
    connection: &Connection,
    resource_id: &ResourceId,
    metadata: &ResourceMetadata,
    request: &AppendRequest,
) -> Result<Option<AppendOutcome>, StoreError> {
    let Some(receipt) = &request.receipt else {
        return Ok(None);
    };
    let Some(existing) = read_receipt_row(connection, resource_id, &receipt.key)? else {
        return Ok(None);
    };
    if existing.method != receipt.method || existing.fingerprint != receipt.fingerprint {
        return Err(StoreError::ReceiptConflict);
    }
    Ok(Some(AppendOutcome {
        position: metadata.position,
        receipt: Some(existing),
        replayed: true,
    }))
}

fn append_record_batch(
    connection: &Connection,
    resource_id: &ResourceId,
    metadata: &ResourceMetadata,
    previous: Option<&LedgerRecord>,
    records: &[LedgerRecord],
) -> Result<Position, StoreError> {
    validate_append_chain(resource_id, metadata.position, previous, records)?;
    for record in records {
        insert_record(connection, record)?;
    }
    let appended = u64::try_from(records.len()).map_err(|_| StoreError::Corrupt)?;
    let new_position = Position::new(
        metadata
            .position
            .get()
            .checked_add(appended)
            .ok_or(StoreError::Corrupt)?,
    )
    .map_err(|_| StoreError::Corrupt)?;
    connection
        .execute(
            "UPDATE resource SET position = ?1 WHERE resource_id = ?2",
            params![u64_to_i64(new_position.get())?, resource_id.as_str()],
        )
        .map_err(storage_error)?;
    Ok(new_position)
}

fn insert_receipt(
    connection: &Connection,
    resource_id: &ResourceId,
    receipt: &OpaqueMutationReceipt,
) -> Result<(), StoreError> {
    connection
        .execute(
            "INSERT INTO receipts(\
                resource_id, idempotency_key, method, fingerprint, value, at_position\
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                resource_id.as_str(),
                receipt.key.as_str(),
                &receipt.method,
                receipt.fingerprint.as_slice(),
                &receipt.value,
                u64_to_i64(receipt.at_position.get())?,
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn database_name(resource_id: &ResourceId) -> String {
    let digest = Sha256::digest(resource_id.as_str().as_bytes());
    let mut name = String::with_capacity(71);
    for byte in digest {
        write!(&mut name, "{byte:02x}").expect("writing to String cannot fail");
    }
    name.push_str(".sqlite");
    name
}

fn create_database(path: &Path, resource_id: &ResourceId) -> Result<ResourceMetadata, StoreError> {
    prepare_database_path(path)?;
    initialize_database(path, resource_id)?;
    Ok(ResourceMetadata {
        resource_id: resource_id.clone(),
        position: Position::ZERO,
    })
}

fn prepare_database_path(path: &Path) -> Result<(), StoreError> {
    if path.exists() {
        let connection = Connection::open(path).map_err(storage_error)?;
        let (application_id, schema_version) = database_versions(&connection)?;
        match (application_id, schema_version) {
            (APPLICATION_ID, SCHEMA_VERSION) => match read_metadata(&connection) {
                Ok(_) => return Err(StoreError::AlreadyExists),
                Err(StoreError::NotFound) => {}
                Err(error) => return Err(error),
            },
            (0, 0) | (APPLICATION_ID, 0) | (0, SCHEMA_VERSION) => {}
            _ => return Err(StoreError::Corrupt),
        }
        drop(connection);
        remove_database_files(path)?;
    }
    Ok(())
}

fn initialize_database(path: &Path, resource_id: &ResourceId) -> Result<(), StoreError> {
    let mut connection = match Connection::open(path) {
        Ok(connection) => connection,
        Err(error) => {
            remove_database_files(path)?;
            return Err(storage_error(error));
        }
    };
    let initialization = initialize_schema(&mut connection, resource_id);
    if let Err(error) = initialization {
        drop(connection);
        remove_database_files(path)?;
        return Err(error);
    }
    Ok(())
}

fn initialize_schema(
    connection: &mut Connection,
    resource_id: &ResourceId,
) -> Result<(), StoreError> {
    configure_common(connection)?;
    let (application_id, schema_version) = database_versions(connection)?;
    if application_id != 0 || schema_version != 0 {
        return Err(StoreError::Corrupt);
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    transaction
        .execute_batch(
            "CREATE TABLE resource(\
                    resource_id TEXT PRIMARY KEY,\
                    position INTEGER NOT NULL CHECK(position >= 0),\
                    next_fence_epoch INTEGER NOT NULL CHECK(next_fence_epoch >= 0)\
                 );\
                 CREATE TABLE records(\
                    resource_id TEXT NOT NULL REFERENCES resource(resource_id) ON DELETE CASCADE,\
                    sequence INTEGER NOT NULL CHECK(sequence > 0),\
                    family TEXT NOT NULL, kind TEXT NOT NULL, version INTEGER NOT NULL,\
                    payload BLOB NOT NULL, previous_hash BLOB NOT NULL, record_hash BLOB NOT NULL,\
                    PRIMARY KEY(resource_id, sequence)\
                 );\
                 CREATE TABLE receipts(\
                    resource_id TEXT NOT NULL REFERENCES resource(resource_id) ON DELETE CASCADE,\
                    idempotency_key TEXT NOT NULL, method TEXT NOT NULL,\
                    fingerprint BLOB NOT NULL, value BLOB NOT NULL,\
                    at_position INTEGER NOT NULL CHECK(at_position > 0),\
                    PRIMARY KEY(resource_id, idempotency_key),\
                    UNIQUE(resource_id, at_position),\
                    FOREIGN KEY(resource_id, at_position)\
                        REFERENCES records(resource_id, sequence) ON DELETE CASCADE\
                 );\
                 CREATE TABLE fence(\
                    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),\
                    resource_id TEXT NOT NULL REFERENCES resource(resource_id) ON DELETE CASCADE,\
                    owner TEXT NOT NULL, epoch INTEGER NOT NULL CHECK(epoch > 0),\
                    expires_at INTEGER NOT NULL CHECK(expires_at > 0)\
                 );",
        )
        .map_err(storage_error)?;
    transaction
        .execute(
            "INSERT INTO resource(resource_id, position, next_fence_epoch) VALUES(?1, 0, 0)",
            [resource_id.as_str()],
        )
        .map_err(storage_error)?;
    transaction.commit().map_err(storage_error)?;
    connection
        .execute_batch(&format!(
            "PRAGMA application_id={APPLICATION_ID}; PRAGMA user_version={SCHEMA_VERSION};"
        ))
        .map_err(storage_error)
}

fn configure_common(connection: &Connection) -> Result<(), StoreError> {
    connection
        .busy_timeout(Duration::from_millis(5000))
        .map_err(storage_error)?;
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;\
             PRAGMA synchronous=FULL;\
             PRAGMA foreign_keys=ON;",
        )
        .map_err(storage_error)?;
    Ok(())
}

fn open_existing(path: &Path) -> Result<Connection, StoreError> {
    if !path.is_file() {
        return Err(StoreError::NotFound);
    }
    let connection = Connection::open(path).map_err(storage_error)?;
    connection
        .busy_timeout(Duration::from_millis(5000))
        .map_err(storage_error)?;
    let (application_id, schema_version) = database_versions(&connection)?;
    if application_id != APPLICATION_ID || schema_version != SCHEMA_VERSION {
        return Err(StoreError::Corrupt);
    }
    configure_common(&connection)?;
    Ok(connection)
}

fn database_versions(connection: &Connection) -> Result<(i32, i32), StoreError> {
    let application_id = connection
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .map_err(storage_error)?;
    let schema_version = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(storage_error)?;
    Ok((application_id, schema_version))
}

fn remove_database_files(path: &Path) -> Result<(), StoreError> {
    let sidecar = |suffix: &str| {
        let mut value = path.as_os_str().to_os_string();
        value.push(suffix);
        PathBuf::from(value)
    };
    for candidate in [path.to_path_buf(), sidecar("-wal"), sidecar("-shm")] {
        match std::fs::remove_file(candidate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(StoreError::StorageUnavailable),
        }
    }
    Ok(())
}

fn ensure_resource(connection: &Connection, expected: &ResourceId) -> Result<(), StoreError> {
    let actual: Option<String> = connection
        .query_row("SELECT resource_id FROM resource LIMIT 1", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(storage_error)?;
    match actual {
        Some(actual) if actual == expected.as_str() => Ok(()),
        Some(_) => Err(StoreError::Corrupt),
        None => Err(StoreError::NotFound),
    }
}

fn read_metadata(connection: &Connection) -> Result<ResourceMetadata, StoreError> {
    connection
        .query_row(
            "SELECT resource_id, position FROM resource LIMIT 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(storage_error)?
        .map(|(id, position)| {
            Ok(ResourceMetadata {
                resource_id: ResourceId::new(id).map_err(|_| StoreError::Corrupt)?,
                position: Position::new(i64_to_u64(position)?).map_err(|_| StoreError::Corrupt)?,
            })
        })
        .transpose()?
        .ok_or(StoreError::NotFound)
}

fn read_fence(connection: &Connection) -> Result<Option<OwnerFence>, StoreError> {
    connection
        .query_row(
            "SELECT owner, epoch, expires_at FROM fence WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_error)?
        .map(|(owner, epoch, expiry)| {
            Ok(OwnerFence {
                owner: OwnerId::new(owner).map_err(|_| StoreError::Corrupt)?,
                epoch: i64_to_u64(epoch)?,
                expires_at_unix_millis: i64_to_u64(expiry)?,
            })
        })
        .transpose()
}

fn validate_sqlite_fence(
    connection: &Connection,
    supplied: &OwnerFence,
    now: u64,
) -> Result<(), StoreError> {
    let current = read_fence(connection)?.ok_or(StoreError::FenceRejected)?;
    if current.owner != supplied.owner
        || current.epoch != supplied.epoch
        || current.expires_at_unix_millis != supplied.expires_at_unix_millis
        || current.is_expired_at(now)
    {
        return Err(StoreError::FenceRejected);
    }
    Ok(())
}

fn read_records(
    path: &Path,
    expected: &ResourceId,
    after: Position,
    limit: Option<usize>,
) -> Result<CoherentPrefix, StoreError> {
    let mut connection = open_existing(path)?;
    let transaction = connection.transaction().map_err(storage_error)?;
    ensure_resource(&transaction, expected)?;
    let metadata = read_metadata(&transaction)?;
    if after > metadata.position {
        return Err(StoreError::Corrupt);
    }
    let records = query_records(&transaction, expected, after, limit)?;
    let end = records.last().map_or(after, |record| record.sequence);
    validate_record_range(
        &transaction,
        expected,
        after,
        limit,
        &metadata,
        &records,
        end,
    )?;
    let receipts = read_receipts_in_range(&transaction, expected, after, end)?;
    transaction.commit().map_err(storage_error)?;
    Ok(CoherentPrefix {
        end,
        records,
        receipts,
    })
}

fn query_records(
    connection: &Connection,
    resource_id: &ResourceId,
    after: Position,
    limit: Option<usize>,
) -> Result<Vec<LedgerRecord>, StoreError> {
    let sql = if limit.is_some() {
        "SELECT sequence, family, kind, version, payload, previous_hash, record_hash \
         FROM records WHERE resource_id = ?1 AND sequence > ?2 ORDER BY sequence LIMIT ?3"
    } else {
        "SELECT sequence, family, kind, version, payload, previous_hash, record_hash \
         FROM records WHERE resource_id = ?1 AND sequence > ?2 ORDER BY sequence"
    };
    let mut statement = connection.prepare(sql).map_err(storage_error)?;
    let map_row = |row: &rusqlite::Row<'_>| decode_record_row(resource_id, row);
    let records = match limit {
        Some(limit) => statement
            .query_map(
                params![
                    resource_id.as_str(),
                    u64_to_i64(after.get())?,
                    i64::try_from(limit).map_err(|_| StoreError::Corrupt)?
                ],
                map_row,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(read_error)?,
        None => statement
            .query_map(
                params![resource_id.as_str(), u64_to_i64(after.get())?],
                map_row,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(read_error)?,
    };
    Ok(records)
}

fn validate_record_range(
    connection: &Connection,
    resource_id: &ResourceId,
    after: Position,
    limit: Option<usize>,
    metadata: &ResourceMetadata,
    records: &[LedgerRecord],
    end: Position,
) -> Result<(), StoreError> {
    if after == Position::ZERO {
        validate_prefix(resource_id, records)?;
    } else {
        let previous = read_record_at(connection, resource_id, after)?;
        if previous.resource_id != *resource_id || previous.sequence != after {
            return Err(StoreError::Corrupt);
        }
        previous
            .validate_integrity()
            .map_err(|_| StoreError::Corrupt)?;
        validate_append_chain(resource_id, after, Some(&previous), records)?;
    }
    validate_range_coverage(after, limit, metadata.position, end, records.len())?;
    Ok(())
}

fn validate_range_coverage(
    after: Position,
    limit: Option<usize>,
    resource_end: Position,
    returned_end: Position,
    returned_count: usize,
) -> Result<(), StoreError> {
    if returned_end > resource_end || (after < resource_end && returned_count == 0) {
        return Err(StoreError::Corrupt);
    }
    let should_reach_end = limit.is_none_or(|limit| returned_count < limit);
    if should_reach_end && returned_end != resource_end {
        return Err(StoreError::Corrupt);
    }
    Ok(())
}

fn read_receipts_in_range(
    connection: &Connection,
    resource_id: &ResourceId,
    after: Position,
    end: Position,
) -> Result<BTreeMap<IdempotencyId, OpaqueMutationReceipt>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT idempotency_key, method, fingerprint, value, at_position FROM receipts \
             WHERE resource_id = ?1 AND at_position > ?2 AND at_position <= ?3 \
             ORDER BY at_position",
        )
        .map_err(storage_error)?;
    let rows = statement
        .query_map(
            params![
                resource_id.as_str(),
                u64_to_i64(after.get())?,
                u64_to_i64(end.get())?
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .map_err(storage_error)?;
    let mut receipts = BTreeMap::new();
    for row in rows {
        let (key, method, fingerprint, value, at_position) = row.map_err(read_error)?;
        let key = IdempotencyId::new(key).map_err(|_| StoreError::Corrupt)?;
        let receipt = OpaqueMutationReceipt {
            key: key.clone(),
            method,
            fingerprint: array_32(fingerprint)?,
            value,
            at_position: Position::new(i64_to_u64(at_position)?)
                .map_err(|_| StoreError::Corrupt)?,
        };
        receipt.validate()?;
        read_record_at(connection, resource_id, receipt.at_position)?;
        if receipt.at_position <= after
            || receipt.at_position > end
            || receipts.insert(key, receipt).is_some()
        {
            return Err(StoreError::Corrupt);
        }
    }
    Ok(receipts)
}

fn decode_record_row(
    resource_id: &ResourceId,
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<LedgerRecord> {
    let sequence: i64 = row.get(0)?;
    let family: String = row.get(1)?;
    let kind: String = row.get(2)?;
    let version: i64 = row.get(3)?;
    let payload: Vec<u8> = row.get(4)?;
    let previous_hash: Vec<u8> = row.get(5)?;
    let record_hash: Vec<u8> = row.get(6)?;
    Ok(LedgerRecord {
        resource_id: resource_id.clone(),
        sequence: Position::new(u64::try_from(sequence).map_err(|_| invalid_column(0))?)
            .map_err(|_| invalid_column(0))?,
        family: parse_family(&family).ok_or_else(|| invalid_column(1))?,
        kind: parse_kind(&kind).ok_or_else(|| invalid_column(2))?,
        version: u16::try_from(version).map_err(|_| invalid_column(3))?,
        payload,
        previous_hash: array_32(previous_hash).map_err(|_| invalid_column(5))?,
        record_hash: array_32(record_hash).map_err(|_| invalid_column(6))?,
    })
}

fn validate_authoritative_tail(
    connection: &Connection,
    resource_id: &ResourceId,
    metadata: &ResourceMetadata,
) -> Result<Option<LedgerRecord>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT sequence, family, kind, version, payload, previous_hash, record_hash \
             FROM records WHERE resource_id = ?1 ORDER BY sequence",
        )
        .map_err(storage_error)?;
    let mut rows = statement
        .query([resource_id.as_str()])
        .map_err(storage_error)?;
    let mut expected = Position::ZERO;
    let mut previous_hash = [0; 32];
    let mut tail = None;
    while let Some(row) = rows.next().map_err(storage_error)? {
        let record = decode_record_row(resource_id, row).map_err(read_error)?;
        expected = expected.checked_next().map_err(|_| StoreError::Corrupt)?;
        if record.sequence != expected || record.previous_hash != previous_hash {
            return Err(StoreError::Corrupt);
        }
        record
            .validate_integrity()
            .map_err(|_| StoreError::Corrupt)?;
        previous_hash = record.record_hash;
        tail = Some(record);
    }
    if expected != metadata.position {
        return Err(StoreError::Corrupt);
    }
    Ok(tail)
}

fn read_record_at(
    connection: &Connection,
    resource_id: &ResourceId,
    position: Position,
) -> Result<LedgerRecord, StoreError> {
    connection
        .query_row(
            "SELECT sequence, family, kind, version, payload, previous_hash, record_hash \
             FROM records WHERE resource_id = ?1 AND sequence = ?2",
            params![resource_id.as_str(), u64_to_i64(position.get())?],
            |row| decode_record_row(resource_id, row),
        )
        .map_err(|_| StoreError::Corrupt)
}

fn insert_record(connection: &Connection, record: &LedgerRecord) -> Result<(), StoreError> {
    connection
        .execute(
            "INSERT INTO records(resource_id, sequence, family, kind, version, payload, previous_hash, record_hash) \
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                record.resource_id.as_str(),
                u64_to_i64(record.sequence.get())?,
                record.family.as_str(),
                record.kind.as_str(),
                i64::from(record.version),
                &record.payload,
                record.previous_hash.as_slice(),
                record.record_hash.as_slice(),
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn read_receipt_row(
    connection: &Connection,
    resource_id: &ResourceId,
    key: &IdempotencyId,
) -> Result<Option<OpaqueMutationReceipt>, StoreError> {
    let receipt = connection
        .query_row(
            "SELECT method, fingerprint, value, at_position FROM receipts \
             WHERE resource_id = ?1 AND idempotency_key = ?2",
            params![resource_id.as_str(), key.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_error)?
        .map(|(method, fingerprint, value, at_position)| {
            let receipt = OpaqueMutationReceipt {
                key: key.clone(),
                method,
                fingerprint: array_32(fingerprint)?,
                value,
                at_position: Position::new(i64_to_u64(at_position)?)
                    .map_err(|_| StoreError::Corrupt)?,
            };
            receipt.validate()?;
            Ok(receipt)
        })
        .transpose()?;
    if let Some(receipt) = &receipt {
        read_record_at(connection, resource_id, receipt.at_position)?;
        if receipt.at_position > read_metadata(connection)?.position {
            return Err(StoreError::Corrupt);
        }
    }
    Ok(receipt)
}

fn parse_family(value: &str) -> Option<RecordFamily> {
    match value {
        "control" => Some(RecordFamily::Control),
        "verified_io" => Some(RecordFamily::VerifiedIo),
        _ => None,
    }
}

fn parse_kind(value: &str) -> Option<RecordKind> {
    match value {
        "admission" => Some(RecordKind::Admission),
        "dispatch" => Some(RecordKind::Dispatch),
        "settlement" => Some(RecordKind::Settlement),
        "void" => Some(RecordKind::Void),
        "safe_fault" => Some(RecordKind::SafeFault),
        "effect_intent" => Some(RecordKind::EffectIntent),
        "effect_receipt" => Some(RecordKind::EffectReceipt),
        "lifecycle_update" => Some(RecordKind::LifecycleUpdate),
        "stop_requested" => Some(RecordKind::StopRequested),
        "terminal" => Some(RecordKind::Terminal),
        "cleanup_receipt" => Some(RecordKind::CleanupReceipt),
        "mutation_receipt" => Some(RecordKind::MutationReceipt),
        _ => None,
    }
}

fn array_32(value: Vec<u8>) -> Result<[u8; 32], StoreError> {
    value.try_into().map_err(|_| StoreError::Corrupt)
}

fn invalid_column(index: usize) -> rusqlite::Error {
    rusqlite::Error::InvalidColumnType(index, "corrupt".to_owned(), rusqlite::types::Type::Blob)
}

fn fence_expiry(now: u64, ttl_millis: u64) -> Result<u64, StoreError> {
    if ttl_millis == 0 {
        return Err(StoreError::BoundExceeded("fence TTL must be positive"));
    }
    now.checked_add(ttl_millis)
        .filter(|expiry| *expiry <= i64::MAX as u64)
        .ok_or(StoreError::BoundExceeded("fence expiry overflow"))
}

fn u64_to_i64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Corrupt)
}

fn i64_to_u64(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Corrupt)
}

fn storage_error(_error: rusqlite::Error) -> StoreError {
    StoreError::StorageUnavailable
}

fn read_error(error: rusqlite::Error) -> StoreError {
    match error {
        rusqlite::Error::InvalidColumnType(..)
        | rusqlite::Error::FromSqlConversionFailure(..)
        | rusqlite::Error::IntegralValueOutOfRange(..)
        | rusqlite::Error::Utf8Error(..) => StoreError::Corrupt,
        _ => StoreError::StorageUnavailable,
    }
}

async fn blocking<T, F>(operation: F) -> Result<T, StoreError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, StoreError> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| StoreError::StorageUnavailable)?
}
