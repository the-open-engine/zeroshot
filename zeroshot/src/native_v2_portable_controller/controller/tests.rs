use openengine_cluster_protocol::{
    ExecutionRef, GetParams, IdempotencyKey, InitializeParams, NOT_FOUND, PROTOCOL_VERSION,
    RunAttachParams, RunCheckpointsParams, RunForceParams, RunLogsParams, RunStatus,
    RunStatusParams, RunWatchParams, Sha256Digest,
};
use openengine_cluster_testkit::assertions::AssertValue;
use openengine_cluster_server::{ClusterBackend, ConnectionContext};

use super::*;
use crate::v2_run_ledger::{CreateRun, RunLedger, fake::FakeRunLedger};

async fn insert_fake_run(ledger: &FakeRunLedger, run: &str, key: &str, digest: char) {
    ledger
        .create_or_get(CreateRun {
            run_id: RunId::new(run),
            submission_key: IdempotencyKey::new(key).assert_value(),
            submission_digest: Sha256Digest::new(digest.to_string().repeat(64)).assert_value(),
            admitted: crate::native_v2_runner::test_support::admitted(),
        })
        .await
        .assert_value();
}

fn workspace_identity(root: &Path) -> (PathBuf, WorkspaceIdentity) {
    let workspace = root.join("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let identity = WorkspaceIdentity::capture(&workspace).assert_value();
    (workspace, identity)
}

fn private_directory(root: &Path, name: &str) -> PathBuf {
    let directory = root.join(name);
    crate::execution::platform::create_private_directory(&directory).assert_value();
    directory
}

fn observer_storage(root: &Path) -> (PortableControllerPaths, Arc<SqliteRunLedger>) {
    let state = private_directory(root, "observer-state");
    let paths = PortableControllerPaths::new(&state);
    let ledger = Arc::new(SqliteRunLedger::open(paths.ledger()).assert_value());
    (paths, ledger)
}

async fn terminal_observer(root: &Path, run_id: RunId) -> PortableRunController {
    let (paths, ledger) = observer_storage(root);
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: IdempotencyKey::new("durable-observer-key").assert_value(),
            submission_digest: Sha256Digest::new("c".repeat(64)).assert_value(),
            admitted: crate::native_v2_runner::test_support::admitted(),
        })
        .await
        .assert_value_with("durable observer fixture creation");
    ledger
        .append(
            &run_id,
            vec![crate::v2_run_ledger::RunEvent::Terminal {
                result: openengine_cluster_protocol::TerminalResult::Succeeded {
                    output: serde_json::Value::Null,
                },
            }],
        )
        .await
        .assert_value_with("durable observer fixture settlement");
    let controller = PortableRunController::open_observer(paths, run_id)
        .await
        .assert_value_with("durable observer reopen");
    drop(ledger);
    controller
}

#[tokio::test]
async fn boundary_contract_single_run_allocator_refuses_foreign_runs_and_confirms_absent_runtime_cleanup()
 {
    let root = tempfile::tempdir().assert_value();
    let state = private_directory(root.path(), "allocator-state");
    let run_id = RunId::new("run-single-allocator");
    let foreign = RunId::new("run-foreign");
    let lease = Arc::new(ControllerLease::acquire(state.join("controller.lock")).assert_value());
    let allocator = SingleRunAllocator::new(run_id.clone(), None, lease, state.join("checkpoints"));

    assert!(allocator.require_run(&run_id).is_ok());
    assert!(allocator.require_run(&foreign).is_err());
    assert!(allocator.claim_controller(&foreign).await.is_err());
    let admitted = crate::native_v2_runner::test_support::admitted();
    assert!(allocator.allocate(&foreign, &admitted, None).await.is_err());
    assert!(allocator.allocate(&run_id, &admitted, None).await.is_err());
    let claim = allocator.claim_controller(&run_id).await.assert_value();
    drop(claim);

    let empty = allocator
        .checkpoints(openengine_cluster_protocol::RunCheckpointsParams {
            run_id: run_id.clone(),
            after: None,
            limit: None,
        })
        .await
        .assert_value();
    assert_eq!(empty.run_id, run_id);
    assert!(empty.checkpoints.is_empty());
    assert!(empty.next_after.is_none());
    assert!(
        allocator
            .checkpoints(openengine_cluster_protocol::RunCheckpointsParams {
                run_id: run_id.clone(),
                after: Some(
                    openengine_cluster_protocol::CheckpointId::new("unknown-checkpoint")
                        .assert_value(),
                ),
                limit: Some(1),
            })
            .await
            .is_err()
    );
    assert!(
        allocator
            .destroy_or_confirm_absent(&foreign, RunRuntimeExit::Failed)
            .await
            .is_err()
    );
    allocator
        .destroy_or_confirm_absent(&run_id, RunRuntimeExit::Completed)
        .await
        .assert_value();

    let mut loss = allocator.loss_receiver.clone();
    allocator.loss_sender().send_replace(true);
    loss.changed().await.assert_value();
    assert!(*loss.borrow());
}

#[test]
fn boundary_contract_workspace_identity_detects_replacement_and_rejects_non_directories() {
    let root = tempfile::tempdir().assert_value();
    let (workspace, identity) = workspace_identity(root.path());
    assert!(identity.is_current(&workspace));

    let original = root.path().join("original");
    std::fs::rename(&workspace, &original).assert_value();
    std::fs::create_dir(&workspace).assert_value();
    assert!(!identity.is_current(&workspace));
    assert!(!identity.is_current(&root.path().join("absent")));

    let file = root.path().join("file");
    std::fs::write(&file, b"not a workspace").assert_value();
    assert!(matches!(
        WorkspaceIdentity::capture(&file),
        Err(PortableControllerError::Workspace)
    ));
}

#[tokio::test]
async fn boundary_contract_empty_ledger_has_no_existing_run_and_storage_rejects_unsafe_ledger_entries()
 {
    let run_id = RunId::new("run-empty-ledger");
    let ledger = FakeRunLedger::new();
    assert!(!validate_existing_run(&ledger, &run_id).await.assert_value());

    assert!(matches!(
        open_controller_storage(Path::new("relative")),
        Err(PortableControllerError::Path)
    ));

    let root = tempfile::tempdir().assert_value();
    let unsafe_storage = private_directory(root.path(), "unsafe-state");
    std::fs::create_dir(unsafe_storage.join("runs.sqlite3")).assert_value();
    assert!(matches!(
        open_controller_storage(&unsafe_storage),
        Err(PortableControllerError::LedgerPath)
    ));

    let storage = root.path().join("state");
    let (paths, lease, ledger) = open_controller_storage(&storage).assert_value();
    assert_eq!(paths.storage(), storage);
    assert!(lease.is_intact());
    drop((ledger, lease));
}

#[tokio::test]
async fn coverage_contract_observer_refuses_storage_without_the_exact_durable_run() {
    let root = tempfile::tempdir().assert_value();
    let (paths, ledger) = observer_storage(root.path());
    // Keep the initialized database open while the observer takes its own connection. Closing the
    // final SQLite connection here would test connection teardown instead of durable identity.
    let result = PortableRunController::open_observer(paths, RunId::new("missing-run")).await;
    drop(ledger);
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("observer accepted an empty durable ledger"),
    };
    assert!(
        matches!(error, PortableControllerError::DurableIdentity),
        "observer must reject an empty durable ledger, got {error:?}"
    );
}

#[tokio::test]
async fn coverage_contract_observer_reopens_one_exact_durable_run_and_keeps_identity_fenced() {
    let root = tempfile::tempdir().assert_value();
    let run_id = RunId::new("durable-observer-run");
    let controller = terminal_observer(root.path(), run_id.clone()).await;
    assert_eq!(controller.run_id(), &run_id);
    assert_eq!(
        controller.paths().storage(),
        root.path().join("observer-state")
    );
    assert!(controller.require_run(&run_id).is_ok());
    assert!(controller.require_run(&RunId::new("other-run")).is_err());
}

async fn assert_exact_backend_delegation(
    controller: &PortableRunController,
    context: &ConnectionContext,
    run_id: &RunId,
) {
    let initialized = ClusterBackend::initialize(
        controller,
        context,
        InitializeParams {
            protocol_version: PROTOCOL_VERSION.to_owned(),
        },
    )
    .await
    .assert_value();
    assert_eq!(initialized.protocol_version, PROTOCOL_VERSION);
    ClusterBackend::get(controller, context, GetParams::default())
        .await
        .assert_value();

    let checkpoints = ClusterBackend::run_checkpoints(
        controller,
        context,
        RunCheckpointsParams {
            run_id: run_id.clone(),
            after: None,
            limit: None,
        },
    )
    .await
    .assert_value();
    assert_eq!(&checkpoints.run_id, run_id);
    assert!(checkpoints.checkpoints.is_empty());

    let status = ClusterBackend::run_status(
        controller,
        context,
        RunStatusParams {
            run_id: run_id.clone(),
        },
    )
    .await
    .assert_value();
    assert!(matches!(status.status, RunStatus::Finished { .. }));
    let (watch, watch_stream) = ClusterBackend::run_watch(
        controller,
        context,
        RunWatchParams {
            run_id: run_id.clone(),
            from_cursor: None,
        },
    )
    .await
    .assert_value();
    assert_eq!(&watch.run_id, run_id);
    drop(watch_stream);
    let (logs, log_stream) = ClusterBackend::run_logs(
        controller,
        context,
        RunLogsParams {
            run_id: run_id.clone(),
            from_cursor: None,
            execution: None,
        },
    )
    .await
    .assert_value();
    assert_eq!(&logs.run_id, run_id);
    drop(log_stream);

    let missing_execution = ExecutionRef::new("missing-execution").assert_value();
    let Err(attach_error) = ClusterBackend::run_attach(
        controller,
        context,
        RunAttachParams {
            run_id: run_id.clone(),
            execution: missing_execution,
        },
    )
    .await
    else {
        panic!("terminal run unexpectedly exposed a live execution");
    };
    assert_eq!(attach_error.code, NOT_FOUND);
    assert_eq!(attach_error.message, "execution was not found");

    let forced = ClusterBackend::run_force(
        controller,
        context,
        RunForceParams {
            run_id: run_id.clone(),
        },
    )
    .await
    .assert_value();
    assert_eq!(&forced.run_id, run_id);
    assert!(matches!(forced.status, RunStatus::Finished { .. }));
}

#[tokio::test]
async fn portable_backend_delegates_exact_run_operations_and_fences_foreign_ids() {
    let root = tempfile::tempdir().assert_value();
    let run_id = RunId::new("portable-backend-run");
    let controller = terminal_observer(root.path(), run_id.clone()).await;
    let context = ConnectionContext::default();

    assert_exact_backend_delegation(&controller, &context, &run_id).await;
    let Err(error) = ClusterBackend::run_status(
        &controller,
        &context,
        RunStatusParams {
            run_id: RunId::new("portable-backend-foreign"),
        },
    )
    .await
    else {
        panic!("portable backend accepted a foreign run identity");
    };
    assert_eq!(error.code, NOT_FOUND);
    assert_eq!(error.message, "run was not found");
}

#[test]
fn coverage_contract_directory_cleanup_does_not_treat_a_non_directory_as_absent() {
    let root = tempfile::tempdir().assert_value();
    let file = root.path().join("not-a-directory");
    std::fs::write(&file, b"retain").assert_value();

    assert!(remove_directory_if_present(&file).is_err());
    assert_eq!(std::fs::read(&file).assert_value(), b"retain");
}

#[tokio::test]
async fn coverage_contract_existing_ledger_requires_one_exact_run_identity() {
    let ledger = FakeRunLedger::new();
    let expected = RunId::new("expected-run");
    insert_fake_run(&ledger, expected.as_str(), "first-key", 'a').await;
    assert!(
        validate_existing_run(&ledger, &expected)
            .await
            .assert_value()
    );
    assert!(matches!(
        validate_existing_run(&ledger, &RunId::new("foreign-run")).await,
        Err(PortableControllerError::DurableIdentity)
    ));

    insert_fake_run(&ledger, "second-run", "second-key", 'b').await;
    assert!(matches!(
        validate_existing_run(&ledger, &expected).await,
        Err(PortableControllerError::DurableIdentity)
    ));
}

#[test]
fn coverage_contract_workspace_loss_requires_all_three_sources_of_positive_evidence() {
    for identity in [false, true] {
        for workspace_lease in [false, true] {
            for controller_lease in [false, true] {
                assert_eq!(
                    workspace_is_lost(identity, workspace_lease, controller_lease),
                    !(identity && workspace_lease && controller_lease)
                );
            }
        }
    }
}

#[test]
fn final_contract_workspace_monitor_distinguishes_continue_loss_and_owner_shutdown() {
    let root = tempfile::tempdir().assert_value();
    let state = private_directory(root.path(), "monitor-state");
    let (workspace, identity) = workspace_identity(&state);
    let controller =
        Arc::new(ControllerLease::acquire(state.join("controller.lock")).assert_value());
    let workspace_lease =
        Arc::new(ControllerLease::acquire(state.join("workspace.lock")).assert_value());
    let monitor = WorkspaceMonitor {
        workspace,
        identity,
        controller_lease: Arc::downgrade(&controller),
        workspace_lease: Arc::downgrade(&workspace_lease),
    };
    assert!(active_monitor_leases(&monitor).is_some());
    assert_eq!(
        workspace_monitor_action(&monitor),
        WorkspaceMonitorAction::Continue
    );
    drop(workspace_lease);
    assert!(active_monitor_leases(&monitor).is_none());
    assert_eq!(
        workspace_monitor_action(&monitor),
        WorkspaceMonitorAction::Stop
    );

    let replacement =
        Arc::new(ControllerLease::acquire(state.join("replacement.lock")).assert_value());
    let monitor = WorkspaceMonitor {
        workspace: monitor.workspace,
        identity: monitor.identity,
        controller_lease: Arc::downgrade(&controller),
        workspace_lease: Arc::downgrade(&replacement),
    };
    assert!(active_monitor_leases(&monitor).is_some());
    let moved = state.join("moved-workspace");
    std::fs::rename(&monitor.workspace, &moved).assert_value();
    std::fs::create_dir(&monitor.workspace).assert_value();
    assert_eq!(
        workspace_monitor_action(&monitor),
        WorkspaceMonitorAction::Lost
    );
    drop(controller);
    assert!(active_monitor_leases(&monitor).is_none());
    assert_eq!(
        workspace_monitor_action(&monitor),
        WorkspaceMonitorAction::Stop
    );
}
