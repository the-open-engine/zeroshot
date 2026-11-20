use super::*;
use rusqlite::ffi;
use std::ffi::{CStr, c_char, c_int, c_uint, c_void};

struct JournalFault {
    path: PathBuf,
    fired: bool,
    in_transaction: bool,
    error: Option<std::io::Error>,
}

unsafe extern "C" fn fail_migration(
    _: c_uint,
    context: *mut c_void,
    statement: *mut c_void,
    sql: *mut c_void,
) -> c_int {
    // SAFETY: SQLite supplies the live statement and SQL for SQLITE_TRACE_STMT. The test
    // keeps the boxed context alive until its connection closes. Never unwind across FFI.
    unsafe {
        if CStr::from_ptr(sql.cast::<c_char>())
            .to_bytes()
            .starts_with(b"ALTER TABLE v2_run_events ADD COLUMN")
        {
            let fault = &mut *context.cast::<JournalFault>();
            fault.in_transaction =
                ffi::sqlite3_get_autocommit(ffi::sqlite3_db_handle(statement.cast())) == 0;
            fault.error = std::fs::create_dir(&fault.path).err();
            fault.fired = true;
        }
    }
    0
}

fn legacy_connection(path: &Path) -> Connection {
    let connection = Connection::open(path).assert_value();
    connection.execute_batch(SCHEMA).assert_value();
    connection
        .execute_batch("ALTER TABLE v2_run_events DROP COLUMN recorded_at_ms")
        .assert_value();
    connection
}

fn install_fault(connection: &Connection, path: &Path) -> Box<JournalFault> {
    let mut fault = Box::new(JournalFault {
        path: path.with_extension("sqlite3-journal"),
        fired: false,
        in_transaction: false,
        error: None,
    });
    // SAFETY: the connection is exclusively owned. The returned box stays allocated until
    // after the connection closes; only this connection's synchronous trace accesses it.
    let result = unsafe {
        ffi::sqlite3_trace_v2(
            connection.handle(),
            ffi::SQLITE_TRACE_STMT,
            Some(fail_migration),
            (&mut *fault as *mut JournalFault).cast(),
        )
    };
    assert_eq!(result, ffi::SQLITE_OK);
    fault
}

#[test]
fn migration_failure_after_begin_retains_native_cause_before_rollback() {
    let root = tempfile::tempdir().assert_value();
    let mut control = legacy_connection(&root.path().join("control.sqlite3"));
    let control_fault = install_fault(&control, &root.path().join("control.sqlite3"));
    let transaction = control
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .assert_value();
    let original = transaction
        .execute_batch("ALTER TABLE v2_run_events ADD COLUMN recorded_at_ms INTEGER")
        .expect_err("fault is installed only after BEGIN succeeds");
    let rusqlite::Error::SqliteFailure(code, _) = original else {
        panic!("expected SQLite error");
    };
    // SAFETY: this test exclusively owns the live transaction and connection.
    let native = unsafe { ffi::sqlite3_system_errno(transaction.handle()) };
    drop(transaction);
    drop(control);
    assert!(control_fault.fired && control_fault.in_transaction);
    assert!(control_fault.error.is_none());

    let path = root.path().join("migration.sqlite3");
    let connection = legacy_connection(&path);
    let fault = install_fault(&connection, &path);
    let error = match SqliteRunLedger::from_connection(connection, Some(path.clone())) {
        Err(error) => error,
        Ok(_) => panic!("migration must fail"),
    };
    assert!(fault.fired && fault.in_transaction);
    assert!(fault.error.is_none());
    let RunLedgerError::SqliteStorage(failure) = &error else {
        panic!("expected SQLite error");
    };
    assert_eq!(failure.code, code);
    if code.extended_code == ffi::SQLITE_CANTOPEN_ISDIR {
        assert_eq!(failure.system_error, None);
    } else {
        assert_ne!(native, 0, "control has a native cause before rollback");
        assert_eq!(failure.system_error, Some(native));
    }
    assert!(failure.available_bytes.is_some_and(|bytes| bytes > 0));
    let message = error.to_string();
    assert!(!message.contains("migration.sqlite3"));
    assert!(!message.contains("ALTER TABLE"));
    std::fs::remove_dir(&fault.path).assert_value();
    let ledger = SqliteRunLedger::open(&path).assert_value();
    assert!(ledger.recorded_at, "rollback left migration retryable");
}
