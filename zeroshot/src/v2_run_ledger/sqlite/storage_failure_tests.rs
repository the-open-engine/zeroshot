use super::*;
use openengine_cluster_testkit::assertions::AssertValue;

#[path = "migration_failure_tests.rs"]
mod migration;

fn assert_native_cause(failure: &crate::v2_run_ledger::SqliteStorageError) {
    if failure.code.extended_code == rusqlite::ffi::SQLITE_CANTOPEN_ISDIR {
        assert_eq!(
            failure.system_error, None,
            "SQLite detected a directory without a failed OS call"
        );
    } else {
        #[cfg(unix)]
        assert!(
            failure.system_error.is_some(),
            "native journal I/O cause retained"
        );
    }
}

#[tokio::test]
async fn journal_open_failure_reports_ledger_volume_without_claiming_exhaustion() {
    let root = tempfile::tempdir().assert_value();
    let path = root.path().join("private-ledger.sqlite3");
    let ledger = SqliteRunLedger::open(&path).assert_value();
    // A directory in place of the rollback journal forces a storage failure without filling a disk.
    std::fs::create_dir(root.path().join("private-ledger.sqlite3-journal")).assert_value();
    let error = ledger
        .with_connection(|connection| {
            connection
                .execute_batch("CREATE TABLE injected_failure (value TEXT)")
                .map_err(sqlite_error)
        })
        .await
        .expect_err("journal access fails");
    let message = error.to_string();
    assert!(message.contains("Error code "), "{message}");
    assert!(
        message.contains("Available space on the storage volume:"),
        "{message}"
    );
    assert!(!message.contains("Free space or select a larger run size"));
    assert!(!message.contains("private-ledger"));
    let RunLedgerError::SqliteStorage(failure) = error else {
        panic!("expected SQLite failure");
    };
    assert_native_cause(&failure);
    assert!(failure.available_bytes.is_some_and(|bytes| bytes > 0));
}

#[tokio::test]
async fn transaction_failure_retains_native_cause_before_rollback() {
    let root = tempfile::tempdir().assert_value();
    let path = root.path().join("ledger.sqlite3");
    let ledger = SqliteRunLedger::open(&path).assert_value();
    std::fs::create_dir(root.path().join("ledger.sqlite3-journal")).assert_value();
    let error = ledger
        .with_transaction(|transaction| {
            transaction
                .execute_batch("CREATE TABLE injected_failure (value TEXT)")
                .map_err(sqlite_error)
        })
        .await
        .expect_err("journal access fails");
    let RunLedgerError::SqliteStorage(failure) = error else {
        panic!("expected SQLite failure");
    };
    assert!(matches!(
        failure.code.code,
        rusqlite::ffi::ErrorCode::CannotOpen | rusqlite::ffi::ErrorCode::SystemIoFailure
    ));
    assert_native_cause(&failure);
    assert!(failure.available_bytes.is_some_and(|bytes| bytes > 0));
    // The rollback settled: repairing the journal path allows the same connection to write.
    std::fs::remove_dir(root.path().join("ledger.sqlite3-journal")).assert_value();
    ledger
        .with_transaction(|transaction| {
            transaction
                .execute_batch("CREATE TABLE recovered (value TEXT)")
                .map_err(sqlite_error)
        })
        .await
        .assert_value();
}

#[test]
fn unavailable_space_query_preserves_the_original_failure_and_runs_once() {
    let original = sqlite_error(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
        Some("private database payload".to_owned()),
    ));
    let mut queries = 0;
    let error = storage_context(original.clone(), None, || {
        queries += 1;
        std::io::Result::<u64>::Err(std::io::ErrorKind::PermissionDenied.into()).ok()
    });
    assert_eq!(queries, 1);
    assert_eq!(error, original);
    assert!(!error.to_string().contains("private"));
    assert!(!error.to_string().contains("Available space"));
    assert!(
        !error
            .to_string()
            .contains("Free space or select a larger run size")
    );
}

#[test]
fn open_failure_includes_space_without_exposing_the_path() {
    let root = tempfile::tempdir().assert_value();
    // Opening a directory as SQLite fails even for an elevated test user.
    let error = match SqliteRunLedger::open(root.path()) {
        Err(error) => error,
        Ok(_) => panic!("opening a directory must fail"),
    };
    let message = error.to_string();
    let RunLedgerError::SqliteStorage(failure) = &error else {
        panic!("expected SQLite failure");
    };
    assert_eq!(failure.code.code, rusqlite::ffi::ErrorCode::CannotOpen);
    assert!(
        message.contains(&format!("Error code {}", failure.code.extended_code)),
        "{message}"
    );
    assert!(message.contains("Available space on the storage volume:"));
    assert!(!message.contains(&*root.path().to_string_lossy()));
    assert!(!message.contains("Free space or select a larger run size"));
}

#[test]
fn guidance_requires_exhaustion_evidence() {
    use crate::v2_run_ledger::SqliteStorageError;
    use rusqlite::ffi::{Error, SQLITE_CANTOPEN, SQLITE_FULL};

    #[cfg(unix)]
    let no_space = libc::ENOSPC;
    #[cfg(windows)]
    let no_space = windows_sys::Win32::Foundation::ERROR_DISK_FULL as i32;
    for (code, system_error, available_bytes, guidance) in [
        (SQLITE_CANTOPEN, None, None, false),
        (SQLITE_CANTOPEN, None, Some(12345), false),
        (SQLITE_CANTOPEN, None, Some(0), true),
        (SQLITE_CANTOPEN, Some(no_space), Some(12345), true),
        (SQLITE_FULL, None, Some(12345), true),
        (SQLITE_FULL, None, None, true),
    ] {
        let error = RunLedgerError::SqliteStorage(SqliteStorageError {
            code: Error::new(code),
            system_error,
            available_bytes,
        });
        let message = error.to_string();
        assert_eq!(
            message.contains("Free space or select a larger run size before resuming"),
            guidance,
            "{message}"
        );
        if let Some(bytes) = available_bytes {
            assert!(message.contains(&format!("Available space on the storage volume: {bytes} B")));
        }
        if let Some(code) = system_error {
            assert!(message.contains(&std::io::Error::from_raw_os_error(code).to_string()));
        }
    }
}

#[test]
fn a_full_sqlite_database_retains_its_machine_error() {
    let connection = Connection::open_in_memory().assert_value();
    connection
        .pragma_update(None, "max_page_count", 1)
        .assert_value();
    let result = SqliteRunLedger::from_connection(connection, None);
    let Err(RunLedgerError::SqliteStorage(code)) = result else {
        panic!("expected SQLite capacity failure");
    };
    assert_eq!(code.code.extended_code, rusqlite::ffi::SQLITE_FULL);
    assert_eq!(code.system_error, None);
    assert_eq!(code.available_bytes, None);
    assert!(
        RunLedgerError::SqliteStorage(code)
            .to_string()
            .contains("database is full")
    );
}

#[test]
fn schema_failure_measures_space_on_the_database_volume() {
    let root = tempfile::tempdir().assert_value();
    let path = root.path().join("ledger.sqlite3");
    let connection = Connection::open(&path).assert_value();
    connection
        .pragma_update(None, "max_page_count", 1)
        .assert_value();
    let Err(RunLedgerError::SqliteStorage(failure)) =
        SqliteRunLedger::from_connection(connection, Some(path))
    else {
        panic!("expected SQLite capacity failure");
    };
    assert_eq!(failure.code.extended_code, rusqlite::ffi::SQLITE_FULL);
    assert!(failure.available_bytes.is_some_and(|bytes| bytes > 0));
}

#[test]
fn sqlite_diagnostics_exclude_arbitrary_input_text() {
    let error = sqlite_error(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR_WRITE),
        Some("secret trigger input".to_owned()),
    ));
    let message = error.to_string();
    assert!(message.contains(&rusqlite::ffi::SQLITE_IOERR_WRITE.to_string()));
    assert!(!message.contains("secret"));
    assert!(matches!(error, RunLedgerError::SqliteStorage(code)
        if code.code.extended_code == rusqlite::ffi::SQLITE_IOERR_WRITE));
}
