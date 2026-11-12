//! Crash with dirty pages on disk, using the same bundled SQLite as the product.
use std::path::Path;
use std::process::Command;

pub(crate) fn interrupt(path: &Path) {
    // Rust's test names omit the crate prefix from module_path!().
    let (_, module) = module_path!().split_once("::").unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", &format!("{module}::writer")])
        .env("ZEROSHOT_TEST_INTERRUPTED_LEDGER", path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let error = connection
        .query_row("PRAGMA schema_version", [], |_| Ok(()))
        .unwrap_err();
    assert!(
        matches!(error, rusqlite::Error::SqliteFailure(code, _)
        if code.extended_code == rusqlite::ffi::SQLITE_READONLY_ROLLBACK),
        "{error}"
    );
}

#[test]
#[ignore = "owned subprocess crashes before SQLite rollback"]
fn writer() {
    let path = std::env::var_os("ZEROSHOT_TEST_INTERRUPTED_LEDGER").unwrap();
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode=DELETE; PRAGMA cache_size=1;
        BEGIN IMMEDIATE; CREATE TABLE interrupted(value BLOB);
        INSERT INTO interrupted VALUES (zeroblob(65536));",
        )
        .unwrap();
    connection.cache_flush().unwrap();
    std::process::exit(0);
}
