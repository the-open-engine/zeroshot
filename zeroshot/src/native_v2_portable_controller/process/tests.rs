use std::fs;

use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

#[tokio::test]
async fn boundary_contract_readiness_rejects_malformed_or_mismatched_documents_and_times_out_immediately()
 {
    let root = tempfile::tempdir().assert_value();
    let storage = root.path().join("state");
    crate::execution::platform::create_private_directory(&storage).assert_value();
    let paths = PortableControllerPaths::new(&storage);
    let run_id = RunId::new("run-ready-contract");

    for ready in [
        PortableControllerReady {
            kind: "wrong-kind".to_owned(),
            run_id: run_id.clone(),
            socket: paths.socket(),
            pid: 1,
            lease_identity: None,
        },
        PortableControllerReady {
            kind: READY_KIND.to_owned(),
            run_id: run_id.clone(),
            socket: storage.join("wrong.sock"),
            pid: 1,
            lease_identity: None,
        },
    ] {
        let bytes = serde_json::to_vec(&ready).assert_value();
        write_private_new_file(&paths.ready(), &bytes).assert_value();
        assert!(matches!(
            read_ready(&paths),
            Err(PortableControllerError::Readiness)
        ));
        fs::remove_file(paths.ready()).assert_value();
    }

    write_private_new_file(&paths.ready(), br#"{"kind":"unknown","extra":true}"#).assert_value();
    assert!(matches!(
        read_ready(&paths),
        Err(PortableControllerError::Readiness)
    ));
    fs::remove_file(paths.ready()).assert_value();

    let expected = PortableControllerReady {
        kind: READY_KIND.to_owned(),
        run_id: run_id.clone(),
        socket: paths.socket(),
        pid: 7,
        lease_identity: None,
    };
    write_private_new_file(
        &paths.ready(),
        &serde_json::to_vec(&expected).assert_value(),
    )
    .assert_value();
    assert_eq!(
        wait_ready(&paths, &run_id, Duration::from_millis(50))
            .await
            .assert_value(),
        expected
    );
    fs::remove_file(paths.ready()).assert_value();

    assert!(matches!(
        wait_ready(&paths, &run_id, Duration::ZERO).await,
        Err(PortableControllerError::Readiness)
    ));
}

#[test]
fn boundary_contract_bootstrap_parse_failure_is_consumed_and_file_bounds_are_enforced() {
    let root = tempfile::tempdir().assert_value();
    let malformed = root.path().join("bootstrap.json");
    write_private_new_file(&malformed, b"{").assert_value();
    assert!(matches!(
        load_bootstrap_file(&malformed),
        Err(PortableControllerError::Bootstrap)
    ));
    assert!(
        !malformed.exists(),
        "even malformed bootstrap secrets are consumed"
    );

    let bounded = root.path().join("bounded");
    fs::write(&bounded, b"four").assert_value();
    assert_eq!(
        read_bounded_file(fs::File::open(&bounded).assert_value(), 4).assert_value(),
        b"four"
    );
    assert!(matches!(
        read_bounded_file(fs::File::open(&bounded).assert_value(), 3),
        Err(PortableControllerError::Bootstrap)
    ));

    #[cfg(unix)]
    assert!(matches!(
        read_bounded_file(fs::File::open(root.path()).assert_value(), 4),
        Err(PortableControllerError::Bootstrap)
    ));

    #[cfg(target_os = "linux")]
    assert!(matches!(
        read_bounded_file(fs::File::open("/proc/self/status").assert_value(), 1),
        Err(PortableControllerError::Bootstrap)
    ));
}

#[test]
fn boundary_contract_path_validation_distinguishes_regular_files_directories_symlinks_and_absence()
{
    let root = tempfile::tempdir().assert_value();
    assert!(require_absolute(root.path()).is_ok());
    assert!(matches!(
        require_absolute(Path::new("relative")),
        Err(PortableControllerError::Path)
    ));

    let missing = root.path().join("missing");
    assert!(validate_ledger_path(&missing).is_ok());
    assert!(remove_existing_regular_file(&missing).is_ok());

    let file = root.path().join("ledger");
    fs::write(&file, b"ledger").assert_value();
    assert!(validate_ledger_path(&file).is_ok());
    assert!(validate_existing_ledger_path(&file).is_ok());
    assert!(matches!(
        validate_existing_storage(&file),
        Err(PortableControllerError::LedgerPath)
    ));

    let directory = root.path().join("directory");
    fs::create_dir(&directory).assert_value();
    assert!(validate_existing_storage(&directory).is_ok());
    assert!(matches!(
        validate_ledger_path(&directory),
        Err(PortableControllerError::LedgerPath)
    ));
    assert!(matches!(
        validate_existing_ledger_path(&directory),
        Err(PortableControllerError::LedgerPath)
    ));
    assert!(matches!(
        remove_existing_regular_file(&directory),
        Err(PortableControllerError::EndpointPath)
    ));

    let removable = root.path().join("ready");
    fs::write(&removable, b"ready").assert_value();
    remove_existing_regular_file(&removable).assert_value();
    assert!(!removable.exists());

    let blocking_file = root.path().join("blocking-file");
    fs::write(&blocking_file, b"not a directory").assert_value();
    let blocked_child = blocking_file.join("child");
    #[cfg(unix)]
    assert!(matches!(
        validate_ledger_path(&blocked_child),
        Err(PortableControllerError::LedgerPath)
    ));
    // Windows reports a path below a regular file as absent. The later open still fails closed;
    // the path-only admission check cannot distinguish it from an ordinary missing ledger.
    #[cfg(windows)]
    assert!(validate_ledger_path(&blocked_child).is_ok());
    #[cfg(unix)]
    assert!(matches!(
        remove_existing_regular_file(&blocked_child),
        Err(PortableControllerError::Io(_))
    ));
    #[cfg(windows)]
    assert!(remove_existing_regular_file(&blocked_child).is_ok());

    #[cfg(unix)]
    crate::execution::platform::private_directory(root.path()).assert_value();
    let paths = PortableControllerPaths::new(root.path());
    fs::write(paths.ready(), b"stale readiness").assert_value();
    clear_stale_endpoint(&paths).assert_value();
    assert!(!paths.ready().exists());

    #[cfg(unix)]
    {
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&file, &link).assert_value();
        assert!(matches!(
            validate_ledger_path(&link),
            Err(PortableControllerError::LedgerPath)
        ));
        assert!(matches!(
            validate_existing_ledger_path(&link),
            Err(PortableControllerError::LedgerPath)
        ));
        assert!(matches!(
            remove_existing_regular_file(&link),
            Err(PortableControllerError::EndpointPath)
        ));
        assert!(link.exists(), "unsafe endpoint entries are never removed");
    }
}

#[test]
fn coverage_contract_bootstrap_parent_and_private_read_refuse_ambiguous_filesystem_entries() {
    assert!(matches!(
        prepare_bootstrap_parent(Path::new("/")),
        Err(PortableControllerError::Path)
    ));

    let root = tempfile::tempdir().assert_value();
    let regular_parent = root.path().join("regular-parent");
    fs::write(&regular_parent, b"not a directory").assert_value();
    assert!(prepare_bootstrap_parent(&regular_parent.join("bootstrap.json")).is_err());

    let missing = root.path().join("missing-bootstrap.json");
    assert!(matches!(
        validate_private_bootstrap(&missing),
        Err(PortableControllerError::BootstrapPermissions)
    ));
    assert!(matches!(
        read_bounded_regular_file(root.path(), 16),
        Err(PortableControllerError::Bootstrap | PortableControllerError::Io(_))
    ));

    #[cfg(unix)]
    {
        let source = root.path().join("source");
        fs::write(&source, b"secret").assert_value();
        let link = root.path().join("bootstrap-link");
        std::os::unix::fs::symlink(&source, &link).assert_value();
        assert!(matches!(
            validate_private_bootstrap(&link),
            Err(PortableControllerError::BootstrapPermissions)
        ));
        assert!(matches!(
            read_bounded_regular_file(&link, 16),
            Err(PortableControllerError::Io(_))
        ));
    }
}

#[test]
fn final_contract_all_private_bootstrap_paths_must_be_absolute() {
    let root = tempfile::tempdir().assert_value();
    let valid = [
        root.path().join("workspace"),
        root.path().join("workspace.lock"),
        root.path().join("repository"),
        root.path().join("state"),
    ];
    validate_bootstrap_paths(&valid[0], &valid[1], &valid[2], &valid[3]).assert_value();

    for relative_index in 0..valid.len() {
        let mut paths = valid.clone();
        paths[relative_index] = PathBuf::from("relative");
        assert!(matches!(
            validate_bootstrap_paths(&paths[0], &paths[1], &paths[2], &paths[3]),
            Err(PortableControllerError::Path)
        ));
    }
}

#[tokio::test]
async fn final_contract_controller_entry_consumes_a_rejected_bootstrap_before_any_effect() {
    let root = tempfile::tempdir().assert_value();
    let path = root.path().join("controller.bootstrap.json");
    write_private_new_file(&path, b"{").assert_value();

    assert!(matches!(
        run_controller_process(&path).await,
        Err(PortableControllerError::Bootstrap)
    ));
    assert!(!path.exists());
    assert_eq!(
        take_startup_failure(&PortableControllerPaths::new(root.path())).as_deref(),
        Some("portable controller bootstrap is malformed or too large")
    );
}

#[test]
fn coverage_contract_controller_server_waits_for_both_terminal_truth_and_one_observer() {
    let mut lifecycle = ServerLifecycle::default();
    assert!(!lifecycle.is_complete());
    lifecycle.terminal();
    assert!(
        !lifecycle.is_complete(),
        "a short run still needs one observer"
    );
    lifecycle.accepted();
    assert!(lifecycle.is_complete());

    let mut reversed = ServerLifecycle::default();
    reversed.accepted();
    assert!(
        !reversed.is_complete(),
        "an observer cannot hide active work"
    );
    reversed.terminal();
    assert!(reversed.is_complete());
}

/// Confirms runtime-failure cleanup, then holds the run ledger briefly, as a slow disk or a
/// concurrent ledger reader can, so the durable `runtime_failed` append is still pending when the
/// failure is already observable in memory.
#[cfg(target_os = "linux")]
struct SlowFailurePersistence {
    ledger: PathBuf,
    holder: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}

#[cfg(target_os = "linux")]
impl SlowFailurePersistence {
    const HOLD: Duration = Duration::from_secs(1);

    fn release(&self) {
        let holder = self
            .holder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .assert_value_with("runtime failure cleanup ran");
        holder.join().assert_value();
    }
}

#[cfg(target_os = "linux")]
#[async_trait::async_trait]
impl crate::native_v2_platform::CapsuleCleanup for SlowFailurePersistence {
    async fn destroy_or_confirm_absent(
        &self,
        exit: crate::native_v2_supervisor::RunRuntimeExit,
    ) -> Result<
        crate::native_v2_platform::CapsuleDestroyed,
        crate::native_v2_platform::CapsuleCleanupUnavailable,
    > {
        let mut holder = self
            .holder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(
            exit,
            crate::native_v2_supervisor::RunRuntimeExit::RuntimeLost
        ) && holder.is_none()
        {
            let connection = rusqlite::Connection::open(&self.ledger).assert_value();
            connection.execute_batch("BEGIN EXCLUSIVE").assert_value();
            *holder = Some(std::thread::spawn(move || {
                std::thread::sleep(Self::HOLD);
                connection.execute_batch("ROLLBACK").assert_value();
            }));
        }
        Ok(crate::native_v2_platform::CapsuleDestroyed::confirmed())
    }
}

#[cfg(target_os = "linux")]
#[test]
fn final_contract_controller_serves_a_runtime_failure_until_its_terminal_is_durable() {
    use std::os::unix::ffi::OsStrExt;

    use openengine_cluster_protocol::{NodeName, TerminalResult};

    use crate::native_v2_candidate::test_support::{full_graph, success_node};
    use crate::native_v2_portable_controller::tests::{NeverDispatched, bootstrap, submission};
    use crate::v2_run_ledger::RunLedger;
    use crate::v2_run_ledger::sqlite::SqliteRunLedger;

    let root = tempfile::tempdir().assert_value();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).assert_value();
    // The first workspace checkpoint refuses this entry, so the runtime fails before dispatch.
    let pipe = std::ffi::CString::new(workspace.join("pipe").as_os_str().as_bytes()).assert_value();
    // SAFETY: pipe is a live NUL-terminated filename in this test's private directory.
    assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0);
    let storage = root.path().join("state");
    let run_id = RunId::new("run-runtime-failure-exit");
    let mut submitted = submission("runtime-failure-exit");
    submitted.graph = full_graph(vec![
        serde_json::json!({"kind":"step","name":"work","worker":"worker.work@1",
            "instructions":"Change the workspace.","input":{"kind":"null"},
            "output":{"kind":"null"},"inputBindings":[],"writeBindings":[],"attempts":1}),
        success_node(),
    ]);
    submitted.runtime.nodes_mut().insert(
        NodeName::new("work").assert_value(),
        serde_json::from_value(serde_json::json!({"kind":"agent","model":"fixture"}))
            .assert_value(),
    );
    let persistence = Arc::new(SlowFailurePersistence {
        ledger: PortableControllerPaths::new(&storage).ledger(),
        holder: std::sync::Mutex::default(),
    });

    // Like the controller child, this runtime ends as soon as serving returns.
    let controller_process = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .assert_value();
    controller_process.block_on(async {
        let cleanup = persistence.clone();
        let controller = Arc::new(
            PortableRunController::start(
                bootstrap(run_id.clone(), submitted, workspace, storage.clone()),
                move |_| {
                    Ok::<_, std::convert::Infallible>(PortableRuntime::with_cleanup(
                        Arc::new(NeverDispatched),
                        cleanup,
                    ))
                },
            )
            .await
            .assert_value_with("start portable controller"),
        );
        let server = controller.clone().bind().await.assert_value();
        // The submitting CLI is the observer a short run waits for.
        let _observer = connect_transport(controller.paths()).await.assert_value();
        let (_stop, stopping) = tokio::sync::watch::channel(false);
        tokio::time::timeout(
            Duration::from_secs(30),
            server.serve_until_terminal(stopping),
        )
        .await
        .assert_value_with("controller stopped serving")
        .assert_value();
    });
    drop(controller_process);
    persistence.release();

    let stored = tokio::runtime::Builder::new_current_thread()
        .build()
        .assert_value()
        .block_on(async {
            SqliteRunLedger::open_read_only(PortableControllerPaths::new(&storage).ledger())
                .assert_value()
                .get(&run_id)
                .await
        })
        .assert_value()
        .assert_value_with("durable run");
    let is_cause = |context: &openengine_cluster_protocol::FailureContext| {
        context.primary.as_ref().is_some_and(|primary| {
            primary.message.as_str()
                == "Runtime failed: workspace checkpoint is unavailable: \
                    candidate contains an unsupported filesystem entry \"pipe\""
        })
    };
    // The cause is logged before cleanup, so it is durable even when the terminal is not.
    assert!(
        stored
            .snapshot
            .failure_context
            .as_deref()
            .is_some_and(is_cause),
        "the checkpoint failure cause was not recorded: {:?}",
        stored.snapshot.failure_context
    );
    let terminal = stored.snapshot.terminal;
    assert!(
        matches!(
            &terminal,
            Some(TerminalResult::Failed { reason, context: Some(context) })
                if reason.as_str() == "runtime_failed" && is_cause(context)
        ),
        "the controller stopped serving before its runtime_failed terminal was durable: \
         {terminal:?}"
    );
}
