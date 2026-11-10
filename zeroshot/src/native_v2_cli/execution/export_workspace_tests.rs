use super::*;

#[test]
fn cancellation_while_disk_io_is_pending_never_publishes_and_cleans_the_stage() {
    use std::sync::{Arc, mpsc};
    use std::time::Duration;
    use tokio::sync::Notify;

    struct AfterChunk(Arc<Notify>);

    #[async_trait::async_trait]
    impl DetachSignal for AfterChunk {
        async fn wait(&mut self) {
            self.0.notified().await;
        }
    }

    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("workspace.tar");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    let (release, wait) = mpsc::channel::<()>();
    let (entered, started) = mpsc::channel();
    let blocker = runtime.spawn_blocking(move || {
        entered.send(()).unwrap();
        let _ = wait.recv();
    });
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let received = Arc::new(Notify::new());
    let notify = received.clone();
    let backend = crate::native_v2_cli::tests::support::FakeBackend::with_workspace_export(
        Box::pin(futures_util::stream::once(async move {
            notify.notify_one();
            Ok(b"archive".to_vec())
        })),
    );
    let command = RunExportWorkspaceCommand {
        target: "fixture".to_owned(),
        params: openengine_cluster_protocol::RunExportWorkspaceParams {
            run_id: openengine_cluster_protocol::RunId::new("run-export"),
            from_checkpoint: None,
        },
        output: destination.clone(),
    };
    let mut receipt = Vec::new();
    let result = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(2),
            execute(command, &backend, &mut AfterChunk(received), &mut receipt),
        )
        .await
    });
    // Cancellation must settle before the disk worker is released. Drain queued I/O before
    // checking directory entries: Windows finishes deleting an open file when its handles close.
    release.send(()).unwrap();
    runtime.block_on(blocker).unwrap();
    drop(runtime);
    let error = result.unwrap().expect_err("export must be cancelled");
    assert!(error.to_string().contains("workspace export cancelled"));
    assert!(receipt.is_empty());
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn publication_refuses_a_competing_directory() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("workspace.tar");
    let staged = stage(&destination).unwrap();
    std::fs::create_dir(&destination).unwrap();
    assert!(staged.persist_noclobber(&destination).is_err());
    assert!(destination.is_dir());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[cfg(windows)]
#[test]
fn publication_refuses_competing_existing_and_dangling_junctions() {
    for dangling in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("workspace.tar");
        let victim = root.path().join("victim");
        if !dangling {
            std::fs::create_dir(&victim).unwrap();
        }
        let staged = stage(&destination).unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&destination)
            .arg(&victim)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(staged.persist_noclobber(&destination).is_err());
        assert!(std::fs::symlink_metadata(&destination).is_ok());
        assert!(stage(&destination).is_err());
        assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".zeroshot-export-")
        }));
    }
}

#[cfg(unix)]
#[test]
fn publication_refuses_competing_existing_and_dangling_symlinks() {
    for dangling in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("workspace.tar");
        let victim = root.path().join("victim");
        if !dangling {
            std::fs::write(&victim, b"original").unwrap();
        }
        let staged = stage(&destination).unwrap();
        std::os::unix::fs::symlink(&victim, &destination).unwrap();
        assert!(staged.persist_noclobber(&destination).is_err());
        assert!(
            std::fs::symlink_metadata(&destination)
                .unwrap()
                .is_symlink()
        );
        if !dangling {
            assert_eq!(std::fs::read(victim).unwrap(), b"original");
        }
        assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".zeroshot-export-")
        }));
    }
}

#[test]
fn publication_preserves_privacy_and_refuses_a_racing_destination() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("workspace.tar");
    let mut staged = stage(&destination).unwrap();
    let temporary = staged.path().to_owned();
    crate::execution::platform::validate_private_file(staged.as_file()).unwrap();
    staged.write_all(b"archive").unwrap();
    std::fs::write(&destination, b"competitor").unwrap();
    assert!(staged.persist_noclobber(&destination).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"competitor");
    assert!(!temporary.exists());
    std::fs::remove_file(&destination).unwrap();
    let mut staged = stage(&destination).unwrap();
    staged.write_all(b"archive").unwrap();
    staged.persist_noclobber(&destination).unwrap();
    let file = std::fs::File::open(&destination).unwrap();
    crate::execution::platform::validate_private_file(&file).unwrap();
    assert_eq!(std::fs::read(destination).unwrap(), b"archive");
}
