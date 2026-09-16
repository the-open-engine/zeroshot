use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use openengine_cluster_testkit::assertions::AssertValue;
use tokio::sync::oneshot;

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn waiting_for_sqlite_leaves_async_tasks_schedulable() {
    let ledger = SqliteRunLedger::open_in_memory().assert_value();
    let connection = ledger.connection.clone().lock_owned().await;
    let run_id = RunId::new("pending-status");
    let read = ledger.get(&run_id);
    tokio::pin!(read);

    assert!(futures_util::poll!(read.as_mut()).is_pending());
    tokio::time::sleep(Duration::from_millis(1)).await;
    drop(connection);

    assert!(read.await.assert_value().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn blocking_sqlite_work_leaves_async_tasks_schedulable() {
    let ledger = SqliteRunLedger::open_in_memory().assert_value();
    let (started, ready) = oneshot::channel();
    let (release, wait) = mpsc::channel();
    let operation = tokio::spawn(async move {
        ledger
            .with_connection(move |_| {
                started.send(()).assert_value();
                wait.recv_timeout(Duration::from_secs(5)).assert_value();
                Ok(())
            })
            .await
    });

    ready.await.assert_value();
    tokio::time::sleep(Duration::from_millis(1)).await;
    release.send(()).assert_value();
    operation.await.assert_value().assert_value();
}

#[tokio::test]
async fn cancelled_connection_waiter_never_executes_its_operation() {
    let ledger = SqliteRunLedger::open_in_memory().assert_value();
    let connection = ledger.connection.clone().lock_owned().await;
    let ran = Arc::new(AtomicBool::new(false));
    let observed = ran.clone();
    {
        let waiting = ledger.with_connection(move |_| {
            observed.store(true, Ordering::Release);
            Ok(())
        });
        tokio::pin!(waiting);
        assert!(futures_util::poll!(waiting.as_mut()).is_pending());
    }
    drop(connection);

    ledger.list().await.assert_value();
    assert!(!ran.load(Ordering::Acquire));
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_caller_keeps_started_transaction_serialized_until_commit() {
    let ledger = SqliteRunLedger::open_in_memory().assert_value();
    let first = ledger.clone();
    let (started, ready) = oneshot::channel();
    let (release, wait) = mpsc::channel();
    let caller = tokio::spawn(async move {
        first
            .with_connection(move |connection| {
                let transaction = connection.transaction().map_err(sqlite_error)?;
                transaction
                    .execute_batch(
                        "CREATE TABLE marker (value INTEGER); INSERT INTO marker VALUES (1);",
                    )
                    .map_err(sqlite_error)?;
                started.send(()).assert_value();
                wait.recv_timeout(Duration::from_secs(5)).assert_value();
                transaction.commit().map_err(sqlite_error)
            })
            .await
    });
    ready.await.assert_value();
    caller.abort();
    assert!(
        caller
            .await
            .expect_err("caller was cancelled")
            .is_cancelled()
    );

    let next = ledger.with_connection(|connection| {
        connection
            .query_row("SELECT COUNT(*) FROM marker", [], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(sqlite_error)
    });
    tokio::pin!(next);
    assert!(futures_util::poll!(next.as_mut()).is_pending());
    assert!(ledger.connection.try_lock().is_err());
    release.send(()).assert_value();

    assert_eq!(next.await.assert_value(), 1);
}
