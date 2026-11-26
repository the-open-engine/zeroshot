use super::*;
use crate::native_v2_files::inputs::InputAdmission;
use crate::native_v2_platform::CapsuleAllocator;
use openengine_cluster_protocol::PayloadType;
use serde_json::json;

async fn move_workspace_after_monitor_drop(workspace: &std::path::Path, moved: &std::path::Path) {
    // This in-process fixture keeps the workspace monitor alive briefly after the controller
    // is dropped. Windows cannot rename a directory until that monitor releases its handle.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match std::fs::rename(workspace, moved) {
            Ok(()) => break,
            Err(error)
                if cfg!(windows)
                    && error.raw_os_error() == Some(32)
                    && std::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(error) => panic!("move workspace after completion failed: {error}"),
        }
    }
}

#[tokio::test]
async fn completed_run_reopens_without_recreating_released_inputs_or_its_workspace() {
    let root =
        tempfile::tempdir_in(std::env::temp_dir().canonicalize().assert_value()).assert_value();
    let state = private_directory(root.path(), "state");
    let runs = private_directory(&state, "runs");
    let run = RunId::new("completed-input-recovery");
    let storage = private_directory(&runs, run.as_str());
    let workspace = private_directory(root.path(), "workspace");
    std::fs::write(workspace.join("output"), "live output").assert_value();
    let original = root.path().join("original.csv");
    std::fs::write(&original, "accepted bytes").assert_value();
    let files = crate::native_v2_cli::files::workspace_files(&state).assert_value();
    let frozen = files
        .freeze_file_inputs(InputAdmission {
            run_id: &run,
            owner: &run,
            key: &IdempotencyKey::new("accepted-input").assert_value(),
            schema: &PayloadType::Filepath,
            value: &json!(original),
            local_root: Some(root.path()),
        })
        .assert_value();
    let accepted = PathBuf::from(frozen.value.as_str().assert_value());
    files.materialize_file_inputs(&run, true).assert_value();
    assert_eq!(std::fs::read(&accepted).assert_value(), b"accepted bytes");
    std::fs::write(&original, "later user bytes").assert_value();
    let mut submission = crate::native_v2_portable_controller::tests::submission(run.as_str());
    submission.graph.initial_input = PayloadType::Filepath;
    let openengine_cluster_protocol::GraphNode::Seq(root_group) = &mut submission.graph.root else {
        panic!("fixture requires a sequential root");
    };
    root_group.state = PayloadType::Filepath;
    submission.initial_input = frozen.value;
    submission.runtime.workspace_checkpoints = false;
    std::fs::write(
        storage.join("workspace-recovery.json"),
        serde_json::to_vec(&json!({
            "submission":submission, "workspace":workspace, "deliveryRunId":run,
        }))
        .assert_value(),
    )
    .assert_value();
    let bootstrap = || {
        crate::native_v2_portable_controller::tests::bootstrap(
            run.clone(),
            submission.clone(),
            workspace.clone(),
            storage.clone(),
        )
    };
    let controller = PortableRunController::start(bootstrap(), |_| {
        Ok::<_, std::convert::Infallible>(PortableRuntime::new(Arc::new(
            crate::native_v2_portable_controller::tests::NeverDispatched,
        )))
    })
    .await;
    assert!(
        controller.is_ok(),
        "controller start failed: {:?}",
        controller.as_ref().err()
    );
    let controller = controller.assert_value();
    controller.wait_terminal().await.assert_value();
    let ledger = SqliteRunLedger::open_read_only(controller.paths().ledger()).assert_value();
    let before = ledger.get(&run).await.assert_value().assert_value();
    assert!(matches!(
        before.snapshot.terminal,
        Some(TerminalResult::Succeeded { .. })
    ));
    assert!(!accepted.exists());
    assert!(files.file_input_roots(&run).assert_value().is_empty());
    drop(controller);
    let moved = root.path().join("moved-workspace");
    move_workspace_after_monitor_drop(&workspace, &moved).await;

    let reopened = PortableRunController::reopen(bootstrap())
        .await
        .assert_value();
    reopened.wait_terminal().await.assert_value();
    let after = ledger.get(&run).await.assert_value().assert_value();
    assert_eq!(after.snapshot, before.snapshot);
    assert!(!workspace.exists());
    assert!(!accepted.exists());
    assert!(files.file_input_roots(&run).assert_value().is_empty());
    assert_eq!(std::fs::read(original).assert_value(), b"later user bytes");
    assert_eq!(
        std::fs::read(moved.join("output")).assert_value(),
        b"live output"
    );
}

#[tokio::test]
async fn reconstructed_stop_releases_inputs_only_after_workspace_ownership_is_available() {
    for managed in [false, true] {
        let root =
            tempfile::tempdir_in(std::env::temp_dir().canonicalize().assert_value()).assert_value();
        let state = private_directory(root.path(), "state");
        let runs = private_directory(&state, "runs");
        let run = RunId::new("reconstructed-stop-inputs");
        let storage = private_directory(&runs, run.as_str());
        let paths = PortableControllerPaths::new(storage.clone());
        let workspace = private_directory(root.path(), "workspace");
        let original = root.path().join("original.csv");
        std::fs::write(&original, "user input").assert_value();
        std::fs::write(workspace.join("output"), "live output").assert_value();
        let files = crate::native_v2_cli::files::workspace_files(&state).assert_value();
        let frozen = files
            .freeze_file_inputs(InputAdmission {
                run_id: &run,
                owner: &run,
                key: &IdempotencyKey::new("accepted-input").assert_value(),
                schema: &PayloadType::Filepath,
                value: &json!(original),
                local_root: Some(root.path()),
            })
            .assert_value();
        files.materialize_file_inputs(&run, true).assert_value();
        let accepted = PathBuf::from(frozen.value.as_str().assert_value());
        std::fs::write(storage.join("workspace-recovery.json"), serde_json::to_vec(&json!({
            "submission":crate::native_v2_portable_controller::tests::submission("reconstructed-stop-inputs"),
            "workspace":workspace, "deliveryRunId":run,
        })).assert_value()).assert_value();
        let lease = Arc::new(ControllerLease::acquire(paths.lease()).assert_value());
        let mut allocator =
            SingleRunAllocator::new(run.clone(), None, lease, storage.join("checkpoints"));
        allocator.managed_workspace = managed.then_some(workspace.clone());
        let (_, stopping) = watch::channel(true);
        allocator.stopping = Some(stopping);
        allocator
            .destroy_or_confirm_absent(&run, RunRuntimeExit::ForceStopped)
            .await
            .assert_value();
        assert!(
            accepted.exists(),
            "a preparation stop still permits recovery"
        );
        allocator.stopping = None;
        if managed {
            let lease_path =
                crate::native_v2_cli::local::local_workspace_lease_path(&state, &workspace)
                    .assert_value();
            let competing =
                ControllerLease::acquire_workspace(&lease_path, &workspace).assert_value();
            assert!(
                allocator
                    .destroy_or_confirm_absent(&run, RunRuntimeExit::ForceStopped)
                    .await
                    .is_err()
            );
            assert!(accepted.exists());
            assert!(workspace.join("output").exists());
            drop(competing);
        }
        allocator
            .destroy_or_confirm_absent(&run, RunRuntimeExit::ForceStopped)
            .await
            .assert_value();
        assert!(!accepted.exists());
        assert!(files.file_input_roots(&run).assert_value().is_empty());
        assert_eq!(std::fs::read(original).assert_value(), b"user input");
        assert_eq!(workspace.exists(), !managed);
        if !managed {
            assert_eq!(
                std::fs::read(workspace.join("output")).assert_value(),
                b"live output"
            );
        }
    }
}
