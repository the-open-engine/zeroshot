use super::*;

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
    fs::copy(&restic, &original).assert_value();
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
    fs::copy(&restic, &replacement).assert_value();
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
        fs::remove_file(&original).assert_value();
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
