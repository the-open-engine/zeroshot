//! Minimal local durable adapter for the native-v2 run ledger port.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use async_trait::async_trait;
use openengine_cluster_protocol::{Cursor, IdempotencyKey, RunId, UnixTimestampMillis};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use super::{
    AppendResult, CreateRun, CreateRunOutcome, LedgerClock, RunEvent, RunLedger, RunLedgerError,
    RunSummary, SnapshotAndTail, StoredRun, StoredRunEvent, apply_stored_event, batch_recorded_at,
    cursor_for, cursor_sequence, prepare_event, system_clock, validate_create, MAX_REPLAY_BYTES,
    MAX_REPLAY_EVENTS,
};

const SCHEMA: &str = "
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS v2_runs (
    run_id TEXT PRIMARY KEY NOT NULL,
    submission_key TEXT UNIQUE NOT NULL,
    submission_digest TEXT NOT NULL,
    cursor INTEGER NOT NULL,
    stored_json TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS v2_run_events (
    run_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    event_json TEXT NOT NULL,
    recorded_at_ms INTEGER,
    continuation_checkpoint_id TEXT,
    PRIMARY KEY (run_id, sequence),
    FOREIGN KEY (run_id) REFERENCES v2_runs(run_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX IF NOT EXISTS v2_run_vector_ids
    ON v2_run_events (run_id, json_extract(event_json, '$.vector.id'), sequence)
    WHERE json_extract(event_json, '$.kind') = 'vector';
CREATE INDEX IF NOT EXISTS v2_run_steering
    ON v2_run_events (run_id, sequence)
    WHERE json_extract(event_json, '$.kind') = 'vector'
        AND json_extract(event_json, '$.vector.kind') IN ('steer', 'steer_receipt');
";

const VECTOR_CURSOR_QUERY: &str = "SELECT sequence FROM v2_run_events
    WHERE run_id = ?1 AND json_extract(event_json, '$.kind') = 'vector'
        AND json_extract(event_json, '$.vector.id') = ?2
    ORDER BY sequence LIMIT 1";

/// Read-only ledgers retain their original schema. Prepare the matching projection once per open.
#[derive(Clone, Copy, PartialEq)]
struct EventColumns {
    recorded_at: bool,
    continuation_checkpoint: bool,
}

impl EventColumns {
    const CURRENT: Self = Self {
        recorded_at: true,
        continuation_checkpoint: true,
    };

    fn inspect(connection: &Connection) -> Result<Self, RunLedgerError> {
        connection
            .query_row(
                "SELECT count(*) FILTER (WHERE name = 'recorded_at_ms'),
                        count(*) FILTER (WHERE name = 'continuation_checkpoint_id')
                 FROM pragma_table_info('v2_run_events')",
                [],
                |row| {
                    Ok(Self {
                        recorded_at: row.get(0)?,
                        continuation_checkpoint: row.get(1)?,
                    })
                },
            )
            .map_err(sqlite_error)
    }

    fn replay_query(self) -> Arc<str> {
        let event = if self.continuation_checkpoint {
            "CASE WHEN continuation_checkpoint_id IS NULL THEN event_json
               ELSE json_set(event_json, '$.vector.checkpointId', continuation_checkpoint_id) END"
        } else {
            "event_json"
        };
        let time = if self.recorded_at {
            "recorded_at_ms"
        } else {
            "NULL"
        };
        format!(
            "SELECT sequence, {event}, {time} FROM v2_run_events
                 WHERE run_id = ?1 AND sequence > ?2 ORDER BY sequence LIMIT ?3"
        )
        .into()
    }
}

#[path = "sqlite/readonly.rs"]
mod readonly;
#[path = "sqlite/steering.rs"]
mod steering;

// The temporary recovered image must live through clones and cancelled blocking operations,
// and must be removed only after SQLite has closed its handles (including on Windows).
struct LedgerConnection {
    connection: Connection,
    path: Option<PathBuf>,
    _snapshot: Option<tempfile::TempDir>,
}

#[derive(Clone)]
pub struct SqliteRunLedger {
    connection: Arc<Mutex<LedgerConnection>>,
    clock: LedgerClock,
    replay_query: Arc<str>,
}

impl SqliteRunLedger {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RunLedgerError> {
        let path = path.as_ref();
        let connection = Connection::open(path).map_err(|error| {
            storage_context(sqlite_error(error), None, || available_space(Some(path)))
        })?;
        let path = match connection.path() {
            Some("") => None,
            Some(path) => Some(PathBuf::from(path)),
            None => std::path::absolute(path).ok(),
        };
        Self::from_connection(connection, path)
    }

    /// Opens retained history without creating a database, changing schema, or admitting writes.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self, RunLedgerError> {
        let connection = readonly::open(path.as_ref())?;
        let columns = EventColumns::inspect(&connection.connection).map_err(|error| {
            storage_context(error, Some(&connection.connection), || {
                available_space(connection.path.as_deref())
            })
        })?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            clock: system_clock(),
            replay_query: columns.replay_query(),
        })
    }

    pub fn open_in_memory() -> Result<Self, RunLedgerError> {
        let connection = Connection::open_in_memory().map_err(sqlite_error)?;
        Self::from_connection(connection, None)
    }

    fn from_connection(
        mut connection: Connection,
        path: Option<PathBuf>,
    ) -> Result<Self, RunLedgerError> {
        let initialized = (|| {
            connection.execute_batch(SCHEMA).map_err(sqlite_error)?;
            add_event_columns(&mut connection)
        })();
        initialized.map_err(|error| {
            storage_context(error, Some(&connection), || {
                available_space(path.as_deref())
            })
        })?;
        Ok(Self {
            connection: Arc::new(Mutex::new(LedgerConnection {
                connection,
                path,
                _snapshot: None,
            })),
            clock: system_clock(),
            replay_query: EventColumns::CURRENT.replay_query(),
        })
    }

    /// Stamps recorded events from `clock` instead of the system wall clock.
    #[must_use]
    pub fn with_wall_clock(mut self, clock: LedgerClock) -> Self {
        self.clock = clock;
        self
    }

    /// Lists a bounded descending page through the run-ID index without loading run payloads.
    #[cfg(feature = "ui")]
    pub(crate) async fn list_ids_page(
        &self,
        after: Option<&RunId>,
        limit: u16,
    ) -> Result<Vec<RunId>, RunLedgerError> {
        let after = after.cloned();
        self.with_connection(move |connection| {
            let query = if after.is_some() {
                "SELECT run_id FROM v2_runs WHERE run_id < ?1 ORDER BY run_id DESC LIMIT ?2"
            } else {
                "SELECT run_id FROM v2_runs ORDER BY run_id DESC LIMIT ?1"
            };
            let mut statement = connection.prepare(query).map_err(sqlite_error)?;
            let mut rows = match after {
                Some(after) => statement.query(params![after.as_str(), limit]),
                None => statement.query(params![limit]),
            }
            .map_err(sqlite_error)?;
            let mut ids = Vec::new();
            while let Some(row) = rows.next().map_err(sqlite_error)? {
                ids.push(RunId::new(row.get::<_, String>(0).map_err(sqlite_error)?));
            }
            Ok(ids)
        })
        .await
    }

    async fn with_connection<T, F>(&self, operation: F) -> Result<T, RunLedgerError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, RunLedgerError> + Send + 'static,
    {
        // Wait asynchronously before reserving a blocking thread. The closure owns the guard,
        // so cancelling its caller cannot admit another operation before this one finishes.
        let mut connection = self.connection.clone().lock_owned().await;
        tokio::task::spawn_blocking(move || {
            operation(&mut connection.connection).map_err(|error| {
                storage_context(error, Some(&connection.connection), || {
                    available_space(connection.path.as_deref())
                })
            })
        })
        .await
        .map_err(|_| RunLedgerError::Storage)?
    }

    async fn with_transaction<T, F>(&self, operation: F) -> Result<T, RunLedgerError>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> Result<T, RunLedgerError> + Send + 'static,
    {
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            // Preserve the native cause before rollback can replace SQLite's error state.
            let result =
                operation(&transaction).map_err(|error| system_error(error, &transaction))?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(result)
        })
        .await
    }
}

#[async_trait]
impl RunLedger for SqliteRunLedger {
    async fn create_or_get(&self, request: CreateRun) -> Result<CreateRunOutcome, RunLedgerError> {
        validate_create(&request)?;
        self.with_transaction(move |transaction| {
            match existing_submission(transaction, &request)? {
                Some(existing) => Ok(CreateRunOutcome::Existing(existing)),
                None => Ok(CreateRunOutcome::Created(insert_new_run(
                    transaction,
                    request,
                )?)),
            }
        })
        .await
    }

    async fn get(&self, run_id: &RunId) -> Result<Option<StoredRun>, RunLedgerError> {
        let run_id = run_id.clone();
        self.with_connection(move |connection| load_by_id(connection, &run_id))
            .await
    }

    async fn get_by_submission_key(
        &self,
        submission_key: &IdempotencyKey,
    ) -> Result<Option<StoredRun>, RunLedgerError> {
        let submission_key = submission_key.clone();
        self.with_connection(move |connection| {
            load_by_submission(connection, submission_key.as_str())
                .map(|stored| stored.map(|(_, _, stored)| stored))
        })
        .await
    }

    async fn list(&self) -> Result<Vec<RunSummary>, RunLedgerError> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT stored_json FROM v2_runs ORDER BY rowid")
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(sqlite_error)?;
            rows.map(|row| {
                let stored: StoredRun =
                    openengine_cluster_protocol::bounded_json_from_str(&row.map_err(sqlite_error)?)
                        .map_err(|_| RunLedgerError::Corrupt)?;
                validate_stored_size(&stored)?;
                Ok(RunSummary::from(&stored.snapshot))
            })
            .collect()
        })
        .await
    }

    async fn append(
        &self,
        run_id: &RunId,
        events: Vec<RunEvent>,
    ) -> Result<AppendResult, RunLedgerError> {
        let run_id = run_id.clone();
        let clock = self.clock.clone();
        self.with_transaction(move |transaction| {
            append_transaction(transaction, &clock, &run_id, events)
        })
        .await
    }

    async fn request_force_stop(&self, run_id: &RunId) -> Result<AppendResult, RunLedgerError> {
        let run_id = run_id.clone();
        let clock = self.clock.clone();
        self.with_transaction(move |transaction| {
            let stored = load_by_id(transaction, &run_id)?.ok_or(RunLedgerError::RunNotFound)?;
            if stored.snapshot.force_stop_requested || stored.snapshot.terminal.is_some() {
                Ok(AppendResult {
                    snapshot: stored.snapshot,
                    events: Vec::new(),
                })
            } else {
                append_transaction(
                    transaction,
                    &clock,
                    &run_id,
                    vec![RunEvent::ForceStopRequested],
                )
            }
        })
        .await
    }

    async fn request_suspend(&self, run_id: &RunId) -> Result<AppendResult, RunLedgerError> {
        let run_id = run_id.clone();
        let clock = self.clock.clone();
        self.with_transaction(move |transaction| {
            let stored = load_by_id(transaction, &run_id)?.ok_or(RunLedgerError::RunNotFound)?;
            match super::suspension_request_events(&stored)? {
                Some(events) => append_transaction(transaction, &clock, &run_id, events),
                None => Ok(AppendResult {
                    snapshot: stored.snapshot,
                    events: Vec::new(),
                }),
            }
        })
        .await
    }

    async fn vector_cursor(
        &self,
        run_id: &RunId,
        id: &openengine_cluster_protocol::VectorId,
    ) -> Result<Cursor, RunLedgerError> {
        let run_id = run_id.clone();
        let id = id.clone();
        self.with_connection(move |connection| {
            let sequence = connection
                .query_row(
                    VECTOR_CURSOR_QUERY,
                    params![run_id.as_str(), id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(sqlite_error)?;
            match sequence {
                Some(sequence) => Ok(cursor_for(
                    u64::try_from(sequence).map_err(|_| RunLedgerError::Corrupt)?,
                )),
                None if load_by_id(connection, &run_id)?.is_none() => {
                    Err(RunLedgerError::RunNotFound)
                }
                None => Err(RunLedgerError::VectorNotFound),
            }
        })
        .await
    }

    async fn snapshot_and_tail(
        &self,
        run_id: &RunId,
        after: Option<&Cursor>,
    ) -> Result<SnapshotAndTail, RunLedgerError> {
        let run_id = run_id.clone();
        let after = after.cloned();
        let query = Arc::clone(&self.replay_query);
        self.with_connection(move |connection| {
            snapshot_page(connection, &query, &run_id, after.as_ref())
        })
        .await
    }

    async fn steering_events(
        &self,
        run_id: &RunId,
        after: Option<&Cursor>,
    ) -> Result<Vec<StoredRunEvent>, RunLedgerError> {
        let run_id = run_id.clone();
        let after = after.cloned();
        self.with_connection(move |connection| steering::read(connection, &run_id, after.as_ref()))
            .await
    }
}

fn snapshot_page(
    connection: &Connection,
    query: &str,
    run_id: &RunId,
    after: Option<&Cursor>,
) -> Result<SnapshotAndTail, RunLedgerError> {
    let stored = load_by_id(connection, run_id)?.ok_or(RunLedgerError::RunNotFound)?;
    let after = after.map_or(Ok(0), cursor_sequence)?;
    if after > cursor_sequence(&stored.snapshot.cursor)? {
        return Err(RunLedgerError::CursorAhead);
    }
    let after = i64::try_from(after).map_err(|_| RunLedgerError::CursorAhead)?;
    Ok(SnapshotAndTail {
        snapshot: stored.snapshot,
        events: read_page(connection, query, run_id, after)?,
    })
}

fn read_page(
    connection: &Connection,
    query: &str,
    run_id: &RunId,
    after: i64,
) -> Result<Vec<StoredRunEvent>, RunLedgerError> {
    let mut statement = connection.prepare(query).map_err(sqlite_error)?;
    let mut rows = statement
        .query(params![run_id.as_str(), after, MAX_REPLAY_EVENTS as i64])
        .map_err(sqlite_error)?;
    let mut events = Vec::new();
    let mut raw_bytes = 0;
    while let Some(row) = rows.next().map_err(sqlite_error)? {
        let json = row.get_ref(1).map_err(sqlite_error)?;
        let json = json.as_str().map_err(|_| RunLedgerError::Corrupt)?;
        if json.len() > MAX_REPLAY_BYTES {
            return Err(RunLedgerError::Corrupt);
        }
        if raw_bytes + json.len() > MAX_REPLAY_BYTES {
            break;
        }
        events.push(decode_replay_event(row, json)?);
        raw_bytes += json.len();
    }
    Ok(events)
}

fn decode_replay_event(
    row: &rusqlite::Row<'_>,
    json: &str,
) -> Result<StoredRunEvent, RunLedgerError> {
    let sequence = row.get::<_, i64>(0).map_err(sqlite_error)?;
    let sequence = u64::try_from(sequence).map_err(|_| RunLedgerError::Corrupt)?;
    let event = openengine_cluster_protocol::bounded_json_from_str(json)
        .map_err(|_| RunLedgerError::Corrupt)?;
    let recorded_at = row
        .get::<_, Option<i64>>(2)
        .map_err(sqlite_error)?
        .map(decode_recorded_at)
        .transpose()?;
    Ok(StoredRunEvent {
        cursor: cursor_for(sequence),
        recorded_at,
        event,
    })
}

fn decode_recorded_at(milliseconds: i64) -> Result<UnixTimestampMillis, RunLedgerError> {
    u64::try_from(milliseconds)
        .ok()
        .and_then(|milliseconds| UnixTimestampMillis::new(milliseconds).ok())
        .ok_or(RunLedgerError::Corrupt)
}

/// Acquire the write lock only when a migration is needed. Recheck under the lock because
/// another opener can complete it first. Capture native SQLite evidence before rollback.
fn add_event_columns(connection: &mut Connection) -> Result<(), RunLedgerError> {
    if EventColumns::inspect(connection)? == EventColumns::CURRENT {
        return Ok(());
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite_error)?;
    let columns =
        EventColumns::inspect(&transaction).map_err(|error| system_error(error, &transaction))?;
    if !columns.recorded_at {
        transaction
            .execute_batch("ALTER TABLE v2_run_events ADD COLUMN recorded_at_ms INTEGER")
            .map_err(|error| system_error(sqlite_error(error), &transaction))?;
    }
    if !columns.continuation_checkpoint {
        transaction
            .execute_batch("ALTER TABLE v2_run_events ADD COLUMN continuation_checkpoint_id TEXT")
            .map_err(|error| system_error(sqlite_error(error), &transaction))?;
    }
    transaction.commit().map_err(sqlite_error)
}

/// The latest recorded time in the run. Rows without one (written before the column existed, or
/// by an older binary after it was added) are skipped so a later time can never be earlier.
fn previous_recorded_at(
    transaction: &Transaction<'_>,
    run_id: &RunId,
) -> Result<Option<UnixTimestampMillis>, RunLedgerError> {
    transaction
        .query_row(
            "SELECT recorded_at_ms FROM v2_run_events
             WHERE run_id = ?1 AND recorded_at_ms IS NOT NULL
             ORDER BY sequence DESC LIMIT 1",
            params![run_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(sqlite_error)?
        .map(decode_recorded_at)
        .transpose()
}

fn append_transaction(
    transaction: &Transaction<'_>,
    clock: &LedgerClock,
    run_id: &RunId,
    events: Vec<RunEvent>,
) -> Result<AppendResult, RunLedgerError> {
    require_events(&events)?;
    let stored = load_by_id(transaction, run_id)?.ok_or(RunLedgerError::RunNotFound)?;
    let sequence = cursor_sequence(&stored.snapshot.cursor)?;
    let previous = previous_recorded_at(transaction, run_id)?;
    let mut append = PendingAppend {
        transaction,
        run_id,
        stored,
        sequence,
        recorded_at: batch_recorded_at(clock, previous),
    };
    let appended = append.events(events)?;
    update_stored_run(transaction, run_id, append.sequence, &append.stored)?;
    Ok(AppendResult {
        snapshot: append.stored.snapshot,
        events: appended,
    })
}

fn require_events(events: &[RunEvent]) -> Result<(), RunLedgerError> {
    if events.is_empty() {
        Err(RunLedgerError::InvalidEvent("event batch is empty"))
    } else {
        Ok(())
    }
}

struct PendingAppend<'a, 'connection> {
    transaction: &'a Transaction<'connection>,
    run_id: &'a RunId,
    stored: StoredRun,
    sequence: u64,
    recorded_at: UnixTimestampMillis,
}

impl PendingAppend<'_, '_> {
    fn events(&mut self, events: Vec<RunEvent>) -> Result<Vec<StoredRunEvent>, RunLedgerError> {
        let mut appended = Vec::with_capacity(events.len());
        for event in events {
            appended.push(self.one_event(event)?);
        }
        Ok(appended)
    }

    fn one_event(&mut self, event: RunEvent) -> Result<StoredRunEvent, RunLedgerError> {
        let event = prepare_event(&self.stored.snapshot, event);
        self.sequence = next_sequence(self.sequence)?;
        let stored = StoredRunEvent {
            cursor: cursor_for(self.sequence),
            recorded_at: Some(self.recorded_at),
            event,
        };
        apply_stored_event(&mut self.stored.snapshot, &stored)?;
        insert_event(self.transaction, self.run_id, self.sequence, &stored)?;
        Ok(stored)
    }
}

fn next_sequence(current: u64) -> Result<u64, RunLedgerError> {
    let next = current.checked_add(1).ok_or(RunLedgerError::Storage)?;
    i64::try_from(next).map_err(|_| RunLedgerError::Storage)?;
    Ok(next)
}

fn insert_event(
    transaction: &Transaction<'_>,
    run_id: &RunId,
    sequence: u64,
    stored: &StoredRunEvent,
) -> Result<(), RunLedgerError> {
    let sequence = i64::try_from(sequence).map_err(|_| RunLedgerError::Storage)?;
    let event_json = serde_json::to_string(&stored.event).map_err(|_| RunLedgerError::Storage)?;
    // Old readers decode event_json strictly. Store the optional rewind identity beside it,
    // including for vectors inherited by later successors, in this same atomic row.
    let checkpoint = if let RunEvent::Vector { vector } = &stored.event
        && let openengine_cluster_protocol::RunVectorContent::Continuation { checkpoint_id, .. } =
            &vector.content
    {
        checkpoint_id.as_ref().map(|id| id.as_str())
    } else {
        None
    };
    let recorded_at = stored
        .recorded_at
        .map(|time| i64::try_from(time.get()).map_err(|_| RunLedgerError::Storage))
        .transpose()?;
    transaction
        .execute(
            "INSERT INTO v2_run_events (run_id, sequence, event_json, recorded_at_ms, continuation_checkpoint_id)
             VALUES (?1, ?2, CASE WHEN ?5 IS NULL THEN ?3 ELSE json_remove(?3, '$.vector.checkpointId') END, ?4, ?5)",
            params![run_id.as_str(), sequence, event_json, recorded_at, checkpoint],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

fn update_stored_run(
    transaction: &Transaction<'_>,
    run_id: &RunId,
    sequence: u64,
    stored: &StoredRun,
) -> Result<(), RunLedgerError> {
    let stored_json = serde_json::to_string(&stored).map_err(|_| RunLedgerError::Storage)?;
    let changed = transaction
        .execute(
            "UPDATE v2_runs SET cursor = ?2, stored_json = ?3 WHERE run_id = ?1",
            params![run_id.as_str(), sequence as i64, stored_json],
        )
        .map_err(sqlite_error)?;
    if changed != 1 {
        return Err(RunLedgerError::Corrupt);
    }
    Ok(())
}

fn existing_submission(
    transaction: &Transaction<'_>,
    request: &CreateRun,
) -> Result<Option<StoredRun>, RunLedgerError> {
    let Some((existing_id, digest, existing)) =
        load_by_submission(transaction, request.submission_key.as_str())?
    else {
        require_unused_run_id(transaction, &request.run_id)?;
        return Ok(None);
    };
    if digest == request.submission_digest.as_str() && existing.admitted == request.admitted {
        Ok(Some(existing))
    } else {
        Err(RunLedgerError::SubmissionConflict {
            existing_run_id: RunId::new(existing_id),
        })
    }
}

fn require_unused_run_id(
    transaction: &Transaction<'_>,
    run_id: &RunId,
) -> Result<(), RunLedgerError> {
    if load_by_id(transaction, run_id)?.is_some() {
        Err(RunLedgerError::RunIdConflict)
    } else {
        Ok(())
    }
}

fn insert_new_run(
    transaction: &Transaction<'_>,
    request: CreateRun,
) -> Result<StoredRun, RunLedgerError> {
    let snapshot = super::RunSnapshot::admitted(request.run_id.clone(), &request.admitted)?;
    let stored = StoredRun {
        submission_key: request.submission_key,
        submission_digest: request.submission_digest,
        admitted: request.admitted,
        snapshot,
    };
    let stored_json = serde_json::to_string(&stored).map_err(|_| RunLedgerError::Storage)?;
    transaction
        .execute(
            "INSERT INTO v2_runs
             (run_id, submission_key, submission_digest, cursor, stored_json)
             VALUES (?1, ?2, ?3, 0, ?4)",
            params![
                request.run_id.as_str(),
                stored.submission_key.as_str(),
                stored.submission_digest.as_str(),
                stored_json,
            ],
        )
        .map_err(sqlite_error)?;
    Ok(stored)
}

fn load_by_id(
    connection: &Connection,
    run_id: &RunId,
) -> Result<Option<StoredRun>, RunLedgerError> {
    let row = connection
        .query_row(
            "SELECT cursor, stored_json FROM v2_runs WHERE run_id = ?1",
            [run_id.as_str()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(sqlite_error)?;
    row.map(|(cursor, json)| decode_stored(connection, run_id, cursor, &json))
        .transpose()
}

fn load_by_submission(
    connection: &Connection,
    submission_key: &str,
) -> Result<Option<(String, String, StoredRun)>, RunLedgerError> {
    let row = connection
        .query_row(
            "SELECT run_id, submission_digest, cursor, stored_json
             FROM v2_runs WHERE submission_key = ?1",
            [submission_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?;
    row.map(|(run_id, digest, cursor, json)| {
        let stored = decode_stored(connection, &RunId::new(run_id.clone()), cursor, &json)?;
        Ok((run_id, digest, stored))
    })
    .transpose()
}

fn decode_stored(
    connection: &Connection,
    expected_run_id: &RunId,
    cursor: i64,
    json: &str,
) -> Result<StoredRun, RunLedgerError> {
    let mut stored: StoredRun = openengine_cluster_protocol::bounded_json_from_str(json)
        .map_err(|_| RunLedgerError::Corrupt)?;
    validate_stored_size(&stored)?;
    let cursor = u64::try_from(cursor).map_err(|_| RunLedgerError::Corrupt)?;
    if stored.snapshot.run_id != *expected_run_id
        || cursor_sequence(&stored.snapshot.cursor)? != cursor
    {
        return Err(RunLedgerError::Corrupt);
    }
    restore_active_start_times(connection, &mut stored.snapshot)?;
    Ok(stored)
}

/// Older snapshots retain start cursors but not times. Read only those exact indexed rows,
/// without scanning history or rewriting retained state, including on read-only connections.
fn restore_active_start_times(
    connection: &Connection,
    snapshot: &mut super::RunSnapshot,
) -> Result<(), RunLedgerError> {
    if !snapshot
        .active_executions()
        .any(|node| node.started_recorded_at.is_none())
        || !EventColumns::inspect(connection)?.recorded_at
    {
        return Ok(());
    }
    let mut statement = connection
        .prepare_cached(
            "SELECT recorded_at_ms FROM v2_run_events WHERE run_id = ?1 AND sequence = ?2",
        )
        .map_err(sqlite_error)?;
    for node in snapshot.executions.values_mut().filter(|node| {
        matches!(node.state, super::NodeState::Active) && node.started_recorded_at.is_none()
    }) {
        let sequence = i64::try_from(cursor_sequence(&node.started_at)?)
            .map_err(|_| RunLedgerError::Corrupt)?;
        node.started_recorded_at = statement
            .query_row(params![snapshot.run_id.as_str(), sequence], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .optional()
            .map_err(sqlite_error)?
            .flatten()
            .map(decode_recorded_at)
            .transpose()?;
    }
    Ok(())
}

fn validate_stored_size(stored: &StoredRun) -> Result<(), RunLedgerError> {
    if stored.admitted.runtime.size() != Some(&stored.snapshot.size) {
        return Err(RunLedgerError::Corrupt);
    }
    Ok(())
}

fn sqlite_error(error: rusqlite::Error) -> RunLedgerError {
    match error {
        rusqlite::Error::SqliteFailure(code, _) => RunLedgerError::SqliteStorage(code.into()),
        _ => RunLedgerError::Storage,
    }
}

fn system_error(mut error: RunLedgerError, connection: &Connection) -> RunLedgerError {
    if let RunLedgerError::SqliteStorage(failure) = &mut error
        && failure.system_error.is_none()
        // NOMEM leaves the previous native error intact; Windows detects ISDIR through
        // successful file attributes, so neither establishes a new failed OS operation.
        && !matches!(failure.code.extended_code,
            rusqlite::ffi::SQLITE_IOERR_NOMEM | rusqlite::ffi::SQLITE_CANTOPEN_ISDIR)
        && matches!(
            failure.code.code,
            rusqlite::ffi::ErrorCode::CannotOpen | rusqlite::ffi::ErrorCode::SystemIoFailure
        )
    {
        // SAFETY: the caller exclusively owns this live connection. Read only SQLite's
        // machine error state; never its potentially caller-controlled error message.
        unsafe {
            if rusqlite::ffi::sqlite3_extended_errcode(connection.handle())
                == failure.code.extended_code
            {
                let code = rusqlite::ffi::sqlite3_system_errno(connection.handle());
                failure.system_error = (code != 0).then_some(code);
            }
        }
    }
    error
}

fn storage_context(
    error: RunLedgerError,
    connection: Option<&Connection>,
    measure_space: impl FnOnce() -> Option<u64>,
) -> RunLedgerError {
    let mut error = match connection {
        Some(connection) => system_error(error, connection),
        None => error,
    };
    if let RunLedgerError::SqliteStorage(failure) = &mut error {
        failure.available_bytes = measure_space();
    }
    error
}

fn available_space(path: Option<&Path>) -> Option<u64> {
    let path = std::path::absolute(path?).ok()?;
    // A failed open may not have created the database. Use its immediate parent,
    // never an ancestor on a potentially different volume.
    let volume = match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.parent()?,
        _ => &path,
    };
    fs2::available_space(volume).ok()
}

#[cfg(test)]
#[path = "sqlite/storage_failure_tests.rs"]
mod storage_failure_tests;

#[cfg(test)]
#[path = "sqlite/scheduling_tests.rs"]
mod scheduling_tests;

#[cfg(test)]
#[path = "sqlite/vector_tests.rs"]
mod vector_tests;

#[cfg(all(test, feature = "ui"))]
#[path = "sqlite/listing_tests.rs"]
mod listing_tests;
