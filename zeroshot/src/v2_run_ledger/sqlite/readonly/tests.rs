use super::*;
use crate::v2_run_ledger::sqlite::SqliteRunLedger;
use crate::v2_run_ledger::sqlite_crash;

fn seed(path: &Path) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE committed(value TEXT);
        INSERT INTO committed VALUES ('before crash');",
        )
        .unwrap();
}

#[tokio::test]
async fn interrupted_journal_is_read_from_a_private_read_only_image() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runs.sqlite3");
    seed(&path);
    sqlite_crash::interrupt(&path);
    let journal = std::path::PathBuf::from(format_sidecar(&path, "-journal"));
    let files = [&path, &journal];
    let before = files.map(|path| std::fs::read(path).unwrap());
    let ledger = SqliteRunLedger::open_read_only(&path).unwrap();
    let image = ledger
        .connection
        .lock()
        .await
        ._snapshot
        .as_ref()
        .unwrap()
        .path()
        .to_owned();
    let cloned = ledger.clone();
    drop(ledger);
    let value = cloned
        .with_connection(|connection| {
            connection
                .query_row("SELECT value FROM committed", [], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(sqlite_error)
        })
        .await
        .unwrap();
    assert_eq!(value, "before crash");
    cloned
        .with_connection(|connection| {
            let integrity: String = connection
                .query_row("PRAGMA integrity_check", [], |row| row.get(0))
                .map_err(sqlite_error)?;
            assert_eq!(integrity, "ok");
            assert!(connection.prepare("SELECT * FROM interrupted").is_err());
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        cloned
            .with_connection(|connection| {
                connection
                    .execute("INSERT INTO committed VALUES ('write')", [])
                    .map_err(sqlite_error)
            })
            .await
            .is_err()
    );
    assert!(image.exists());
    drop(cloned);
    assert!(
        !image.exists(),
        "the image lives exactly as long as its connection"
    );
    assert_eq!(before, files.map(|path| std::fs::read(path).unwrap()));
}

#[tokio::test]
async fn cancelled_read_keeps_the_private_image_until_sqlite_finishes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ledger.sqlite3");
    seed(&path);
    sqlite_crash::interrupt(&path);
    let ledger = SqliteRunLedger::open_read_only(&path).unwrap();
    let image = ledger
        .connection
        .lock()
        .await
        ._snapshot
        .as_ref()
        .unwrap()
        .path()
        .to_owned();
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (finished, done) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn(async move {
        ledger
            .with_connection(move |connection| {
                started.send(()).unwrap();
                wait.recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                let value: String = connection
                    .query_row("SELECT value FROM committed", [], |row| row.get(0))
                    .map_err(sqlite_error)?;
                finished.send(value).unwrap();
                Ok(())
            })
            .await
    });
    ready.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(image.exists());
    release.send(()).unwrap();
    assert_eq!(done.await.unwrap(), "before crash");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while image.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn interrupted_images_reject_wal_and_foreign_super_journals() {
    use std::io::Write;

    for wal in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ledger.sqlite3");
        seed(&path);
        sqlite_crash::interrupt(&path);
        let journal = std::path::PathBuf::from(format_sidecar(&path, "-journal"));
        if wal {
            std::fs::write(format_sidecar(&path, "-wal"), b"foreign WAL history").unwrap();
        } else {
            File::options()
                .append(true)
                .open(&journal)
                .unwrap()
                .write_all(&[0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7])
                .unwrap();
        }
        let before = [&path, &journal].map(|file| std::fs::read(file).unwrap());
        assert!(matches!(
            SqliteRunLedger::open_read_only(&path),
            Err(RunLedgerError::Storage)
        ));
        assert_eq!(
            before,
            [&path, &journal].map(|file| std::fs::read(file).unwrap())
        );
    }
}

#[test]
fn native_shared_lock_blocks_database_writes_and_detects_reserved_writers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ledger.sqlite3");
    seed(&path);
    let mut reader = open_connection(&path).unwrap();
    read_schema(&reader).unwrap();
    let writer = Connection::open(&path).unwrap();
    writer.busy_timeout(std::time::Duration::ZERO).unwrap();
    let guard = SharedReadLock::acquire(&mut reader).unwrap();
    guard.require_no_writer().unwrap();
    assert!(writer.execute_batch("BEGIN EXCLUSIVE").is_err());
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(guard.require_no_writer().is_err());
    writer.execute_batch("ROLLBACK").unwrap();
    drop(guard);
    writer.execute_batch("BEGIN EXCLUSIVE; ROLLBACK").unwrap();
}
