#[cfg(unix)]
use openengine_cluster_testkit::fixture::run_size;
use openengine_cluster_protocol::{
    ExecutionRef, GetParams, IdempotencyKey, InitializeParams, NOT_FOUND, PROTOCOL_VERSION,
    RunAttachParams, RunCheckpointsParams, RunForceParams, RunLogsParams, RunStatus,
    RunStatusParams, RunWatchParams, Sha256Digest,
};
use openengine_cluster_testkit::assertions::AssertValue;
use openengine_cluster_server::{ClusterBackend, ConnectionContext};

use super::*;

struct ForceCleanup(std::sync::atomic::AtomicUsize);

#[tokio::test]
async fn forcing_a_suspended_observer_discards_its_recorded_checkpoint_lineage() {
    use crate::native_v2_supervisor::checkpoints::RunCheckpointStore;
    use crate::v2_run_ledger::RunEvent;
    use openengine_cluster_protocol::{NodeName, RunSuspension, UnixTimestampMillis};

    let root = tempfile::tempdir().assert_value();
    let storage = private_directory(root.path(), "run");
    let workspace = private_directory(root.path(), "workspace");
    let paths = PortableControllerPaths::new(storage);
    let run_id = RunId::new("suspended-observer-force");
    let directory = paths.storage().join("checkpoints");
    let store = crate::native_v2_supervisor::checkpoints::fake_checkpoint_store(
        root.path(),
        directory.clone(),
        workspace,
        Default::default(),
    );
    let node = NodeName::new("held").assert_value();
    store
        .enter(
            &crate::full_v1_reducer::ExecutionBoundary {
                node: node.clone(),
                map_indices: Vec::new(),
                loop_iterations: Vec::new(),
                attempt: 1,
            },
            &[],
        )
        .await
        .assert_value();
    let ledger = SqliteRunLedger::open(paths.ledger()).assert_value();
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: IdempotencyKey::new("suspended-observer-force").assert_value(),
            submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
            admitted: crate::native_v2_runner::test_support::admitted(),
        })
        .await
        .assert_value();
    ledger
        .append(
            &run_id,
            vec![
                RunEvent::RunStarted,
                RunEvent::Suspended {
                    suspension: RunSuspension::Command {
                        node,
                        created_at: UnixTimestampMillis::new(1).assert_value(),
                    },
                },
            ],
        )
        .await
        .assert_value();
    drop(ledger);

    let controller = PortableRunController::open_observer(paths, run_id.clone())
        .await
        .assert_value();
    let forced = controller
        .inner
        .force(RunForceParams { run_id })
        .await
        .assert_value();
    assert!(matches!(forced.status, RunStatus::Finished {
        terminal_result: openengine_cluster_protocol::TerminalResult::Failed { reason, .. }, ..
    } if reason.as_str() == "force_stopped"));
    assert!(!directory.exists());
    assert!(!root.path().join("repository").exists());
}

#[async_trait]
impl CapsuleCleanup for ForceCleanup {
    async fn destroy_or_confirm_absent(
        &self,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        assert_eq!(exit, RunRuntimeExit::ForceStopped);
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(CapsuleDestroyed::confirmed())
    }
}

#[tokio::test]
async fn local_force_confirms_cleanup_despite_an_unpersisted_runtime_failure() {
    use crate::native_v2_cli::{NativeV2CliBackend, local::LocalCliBackend};
    use crate::v2_run_ledger::RunEvent;

    let root = tempfile::tempdir().assert_value();
    let runs = private_directory(root.path(), "runs");
    let run_id = RunId::new(uuid::Uuid::now_v7().to_string());
    let paths = PortableControllerPaths::new(private_directory(&runs, run_id.as_str()));
    let lease = Arc::new(ControllerLease::acquire(paths.lease()).assert_value());
    let ledger = Arc::new(SqliteRunLedger::open(paths.ledger()).assert_value());
    let cleanup = Arc::new(ForceCleanup(std::sync::atomic::AtomicUsize::new(0)));
    let runtime = PortableRuntime {
        vector_seed: Vec::new(),
        cleanup: cleanup.clone(),
        ..PortableRuntime::new(Arc::new(super::super::tests::NeverDispatched))
    };
    let allocator = Arc::new(SingleRunAllocator::new(
        run_id.clone(),
        Some(runtime),
        lease.clone(),
        paths.storage().join("checkpoints"),
    ));
    let inner = Arc::new(
        NativeV2PlatformController::new(ledger.clone(), allocator)
            .await
            .assert_value(),
    );
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: IdempotencyKey::new("force-runtime-failure").assert_value(),
            submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
            admitted: crate::native_v2_runner::test_support::admitted(),
        })
        .await
        .assert_value();
    let stored = ledger
        .append(&run_id, vec![RunEvent::RunStarted])
        .await
        .assert_value();
    // A controller can publish failure while its owned runtime still needs cleanup.
    let observation = inner.observations();
    observation.track_runtime(&stored.snapshot).assert_value();
    observation.runtime_failed_with_cause(&run_id, "cleanup is unconfirmed");
    assert!(stored.snapshot.terminal.is_none());
    let controller = Arc::new(PortableRunController {
        run_id: run_id.clone(),
        inner,
        paths,
        _lease: lease,
        _workspace_lease: None,
    });
    let server = controller.bind().await.assert_value();
    let serving = tokio::spawn(server.serve());
    let backend = LocalCliBackend::new(
        root.path().to_owned(),
        "unused".into(),
        root.path().to_owned(),
        "git".into(),
    );
    let before = backend
        .run_status(
            None,
            RunStatusParams {
                run_id: run_id.clone(),
            },
        )
        .await
        .assert_value();
    assert!(
        matches!(before.status, openengine_cluster_protocol::HostedRunStatus::Target(RunStatus::Finished {
        terminal_result: TerminalResult::Failed { reason, .. }, ..
    }) if reason.as_str() == "runtime_failed")
    );
    assert!(
        backend
            .run_resume_status(
                None,
                RunStatusParams {
                    run_id: run_id.clone()
                }
            )
            .await
            .is_err(),
        "a live failure projection cannot authorize resume"
    );
    let forced = tokio::time::timeout(
        Duration::from_secs(20),
        backend.run_force(
            None,
            RunForceParams {
                run_id: run_id.clone(),
            },
        ),
    )
    .await;
    serving.abort();
    let _ = serving.await;
    let forced = forced.assert_value().assert_value();
    assert_eq!(cleanup.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    let recorded = ledger.get(&run_id).await.assert_value().assert_value();
    assert!(matches!(recorded.snapshot.terminal,
        Some(TerminalResult::Failed { reason, .. }) if reason.as_str() == "force_stopped"));
    assert!(
        matches!(forced.status, openengine_cluster_protocol::HostedRunStatus::Target(RunStatus::Finished {
        terminal_result: TerminalResult::Failed { reason, .. }, ..
    }) if reason.as_str() == "force_stopped")
    );
}

#[tokio::test]
async fn shutdown_before_runtime_composition_never_dispatches_or_admits_a_run() {
    let root = tempfile::tempdir().assert_value();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let storage = root.path().join("state");
    let (_request, stopping) = watch::channel(true);
    let result = PortableRunController::start_with_shutdown(
        crate::native_v2_portable_controller::tests::bootstrap(
            RunId::new("stopped-startup"),
            crate::native_v2_portable_controller::tests::submission("stopped-startup"),
            workspace,
            storage.clone(),
        ),
        async |_| -> Result<PortableRuntime, std::io::Error> {
            panic!("a stopped startup must not compose a runtime")
        },
        Some(stopping),
    )
    .await;
    assert!(matches!(result, Err(PortableControllerError::Shutdown)));
    let ledger = SqliteRunLedger::open_read_only(PortableControllerPaths::new(storage).ledger())
        .assert_value();
    assert!(ledger.list().await.assert_value().is_empty());
}
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

#[cfg(unix)]
#[tokio::test]
async fn review_git_preparation_requires_the_workspace_lease_before_any_mutation() {
    use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};
    use openengine_cluster_protocol::{
        CodexProvider, DeclaredConnections, NodeName, NodeRuntimeBinding, RuntimePlan,
    };
    use serde_json::json;
    for occupied in [true, false] {
        let root = tempfile::tempdir().assert_value();
        let workspace = root.path().join("workspace");
        let bin = root.path().join("bin");
        std::fs::create_dir(&workspace).assert_value();
        std::fs::create_dir(&bin).assert_value();
        let git = bin.join("git");
        openengine_cluster_testkit::fixture::write_executable(
            &git,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$0.calls\"\n",
            0o755,
        )
        .assert_value();
        let mut submission =
            crate::native_v2_portable_controller::tests::submission("review-lease");
        submission.graph = BuiltinGraphTemplate::IssueReview
            .materialize(TemplateDelivery::Comment)
            .assert_value();
        let revision = submission.source.as_ref().unwrap().revision.as_str();
        submission.initial_input = json!({
            "url":"https://github.com/open-engine/zeroshot/pull/17",
            "context":{"title":"Review","body":"Changes","reviewScope":{
                "baseRepository":"open-engine/zeroshot", "baseBranch":"main", "baseRevision":"b".repeat(40),
                "headRepository":"contributor/fork", "headBranch":"feature", "headRevision":revision
            }}
        });
        submission.runtime = RuntimePlan::new(
            openengine_cluster_protocol::RuntimeLane::new("codex", (CodexProvider::OpenAi).name())
                .assert_value(),
            run_size("host"),
            std::collections::BTreeMap::from([
                (
                    NodeName::new("reviewer").assert_value(),
                    serde_json::from_value(json!({"kind":"agent", "model":"fixture"}))
                        .assert_value(),
                ),
                (
                    NodeName::new("deliver").assert_value(),
                    NodeRuntimeBinding::CommentDelivery {
                        connections: DeclaredConnections::empty(),
                    },
                ),
            ]),
        );
        let mut bootstrap = crate::native_v2_portable_controller::tests::bootstrap(
            RunId::new("review-lease"),
            submission,
            workspace.clone(),
            root.path().join("state"),
        );
        bootstrap
            .native_environment
            .insert("PATH".to_owned(), bin.to_str().unwrap().to_owned());
        let lease = occupied.then(|| {
            ControllerLease::acquire_workspace(&bootstrap.workspace_lease, &workspace)
                .assert_value()
        });
        let prepared = prepare_controller_start(&bootstrap).await.assert_value();
        let deadline = tokio::time::Instant::now() + bootstrap.preparation_timeout;
        let result = prepare_runtime(
            &prepared,
            &bootstrap,
            async |_| {
                Err::<PortableRuntime, _>(std::io::Error::other("stop before graph dispatch"))
            },
            deadline,
        )
        .await;
        if occupied {
            assert!(matches!(
                result,
                Err(PortableControllerError::Lease(
                    super::super::ControllerLeaseError::Held
                ))
            ));
            assert!(!git.with_extension("calls").exists());
        } else {
            assert!(matches!(
                result,
                Err(PortableControllerError::RuntimeComposition(error))
                    if error.to_string() == "stop before graph dispatch"
            ));
            let calls = std::fs::read_to_string(git.with_extension("calls")).assert_value();
            assert!(calls.lines().any(|arg| arg == "fetch"));
            assert!(calls.lines().any(|arg| arg == "refs/zeroshot/review/base"));
        }
        drop(lease);
    }
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
    assert!(
        allocator
            .allocate(
                crate::native_v2_candidate::test_support::allocation_request(
                    &foreign, &admitted, None
                )
            )
            .await
            .is_err()
    );
    assert!(
        allocator
            .allocate(
                crate::native_v2_candidate::test_support::allocation_request(
                    &run_id, &admitted, None
                )
            )
            .await
            .is_err()
    );
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
async fn observer_opens_durable_status_when_checkpoint_scratch_gc_fails() {
    let root = tempfile::tempdir().assert_value();
    let (paths, original_ledger) = observer_storage(root.path());
    drop(original_ledger);
    std::fs::create_dir(paths.storage().join("checkpoints")).assert_value();
    let staging = paths.storage().join("checkpoints/staging");
    std::fs::write(&staging, "uncollectable scratch").assert_value();

    let (_lease, reopened_ledger) = open_observer_storage(&paths, None).assert_value();

    assert!(reopened_ledger.list().await.is_ok());
    assert_eq!(
        std::fs::read_to_string(&staging).assert_value(),
        "uncollectable scratch"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn observer_removes_legacy_readonly_checkpoint_stage() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().assert_value();
    let (paths, original_ledger) = observer_storage(root.path());
    drop(original_ledger);
    let readonly = paths
        .storage()
        .join("checkpoints/staging/.capture-old/workspace");
    std::fs::create_dir_all(&readonly).assert_value();
    std::fs::write(readonly.join("source"), "copied").assert_value();
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o555)).assert_value();

    let (_lease, reopened_ledger) = open_observer_storage(&paths, None).assert_value();

    assert!(reopened_ledger.list().await.is_ok());
    assert!(!paths.storage().join("checkpoints/staging").exists());
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

#[tokio::test]
async fn workspace_checkpoint_store_follows_runtime_policy() {
    use crate::native_v2_portable_controller::tests::{bootstrap, submission, NeverDispatched};
    for enabled in [false, true] {
        let root = tempfile::tempdir().assert_value();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).assert_value();
        let storage = root.path().join("state");
        let mut submitted = submission("checkpoint-policy");
        submitted.runtime.workspace_checkpoints = enabled;
        let bootstrap = bootstrap(
            RunId::new("checkpoint-policy"),
            submitted,
            workspace,
            storage,
        );
        let prepared = prepare_controller_start(&bootstrap).await.assert_value();
        let deadline = tokio::time::Instant::now() + bootstrap.preparation_timeout;
        let runtime = prepare_runtime(
            &prepared,
            &bootstrap,
            async |_| {
                Ok::<_, std::io::Error>(PortableRuntime {
                    vector_seed: Vec::new(),
                    checkpoints: None,
                    execution_seed: Vec::new(),
                    runner: Arc::new(NeverDispatched),
                    cleanup: Arc::new(super::super::engine::ConfirmedCleanup),
                })
            },
            deadline,
        )
        .await
        .assert_value()
        .runtime
        .assert_value();
        let store = runtime.checkpoints.as_ref().assert_value();
        if !enabled {
            assert!(!bootstrap.checkpoint_repository.exists());
            assert!(!prepared.paths.storage().join("checkpoints").exists());
            let state = serde_json::json!({"owned":"verified conversation receipt"});
            store
                .finish(&crate::native_v2_runner::test_support::delivery_history(
                    state.clone(),
                ))
                .await
                .assert_value();
            let recovered =
                configure_workspace_checkpoints(&prepared, &bootstrap, runtime).assert_value();
            assert_eq!(
                recovered
                    .checkpoints
                    .assert_value()
                    .delivery_state()
                    .await
                    .assert_value()
                    .assert_value()
                    .state,
                state
            );
            assert!(!prepared.paths.storage().join("checkpoints").exists());
            assert_eq!(
                std::fs::read_dir(&bootstrap.checkpoint_repository)
                    .assert_value()
                    .count(),
                1
            );
        }
    }
}

#[tokio::test]
async fn shutdown_retains_delivery_receipts_for_both_snapshot_policies_until_explicit_discard() {
    use crate::native_v2_portable_controller::tests::{bootstrap, submission, NeverDispatched};

    for enabled in [false, true] {
        let root = tempfile::tempdir().assert_value();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).assert_value();
        let mut submitted = submission("shutdown-delivery-receipts");
        submitted.runtime.workspace_checkpoints = enabled;
        let bootstrap = bootstrap(
            RunId::new("shutdown-delivery-receipts"),
            submitted,
            workspace.clone(),
            root.path().join("state"),
        );
        let mut prepared = prepare_controller_start(&bootstrap).await.assert_value();
        let (stop, stopping) = watch::channel(false);
        prepared.stopping = Some(stopping);
        let runtime = configure_workspace_checkpoints(
            &prepared,
            &bootstrap,
            PortableRuntime::new(Arc::new(NeverDispatched)),
        )
        .assert_value();
        let receipt = crate::native_v2_runner::test_support::delivery_checkpoint(
            serde_json::json!({"owned":"verified conversation revision"}),
            2,
        );
        runtime
            .checkpoints
            .as_ref()
            .assert_value()
            .retain_delivery_state(&receipt)
            .await
            .assert_value();
        let staging = prepared.paths.storage().join("checkpoints/staging");
        std::fs::create_dir_all(&staging).assert_value();
        std::fs::write(staging.join("interrupted"), "disposable scratch").assert_value();
        stop.send_replace(true);
        runtime
            .cleanup
            .destroy_or_confirm_absent(RunRuntimeExit::ForceStopped)
            .await
            .assert_value();
        assert!(!staging.exists());
        assert!(
            bootstrap
                .checkpoint_repository
                .join("delivery-state.json")
                .exists()
        );
        assert_eq!(
            runtime
                .checkpoints
                .as_ref()
                .assert_value()
                .delivery_state()
                .await
                .assert_value(),
            Some(receipt.clone())
        );
        assert!(workspace.exists());

        // A later explicit force-stop still discards the recovered lineage for either policy.
        prepared.stopping = None;
        let recovered = configure_workspace_checkpoints(
            &prepared,
            &bootstrap,
            PortableRuntime::new(Arc::new(NeverDispatched)),
        )
        .assert_value();
        assert_eq!(
            recovered
                .checkpoints
                .as_ref()
                .assert_value()
                .delivery_state()
                .await
                .assert_value(),
            Some(receipt)
        );
        recovered
            .cleanup
            .destroy_or_confirm_absent(RunRuntimeExit::ForceStopped)
            .await
            .assert_value();
        assert!(!bootstrap.checkpoint_repository.exists());
        assert!(!prepared.paths.storage().join("checkpoints").exists());
        assert!(workspace.exists());
    }
}
