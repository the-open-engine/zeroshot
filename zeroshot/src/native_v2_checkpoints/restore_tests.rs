use super::*;
use openengine_cluster_testkit::assertions::AssertError;

const PROBE: &str =
    "native_v2_supervisor::checkpoints::tests::restore_tests::restore_after_restic_move_probe";

#[test]
fn restore_rediscovers_restic_after_the_install_moves() {
    let executable = std::env::current_exe().assert_value();
    let restic_name = if cfg!(windows) {
        "restic.exe"
    } else {
        "restic"
    };
    let restic = std::env::var_os("ZEROSHOT_RESTIC_TEST")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            executable
                .parent()
                .assert_value()
                .parent()
                .assert_value()
                .join(restic_name)
        });
    assert!(
        restic.is_file(),
        "prepare pinned Restic before running restore tests"
    );
    let root = tempfile::tempdir().assert_value();
    for mode in [
        "configured",
        "sibling",
        "path",
        "override",
        "fallback",
        "missing",
    ] {
        let directory = root.path().join(mode);
        fs::create_dir(&directory).assert_value();
        // Isolate current_exe's sibling search as well as the subprocess environment.
        let probe = directory.join(executable.file_name().assert_value());
        #[cfg(unix)]
        openengine_cluster_testkit::fixture::write_executable(
            &probe,
            fs::read(&executable).assert_value(),
            0o700,
        )
        .assert_value();
        #[cfg(windows)]
        fs::copy(&executable, &probe).assert_value();
        let search = directory.join("path");
        fs::create_dir(&search).assert_value();
        let mut command = std::process::Command::new(probe);
        command
            .args(["--exact", PROBE, "--ignored", "--nocapture"])
            .env("ZEROSHOT_RESTORE_TEST_MODE", mode)
            .env("ZEROSHOT_RESTIC_TEST", &restic)
            .env("PATH", &search)
            .env_remove("ZEROSHOT_RESTIC");
        if matches!(mode, "configured" | "override") {
            command.env(
                "ZEROSHOT_RESTIC",
                directory.join(format!("selected{}", std::env::consts::EXE_SUFFIX)),
            );
        }
        let output = command.output().assert_value();
        assert!(
            output.status.success(),
            "{mode}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("restore probe completed"));
        fs::remove_file(directory.join(executable.file_name().assert_value())).assert_value();
    }
}

#[tokio::test]
#[ignore = "runs with isolated executable discovery in restore_rediscovers_restic_after_the_install_moves"]
async fn restore_after_restic_move_probe() {
    let mode = std::env::var("ZEROSHOT_RESTORE_TEST_MODE").assert_value();
    let executable = std::env::current_exe().assert_value();
    let root = executable.parent().assert_value();
    let original = root.join(format!("original-restic{}", std::env::consts::EXE_SUFFIX));
    let restic = std::env::var_os("ZEROSHOT_RESTIC_TEST").assert_value();
    let name = if cfg!(windows) {
        "restic.exe"
    } else {
        "restic"
    };
    let replacement = match mode.as_str() {
        "sibling" => root.join(name),
        "path" => root.join("path").join(name),
        _ => root.join(format!("selected{}", std::env::consts::EXE_SUFFIX)),
    };
    for path in [&original, &replacement] {
        #[cfg(unix)]
        openengine_cluster_testkit::fixture::write_executable(
            path,
            fs::read(&restic).assert_value(),
            0o700,
        )
        .assert_value();
        #[cfg(windows)]
        fs::copy(&restic, path).assert_value();
    }
    let directory = root.join("checkpoints");
    let workspace = root.join("workspace");
    fs::create_dir(&workspace).assert_value();
    fs::write(workspace.join("source"), "initial").assert_value();
    // Deserialize the exact production/legacy shape, without test-program exemptions.
    let program = serde_json::from_value(json!({"executable":original})).assert_value();
    let store = ResticCheckpointStore::with_program(ResticCheckpointStoreTestConfig {
        directory: directory.clone(),
        repository: root.join("repository"),
        workspace: workspace.clone(),
        writers: BTreeSet::from([NodeName::new("writer").assert_value()]),
        program,
    });
    let conversations = root.join("conversation-source");
    fs::create_dir(&conversations).assert_value();
    fs::write(conversations.join("session.json"), "conversation bytes").assert_value();
    store
        .retain_conversations(Some(ConversationFiles::new(conversations)))
        .await
        .assert_value();
    store.enter(&boundary("writer"), &[]).await.assert_value();
    fs::write(workspace.join("source"), "checkpoint bytes").assert_value();
    store
        .enter(&boundary("reader"), &[settled("writer", 1)])
        .await
        .assert_value();
    let points = list(
        &directory,
        RunCheckpointsParams {
            run_id: RunId::new("restore-test"),
            after: None,
            limit: None,
        },
    )
    .assert_value()
    .checkpoints;
    let storage_path = directory.join("storage.json");
    let storage = fs::read(&storage_path).assert_value();
    let expected = serde_json::to_vec(&json!({
        "format":1, "repository":root.join("repository"), "program":{"executable":original, "inProcessTestFake":false}
    })).assert_value();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&storage).assert_value(),
        serde_json::from_slice::<serde_json::Value>(&expected).assert_value()
    );
    if mode != "fallback" {
        remove_used_executable(&original);
    }
    if mode == "override" {
        // If restore wrongly runs the still-existing stored program, process startup fails.
        fs::write(&original, "unusable stored executable").assert_value();
    }
    for selection in [
        CheckpointRestoreSelection::Latest,
        CheckpointRestoreSelection::Checkpoint {
            checkpoint_id: points[1].checkpoint_id.clone(),
        },
    ] {
        let selected = matches!(selection, CheckpointRestoreSelection::Checkpoint { .. });
        let selection = CheckpointRestore {
            directory: directory.clone(),
            selection,
        };
        fs::write(workspace.join("source"), "live edits").assert_value();
        pin(&selection).assert_value(); // Metadata validation must never require Restic.
        let result = restore(&selection, &workspace).await;
        if mode == "missing" {
            let error = result.unwrap_err().to_string();
            let stored_path = format!("{original:?}");
            for expected in [stored_path.as_str(), "ZEROSHOT_RESTIC", "beside", "PATH"] {
                assert!(error.contains(expected), "missing {expected:?}: {error}");
            }
            assert_eq!(
                fs::read_to_string(workspace.join("source")).assert_value(),
                "live edits"
            );
        } else {
            let seed = result.unwrap_or_else(|error| panic!("restore failed: {error}"));
            assert_eq!(seed.len(), usize::from(selected));
            if selected {
                assert_eq!(seed[0].execution.get(), 1);
                let conversations = restore_checkpoint_conversations(&selection, None)
                    .await
                    .assert_value();
                assert_eq!(
                    fs::read_to_string(conversations.directory().join("session.json"))
                        .assert_value(),
                    "conversation bytes"
                );
            }
            assert_eq!(
                fs::read_to_string(workspace.join("source")).assert_value(),
                "checkpoint bytes"
            );
        }
        assert_eq!(fs::read(&storage_path).assert_value(), storage);
    }
    if mode == "fallback" {
        let successor = root.join("successor");
        ResticCheckpointStore::new_with_fallback(
            ResticCheckpointStoreRequest {
                directory: successor.clone(),
                repository: root.join("repository"),
                workspace,
                writers: BTreeSet::new(),
            },
            Some(&directory),
        )
        .assert_value()
        .enter(&boundary("reader"), &[])
        .await
        .assert_value();
        let retained: StorageConfiguration =
            catalog::read(&successor.join("storage.json")).assert_value();
        assert_eq!(
            serde_json::to_value(retained.program).assert_value()["executable"],
            json!(original)
        );
    }
    println!("restore probe completed");
}

fn remove_used_executable(path: &Path) {
    // Windows can briefly retain a used image after its process tree has exited. Wait only
    // for sharing/access errors; the old path must disappear before discovery is tested.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    remove_executable_with(|| fs::remove_file(path), deadline, cfg!(windows))
        .unwrap_or_else(|error| panic!("cannot remove used executable {path:?}: {error}"));
}

fn remove_executable_with(
    mut remove: impl FnMut() -> std::io::Result<()>,
    deadline: std::time::Instant,
    windows: bool,
) -> std::io::Result<()> {
    loop {
        match remove() {
            Err(error)
                if windows
                    && matches!(error.raw_os_error(), Some(5 | 32 | 33))
                    && std::time::Instant::now() < deadline =>
            {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                std::thread::sleep(remaining.min(std::time::Duration::from_millis(10)));
            }
            result => return result,
        }
    }
}

#[test]
fn restore_fixture_retries_windows_image_locks_before_removing_the_old_executable() {
    for code in [5, 32, 33] {
        let mut attempts = 0;
        remove_executable_with(
            || {
                attempts += 1;
                if attempts == 1 {
                    Err(std::io::Error::from_raw_os_error(code))
                } else {
                    Ok(())
                }
            },
            std::time::Instant::now() + std::time::Duration::from_secs(60),
            true,
        )
        .assert_value();
        assert_eq!(attempts, 2);
    }
}

#[test]
fn restore_fixture_preserves_permanent_expired_and_non_windows_removal_errors() {
    for (windows, code, expired) in [(true, 2, false), (true, 32, true), (false, 32, false)] {
        let mut attempts = 0;
        let now = std::time::Instant::now();
        let deadline = if expired {
            now
        } else {
            now + std::time::Duration::from_secs(60)
        };
        let error = remove_executable_with(
            || {
                attempts += 1;
                Err(std::io::Error::from_raw_os_error(code))
            },
            deadline,
            windows,
        )
        .assert_error();
        assert_eq!(error.raw_os_error(), Some(code));
        assert_eq!(attempts, 1);
    }
}

#[test]
fn restore_fixture_reports_the_final_removal_error_after_a_transient_lock() {
    let mut attempts = 0;
    let error = remove_executable_with(
        || {
            attempts += 1;
            Err(std::io::Error::from_raw_os_error(if attempts == 1 {
                32
            } else {
                2
            }))
        },
        std::time::Instant::now() + std::time::Duration::from_secs(60),
        true,
    )
    .assert_error();
    assert_eq!(error.raw_os_error(), Some(2));
    assert_eq!(attempts, 2);
}
