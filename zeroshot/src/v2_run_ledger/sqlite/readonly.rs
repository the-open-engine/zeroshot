//! A hot rollback journal is recovered only in a private, consistent image of retained history.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use rusqlite::{Connection, OpenFlags, ffi};

use super::{LedgerConnection, RunLedgerError, available_space, sqlite_error, storage_context};
use crate::execution::platform::{self, FileAccess};

#[path = "readonly/lock.rs"]
mod lock;
use lock::SharedReadLock;

pub(super) fn open(path: &Path) -> Result<LedgerConnection, RunLedgerError> {
    let mut connection = open_connection(path)?;
    match read_schema(&connection) {
        Ok(()) => Ok(LedgerConnection {
            connection,
            path: std::path::absolute(path).ok(),
            _snapshot: None,
        }),
        Err(rusqlite::Error::SqliteFailure(code, _))
            if code.extended_code == ffi::SQLITE_READONLY_ROLLBACK =>
        {
            recover(&mut connection, path)
        }
        Err(error) => Err(storage_context(
            sqlite_error(error),
            Some(&connection),
            || available_space(Some(path)),
        )),
    }
}

fn open_connection(path: &Path) -> Result<Connection, RunLedgerError> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| storage_context(sqlite_error(error), None, || available_space(Some(path))))
}

fn read_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.query_row("PRAGMA schema_version", [], |_| Ok(()))
}

fn recover(source: &mut Connection, path: &Path) -> Result<LedgerConnection, RunLedgerError> {
    let directory = tempfile::Builder::new()
        .prefix("zeroshot-ledger-read-")
        .tempdir()
        .map_err(|_| RunLedgerError::Storage)?;
    // Create private state with explicit caller ownership, including elevated Windows tokens.
    let private = directory.path().join("history");
    platform::create_private_directory(&private).map_err(|_| RunLedgerError::Storage)?;
    let image = private.join("ledger.sqlite3");
    // SQLite sidecars append to the complete filename, including its extension.
    let journal_path = std::path::PathBuf::from(format_sidecar(path, "-journal"));
    let mut journal =
        platform::open_readonly_file(&journal_path).map_err(|_| RunLedgerError::Storage)?;
    let metadata = journal.metadata().map_err(|_| RunLedgerError::Storage)?;
    let identity = platform::file_identity(&journal).map_err(|_| RunLedgerError::Storage)?;
    let source_error = |error| storage_context(error, None, || available_space(Some(path)));
    let guard = SharedReadLock::acquire(source).map_err(source_error)?;
    guard.require_no_writer().map_err(source_error)?;
    require_single_database_journal(&mut journal)?;
    // Rollback recovery is the sole fallback. Never silently omit WAL history from an image.
    match std::fs::symlink_metadata(std::path::PathBuf::from(format_sidecar(path, "-wal"))) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err(RunLedgerError::Storage),
    }
    {
        let mut database = private_file(&image)?;
        guard.copy_database(&mut database).map_err(source_error)?;
        let mut copy = private_file(&private.join("ledger.sqlite3-journal"))?;
        let copied = std::io::copy(
            &mut Read::by_ref(&mut journal).take(metadata.len()),
            &mut copy,
        )
        .map_err(|_| RunLedgerError::Storage)?;
        if copied != metadata.len() {
            return Err(RunLedgerError::Storage);
        }
    }
    guard.require_no_writer().map_err(source_error)?;
    let current = journal.metadata().map_err(|_| RunLedgerError::Storage)?;
    if current.len() != metadata.len()
        || current.modified().ok() != metadata.modified().ok()
        || !platform::open_identity(&journal_path)
            .and_then(|file| platform::file_identity(&file))
            .is_ok_and(|current| current == identity)
    {
        return Err(RunLedgerError::Storage);
    }
    // No schema initialization or run recovery is performed here. SQLite rolls back only the
    // private files; the retained database and journal remain under the shared read lock.
    let connection = open_image(&image)?;
    drop(guard);
    Ok(LedgerConnection {
        connection,
        path: Some(image),
        _snapshot: Some(directory),
    })
}

fn open_image(image: &Path) -> Result<Connection, RunLedgerError> {
    // Measure image failures on their own volume, before the temporary directory is removed.
    let recovered = Connection::open(image).map_err(|error| {
        storage_context(sqlite_error(error), None, || available_space(Some(image)))
    })?;
    read_schema(&recovered).map_err(|error| {
        storage_context(sqlite_error(error), Some(&recovered), || {
            available_space(Some(image))
        })
    })?;
    drop(recovered);
    let connection = open_connection(image)?;
    read_schema(&connection).map_err(|error| {
        storage_context(sqlite_error(error), Some(&connection), || {
            available_space(Some(image))
        })
    })?;
    Ok(connection)
}

fn format_sidecar(path: &Path, suffix: &str) -> std::ffi::OsString {
    let mut result = path.as_os_str().to_owned();
    result.push(suffix);
    result
}

fn private_file(path: &Path) -> Result<File, RunLedgerError> {
    platform::private_file(path, FileAccess::CreateNew).map_err(|_| RunLedgerError::Storage)
}

fn require_single_database_journal(file: &mut File) -> Result<(), RunLedgerError> {
    // This ledger never ATTACHes databases. A super-journal trailer could otherwise make
    // SQLite follow an external path while recovering the copy. Decline that foreign format.
    if file.metadata().map_err(|_| RunLedgerError::Storage)?.len() >= 8 {
        let mut tail = [0u8; 8];
        file.seek(SeekFrom::End(-8))
            .map_err(|_| RunLedgerError::Storage)?;
        file.read_exact(&mut tail)
            .map_err(|_| RunLedgerError::Storage)?;
        if tail == [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7] {
            return Err(RunLedgerError::Storage);
        }
    }
    file.rewind().map_err(|_| RunLedgerError::Storage)?;
    Ok(())
}

#[cfg(test)]
#[path = "readonly/tests.rs"]
mod tests;
