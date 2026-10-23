use std::collections::BTreeSet;
use std::fs;
#[cfg(unix)]
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};

use super::{
    ActiveMemoryError, Lesson, LessonMutation, LessonScope, LessonVersion, MAX_LESSONS,
    MAX_RECORD_BYTES, MAX_STORE_BYTES, MAX_TOMBSTONES, MEMORY_SCHEMA_VERSION, MemorySnapshot,
    ScopeQuery,
};

/// A single synchronous connection. Owners must serialize access before using
/// `spawn_blocking`; independent processes are serialized by SQLite's writer lock.
pub struct ActiveMemoryStore {
    connection: Connection,
}

/// Acquire `lock_owned` before transferring the guard to `spawn_blocking`, so
/// cancellation cannot release a connection while its SQLite work is in flight.
pub type SharedActiveMemoryStore = std::sync::Arc<tokio::sync::Mutex<ActiveMemoryStore>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredLesson {
    pub version: LessonVersion,
    /// A missing lesson is a retained deletion tombstone, not an unused ID.
    pub lesson: Option<Lesson>,
}

impl ActiveMemoryStore {
    /// The host chooses a dedicated private directory outside every workspace.
    /// Workers must never receive this path as a writable capability.
    pub fn open(path: &Path) -> Result<Self, ActiveMemoryError> {
        prepare_private_path(path)?;
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let mut connection = Connection::open_with_flags(path, flags)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA trusted_schema=OFF;
             PRAGMA foreign_keys=ON;
             PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;",
        )?;
        initialize(&mut connection)?;
        check_capacity(&connection)?;
        check_sidecars(path)?;
        Ok(Self { connection })
    }

    /// All mutations succeed together or none do. Deleted IDs retain their
    /// revision, preventing stale runs from recreating a retired lesson.
    pub fn apply_batch(
        &mut self,
        mutations: &[LessonMutation],
    ) -> Result<Vec<LessonVersion>, ActiveMemoryError> {
        self.apply_reviewed_batch(mutations, &[])
    }

    pub(crate) fn apply_reviewed_batch(
        &mut self,
        mutations: &[LessonMutation],
        expected: &[(String, Option<u64>)],
    ) -> Result<Vec<LessonVersion>, ActiveMemoryError> {
        if mutations.is_empty() && expected.is_empty() {
            return Ok(Vec::new());
        }
        self.apply_checked_batch(mutations, expected, None)
    }

    pub(crate) fn commit_initialization(
        &mut self,
        id: &str,
        mutations: &[LessonMutation],
        expected: &[(String, Option<u64>)],
    ) -> Result<Vec<LessonVersion>, ActiveMemoryError> {
        self.apply_checked_batch(mutations, expected, Some(id))
    }

    fn apply_checked_batch(
        &mut self,
        mutations: &[LessonMutation],
        expected: &[(String, Option<u64>)],
        receipt: Option<&str>,
    ) -> Result<Vec<LessonVersion>, ActiveMemoryError> {
        validate_mutations(mutations)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(versions) = read_initialization_receipt(&transaction, receipt)? {
            return Ok(versions);
        }
        check_expectations(&transaction, expected)?;
        let versions = write_mutations(&transaction, mutations)?;
        save_initialization_receipt(&transaction, receipt, &versions)?;
        check_capacity(&transaction)?;
        transaction.commit()?;
        Ok(versions)
    }

    pub(crate) fn initialization_receipt(
        &self,
        id: &str,
    ) -> Result<Option<Vec<LessonVersion>>, ActiveMemoryError> {
        read_initialization_receipt(&self.connection, Some(id))
    }

    pub(crate) fn finish_initialization(&self, id: &str) -> Result<(), ActiveMemoryError> {
        self.connection.execute(
            "DELETE FROM active_memory_initializations WHERE id=?1",
            [id],
        )?;
        Ok(())
    }

    /// Captures one consistent, immutable generation. Graph lessons may omit a
    /// repository association, but repository lessons never cross repository IDs.
    pub fn snapshot(&self, query: &ScopeQuery) -> Result<MemorySnapshot, ActiveMemoryError> {
        let transaction = self.connection.unchecked_transaction()?;
        let generation: u64 = transaction.query_row(
            "SELECT generation FROM active_memory_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let lessons = read_scoped_lessons(&transaction, query)?;
        transaction.commit()?;
        Ok(MemorySnapshot {
            generation,
            lessons,
        })
    }

    pub fn record(&self, id: &str) -> Result<Option<StoredLesson>, ActiveMemoryError> {
        super::model::validate_key("lesson id", id, 96)?;
        let row: Option<(u64, Option<String>)> = self
            .connection
            .query_row(
                "SELECT revision, body FROM active_memory_records WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(revision, body)| {
            Ok(StoredLesson {
                version: LessonVersion {
                    id: id.to_owned(),
                    revision,
                },
                lesson: body.as_deref().map(decode_lesson).transpose()?,
            })
        })
        .transpose()
    }
}

fn advance_generation(connection: &Connection) -> Result<(), ActiveMemoryError> {
    let generation: i64 = connection.query_row(
        "SELECT generation FROM active_memory_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let next = generation
        .checked_add(1)
        .ok_or(ActiveMemoryError::Capacity)?;
    connection.execute(
        "UPDATE active_memory_meta SET generation=?1 WHERE singleton=1",
        [next],
    )?;
    Ok(())
}

fn read_scoped_lessons(
    connection: &Connection,
    query: &ScopeQuery,
) -> Result<Vec<Lesson>, ActiveMemoryError> {
    let mut statement = connection.prepare(
        "SELECT body FROM active_memory_records WHERE deleted=0 AND (
           (?4=1 AND kind='repository' AND repository=?1)
           OR (?5=1 AND kind='graph' AND graph_hash=?2
             AND (?3 IS NULL OR node_id=?3)
             AND (repository IS NULL OR repository=?1))) ORDER BY id LIMIT ?6",
    )?;
    let mut rows = statement.query(params![
        query.repository,
        query.graph_hash,
        query.node_id,
        query.repository_lessons,
        query.graph_lessons,
        MAX_LESSONS + 1
    ])?;
    let mut lessons = Vec::new();
    while let Some(row) = rows.next()? {
        let body = row
            .get_ref(0)?
            .as_str()
            .map_err(|_| ActiveMemoryError::Invalid("non-text stored lesson".into()))?;
        let lesson = decode_lesson(body)?;
        if !lesson.scope.matches(query) {
            return Err(ActiveMemoryError::Invalid("stored scope mismatch".into()));
        }
        lessons.push(lesson);
    }
    if lessons.len() > MAX_LESSONS {
        return Err(ActiveMemoryError::Capacity);
    }
    Ok(lessons)
}

fn initialize(connection: &mut Connection) -> Result<(), ActiveMemoryError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: u32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != 0 && version != MEMORY_SCHEMA_VERSION {
        return Err(ActiveMemoryError::Invalid(
            "unsupported database schema".into(),
        ));
    }
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS active_memory_meta (
           singleton INTEGER PRIMARY KEY CHECK(singleton=1),
           generation INTEGER NOT NULL CHECK(generation>=0));
         INSERT OR IGNORE INTO active_memory_meta(singleton,generation) VALUES(1,0);
         CREATE TABLE IF NOT EXISTS active_memory_initializations (id TEXT PRIMARY KEY NOT NULL, receipt TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS active_memory_records (
           id TEXT PRIMARY KEY NOT NULL,
           revision INTEGER NOT NULL CHECK(revision>0),
           deleted INTEGER NOT NULL CHECK(deleted IN (0,1)),
           kind TEXT NOT NULL CHECK(kind IN ('repository','graph')),
           repository TEXT,
           graph_hash TEXT,
           node_id TEXT,
           body TEXT,
           CHECK((deleted=0 AND body IS NOT NULL) OR (deleted=1 AND body IS NULL)));
         CREATE INDEX IF NOT EXISTS active_memory_scope
           ON active_memory_records(deleted,kind,repository,graph_hash,node_id);",
    )?;
    transaction.pragma_update(None, "user_version", MEMORY_SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn validate_mutations(mutations: &[LessonMutation]) -> Result<(), ActiveMemoryError> {
    if serde_json::to_vec(mutations)?.len() > MAX_STORE_BYTES {
        return Err(ActiveMemoryError::Capacity);
    }
    let mut ids = BTreeSet::new();
    for mutation in mutations {
        let id = validate_mutation(mutation)?;
        if !ids.insert(id) {
            return Err(ActiveMemoryError::Invalid("duplicate mutation id".into()));
        }
    }
    Ok(())
}

fn validate_mutation(mutation: &LessonMutation) -> Result<&str, ActiveMemoryError> {
    match mutation {
        LessonMutation::Upsert {
            draft,
            expected_revision,
        } => {
            draft.validate()?;
            validate_expected_revision(*expected_revision)?;
            Ok(&draft.id)
        }
        LessonMutation::Delete {
            id,
            expected_revision,
        } => {
            super::model::validate_key("lesson id", id, 96)?;
            validate_expected_revision(Some(*expected_revision))?;
            Ok(id)
        }
    }
}

fn validate_expected_revision(revision: Option<u64>) -> Result<(), ActiveMemoryError> {
    if revision.is_some_and(|revision| revision == 0 || revision >= i64::MAX as u64) {
        return Err(ActiveMemoryError::Invalid("expected revision".into()));
    }
    Ok(())
}

fn write_mutations(
    transaction: &rusqlite::Transaction<'_>,
    mutations: &[LessonMutation],
) -> Result<Vec<LessonVersion>, ActiveMemoryError> {
    let mut versions = Vec::with_capacity(mutations.len());
    for mutation in mutations {
        versions.push(apply_mutation(transaction, mutation)?);
    }
    check_capacity(transaction)?;
    if !mutations.is_empty() {
        advance_generation(transaction)?;
    }
    Ok(versions)
}

fn read_initialization_receipt(
    connection: &Connection,
    id: Option<&str>,
) -> Result<Option<Vec<LessonVersion>>, ActiveMemoryError> {
    let Some(id) = id else {
        return Ok(None);
    };
    super::model::validate_key("initialization receipt", id, 96)?;
    let body: Option<String> = connection
        .query_row(
            "SELECT receipt FROM active_memory_initializations WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    body.map(|body| serde_json::from_str(&body).map_err(Into::into))
        .transpose()
}

fn save_initialization_receipt(
    connection: &Connection,
    id: Option<&str>,
    versions: &[LessonVersion],
) -> Result<(), ActiveMemoryError> {
    if let Some(id) = id {
        connection.execute(
            "INSERT INTO active_memory_initializations(id,receipt) VALUES(?1,?2)",
            params![id, serde_json::to_string(versions)?],
        )?;
    }
    Ok(())
}

fn check_expectations(
    transaction: &rusqlite::Transaction<'_>,
    expected: &[(String, Option<u64>)],
) -> Result<(), ActiveMemoryError> {
    for (id, revision) in expected {
        check_revision(transaction, id, *revision)?;
    }
    Ok(())
}

fn check_revision(
    transaction: &rusqlite::Transaction<'_>,
    id: &str,
    expected: Option<u64>,
) -> Result<Option<u64>, ActiveMemoryError> {
    let actual: Option<u64> = transaction
        .query_row(
            "SELECT revision FROM active_memory_records WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    if expected != actual {
        return Err(ActiveMemoryError::RevisionConflict {
            id: id.to_owned(),
            expected,
            actual,
        });
    }
    Ok(actual)
}

fn apply_mutation(
    transaction: &rusqlite::Transaction<'_>,
    mutation: &LessonMutation,
) -> Result<LessonVersion, ActiveMemoryError> {
    let (id, expected) = match mutation {
        LessonMutation::Upsert {
            draft,
            expected_revision,
        } => (&draft.id, *expected_revision),
        LessonMutation::Delete {
            id,
            expected_revision,
        } => (id, Some(*expected_revision)),
    };
    let actual = check_revision(transaction, id, expected)?;
    let revision = actual
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(ActiveMemoryError::Capacity)?;
    match mutation {
        LessonMutation::Upsert { draft, .. } => {
            upsert_lesson(transaction, draft.clone().into_lesson(revision))?;
        }
        LessonMutation::Delete { .. } => {
            transaction.execute(
                "UPDATE active_memory_records SET revision=?2,deleted=1,body=NULL WHERE id=?1",
                params![id, revision],
            )?;
        }
    }
    Ok(LessonVersion {
        id: id.clone(),
        revision,
    })
}

fn upsert_lesson(connection: &Connection, lesson: Lesson) -> Result<(), ActiveMemoryError> {
    lesson.validate()?;
    let body = serde_json::to_string(&lesson)?;
    let (kind, repository, graph_hash, node_id) = scope_columns(&lesson.scope);
    connection.execute(
        "INSERT INTO active_memory_records(id,revision,deleted,kind,repository,graph_hash,node_id,body)
         VALUES(?1,?2,0,?3,?4,?5,?6,?7)
         ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,deleted=0,
           kind=excluded.kind,repository=excluded.repository,graph_hash=excluded.graph_hash,
           node_id=excluded.node_id,body=excluded.body",
        params![lesson.id, lesson.revision, kind, repository, graph_hash, node_id, body],
    )?;
    Ok(())
}

fn scope_columns(scope: &LessonScope) -> (&str, Option<&str>, Option<&str>, Option<&str>) {
    match scope {
        LessonScope::Repository { repository, .. } => ("repository", Some(repository), None, None),
        LessonScope::Graph {
            graph_hash,
            node_id,
            repository,
        } => (
            "graph",
            repository.as_deref(),
            Some(graph_hash),
            Some(node_id),
        ),
    }
}

fn decode_lesson(body: &str) -> Result<Lesson, ActiveMemoryError> {
    if body.len() > MAX_RECORD_BYTES {
        return Err(ActiveMemoryError::Capacity);
    }
    let lesson: Lesson = serde_json::from_str(body)?;
    lesson.validate()?;
    Ok(lesson)
}

fn check_capacity(connection: &Connection) -> Result<(), ActiveMemoryError> {
    let (live, retired, bytes, largest): (usize, usize, usize, usize) = connection.query_row(
        "SELECT COALESCE(SUM(deleted=0),0),COALESCE(SUM(deleted=1),0),
          COALESCE(SUM(length(CAST(COALESCE(body,'') AS BLOB)) + length(CAST(id AS BLOB))
            + length(CAST(COALESCE(repository,'') AS BLOB))
            + length(CAST(COALESCE(graph_hash,'') AS BLOB))
            + length(CAST(COALESCE(node_id,'') AS BLOB))),0),
          COALESCE(MAX(length(CAST(body AS BLOB))),0) FROM active_memory_records",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    if live > MAX_LESSONS
        || retired > MAX_TOMBSTONES
        || bytes.saturating_add(initialization_bytes(connection)?) > MAX_STORE_BYTES
        || largest > MAX_RECORD_BYTES
    {
        return Err(ActiveMemoryError::Capacity);
    }
    Ok(())
}

fn prepare_private_path(path: &Path) -> Result<(), ActiveMemoryError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(ActiveMemoryError::UnsafeStorage);
    }
    let parent = path.parent().ok_or(ActiveMemoryError::UnsafeStorage)?;
    reject_symlink_ancestors(parent)?;
    create_private_directory(parent)?;
    reject_symlink_ancestors(parent)?;
    check_private(&fs::symlink_metadata(parent)?, true)?;
    create_private_file(path)?;
    check_sidecars(path)
}

#[cfg(unix)]
fn create_private_directory(parent: &Path) -> Result<(), ActiveMemoryError> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    Ok(())
}

#[cfg(windows)]
fn create_private_directory(parent: &Path) -> Result<(), ActiveMemoryError> {
    crate::execution::platform::private_directory(parent)?;
    Ok(())
}

#[cfg(windows)]
fn create_private_file(path: &Path) -> Result<(), ActiveMemoryError> {
    let file = crate::execution::platform::private_file(
        path,
        crate::execution::platform::FileAccess::ReadWrite,
    )?;
    check_private(&file.metadata()?, false)
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> Result<(), ActiveMemoryError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    match options.open(path) {
        Ok(file) => check_private(&file.metadata()?, false)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            check_private(&fs::symlink_metadata(path)?, false)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn check_sidecars(path: &Path) -> Result<(), ActiveMemoryError> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        if let Some(metadata) = optional_metadata(&sidecar)? {
            check_private(&metadata, false)?;
            #[cfg(windows)]
            crate::execution::platform::private_inherited_file(&sidecar)
                .map_err(|_| ActiveMemoryError::UnsafeStorage)?;
        }
    }
    Ok(())
}

fn reject_symlink_ancestors(path: &Path) -> Result<(), ActiveMemoryError> {
    for ancestor in path.ancestors() {
        if optional_metadata(ancestor)?.is_some_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(ActiveMemoryError::UnsafeStorage);
        }
    }
    Ok(())
}

fn optional_metadata(path: &Path) -> Result<Option<fs::Metadata>, ActiveMemoryError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn check_private(metadata: &fs::Metadata, directory: bool) -> Result<(), ActiveMemoryError> {
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(ActiveMemoryError::UnsafeStorage);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Same-UID native harnesses share the user's trust boundary. Contained
        // workers have separate UIDs and cannot traverse this private directory.
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || (!directory && metadata.nlink() != 1)
        {
            return Err(ActiveMemoryError::UnsafeStorage);
        }
    }
    Ok(())
}

fn initialization_bytes(connection: &Connection) -> Result<usize, ActiveMemoryError> {
    Ok(connection.query_row(
        "SELECT COALESCE(SUM(length(CAST(receipt AS BLOB))),0) FROM active_memory_initializations",
        [],
        |row| row.get(0),
    )?)
}
