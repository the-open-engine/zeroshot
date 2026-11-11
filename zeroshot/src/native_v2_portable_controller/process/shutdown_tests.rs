//! A terminal controller retires idle observation connections before its runtime exits.

use super::*;
use crate::native_v2_portable_controller::ControllerLease;
use openengine_cluster_protocol::{IdempotencyKey, Sha256Digest, TerminalResult};
use openengine_cluster_testkit::assertions::AssertValue;
use tokio::io::AsyncReadExt;
use crate::v2_run_ledger::{CreateRun, RunEvent, RunLedger};
use crate::v2_run_ledger::sqlite::SqliteRunLedger;

async fn terminal_controller(root: &Path) -> Arc<PortableRunController> {
    let paths = PortableControllerPaths::new(root.join("state"));
    crate::execution::platform::create_private_directory(paths.storage()).assert_value();
    let run_id = RunId::new("terminal-connection-retirement");
    let ledger = SqliteRunLedger::open(paths.ledger()).assert_value();
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: IdempotencyKey::new("terminal-connection-retirement").assert_value(),
            submission_digest: Sha256Digest::new("c".repeat(64)).assert_value(),
            admitted: crate::native_v2_runner::test_support::admitted(),
        })
        .await
        .assert_value();
    ledger
        .append(
            &run_id,
            vec![RunEvent::Terminal {
                result: TerminalResult::Succeeded {
                    output: serde_json::Value::Null,
                },
            }],
        )
        .await
        .assert_value();
    drop(ledger);

    Arc::new(
        PortableRunController::open_observer(paths.clone(), run_id)
            .await
            .assert_value(),
    )
}

#[tokio::test]
async fn terminal_server_closes_idle_connections_and_releases_controller_ownership() {
    let root = tempfile::tempdir().assert_value();
    let controller = terminal_controller(root.path()).await;
    let paths = controller.paths().clone();
    let server = controller.bind().await.assert_value();
    let (_stop, stopping) = tokio::sync::watch::channel(false);
    let serving = tokio::spawn(server.serve_until_terminal(stopping));
    let mut observer = super::super::transport::connect(&paths.socket())
        .await
        .assert_value();
    tokio::time::timeout(Duration::from_secs(5), serving)
        .await
        .assert_value_with("an idle observer must not keep a terminal controller serving")
        .assert_value()
        .assert_value();

    let _reclaimed = ControllerLease::acquire(paths.lease()).assert_value_with(
        "serving returned while an accepted connection still owned the controller lease",
    );
    let mut byte = [0];
    let read = tokio::time::timeout(Duration::from_secs(5), observer.read(&mut byte))
        .await
        .assert_value_with("the accepted idle connection must close before serving returns");
    assert!(
        matches!(read, Ok(0))
            || read.as_ref().is_err_and(|error| matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
            )),
        "terminal controller left its idle observation connection open: {read:?}"
    );
}

#[tokio::test]
async fn shutdown_keeps_a_durable_terminal_and_does_not_wait_for_a_first_connection() {
    let root = tempfile::tempdir().assert_value();
    let controller = terminal_controller(root.path()).await;
    let paths = controller.paths().clone();
    let run_id = controller.run_id().clone();
    let ledger = SqliteRunLedger::open_read_only(paths.ledger()).assert_value();
    let before = ledger.get(&run_id).await.assert_value().assert_value();
    let server = controller.bind().await.assert_value();
    let (_stop, stopping) = tokio::sync::watch::channel(true);
    tokio::time::timeout(
        Duration::from_secs(2),
        server.serve_until_terminal(stopping),
    )
    .await
    .assert_value_with("shutdown must not wait for the submitting CLI to connect")
    .assert_value();
    let after = ledger.get(&run_id).await.assert_value().assert_value();
    assert_eq!(after.snapshot, before.snapshot);
    let _reclaimed = ControllerLease::acquire(paths.lease()).assert_value();
}
