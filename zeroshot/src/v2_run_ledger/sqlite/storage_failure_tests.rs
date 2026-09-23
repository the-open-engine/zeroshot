use super::*;
use openengine_cluster_testkit::assertions::AssertValue;

#[test]
fn a_full_sqlite_database_retains_its_machine_error() {
    let connection = Connection::open_in_memory().assert_value();
    connection
        .pragma_update(None, "max_page_count", 1)
        .assert_value();
    let result = SqliteRunLedger::from_connection(connection);
    let Err(RunLedgerError::SqliteStorage(code)) = result else {
        panic!("expected SQLite capacity failure");
    };
    assert_eq!(code.extended_code, rusqlite::ffi::SQLITE_FULL);
    assert!(
        RunLedgerError::SqliteStorage(code)
            .to_string()
            .contains("database is full")
    );
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
        if code.extended_code == rusqlite::ffi::SQLITE_IOERR_WRITE));
}
