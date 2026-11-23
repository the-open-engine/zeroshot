use openengine_cluster_testkit::assertions::AssertValue;
use tokio::io::AsyncWriteExt;

use super::*;

#[cfg(unix)]
const DISCOVERY_PROBE_MODE: &str = "ZEROSHOT_RESTIC_TEST_DISCOVERY";
#[cfg(unix)]
const DISCOVERY_PROBE_SENTINEL: &str = "zeroshot-restic-discovery-probe-ran";

#[cfg(unix)]
fn failing_restic(root: &Path) -> PathBuf {
    let executable = root.join("restic");
    openengine_cluster_testkit::fixture::write_executable(
        &executable,
        "#!/bin/sh\nprintf '%s' 'private child diagnostic' >&2\nexit 19\n",
        0o700,
    )
    .assert_value_with("write failing Restic fixture");
    executable
}

#[cfg(unix)]
fn run_discovery_probe(configured: Option<&Path>, search_path: &Path, expected: Option<&Path>) {
    let mut command = std::process::Command::new(std::env::current_exe().assert_value());
    command
        .args([
            "--exact",
            "native_v2_supervisor::checkpoints::restic::tests::restic_discovery_probe",
            "--ignored",
            "--nocapture",
        ])
        .env(DISCOVERY_PROBE_MODE, "1")
        .env("PATH", search_path)
        .env_remove("ZEROSHOT_RESTIC");
    if let Some(configured) = configured {
        command.env("ZEROSHOT_RESTIC", configured);
    }
    if let Some(expected) = expected {
        command.env("ZEROSHOT_RESTIC_TEST_EXPECTED", expected);
    } else {
        command.env_remove("ZEROSHOT_RESTIC_TEST_EXPECTED");
    }
    let output = command.output().assert_value();
    assert!(
        output.status.success(),
        "discovery probe failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(DISCOVERY_PROBE_SENTINEL),
        "discovery probe filter matched no test:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[cfg(unix)]
#[test]
#[ignore = "executed in an isolated subprocess by restic_discovery_and_failure_contracts"]
fn restic_discovery_probe() {
    assert_eq!(std::env::var(DISCOVERY_PROBE_MODE).as_deref(), Ok("1"));
    match std::env::var_os("ZEROSHOT_RESTIC_TEST_EXPECTED") {
        Some(expected) => assert_eq!(
            ResticProgram::discover().assert_value().executable,
            std::fs::canonicalize(expected).assert_value()
        ),
        None => assert_eq!(
            ResticProgram::discover().unwrap_err().kind(),
            io::ErrorKind::NotFound
        ),
    }
    println!("{DISCOVERY_PROBE_SENTINEL}");
}

#[cfg(unix)]
#[tokio::test]
async fn restic_discovery_and_child_failures_are_bounded_and_sanitized() {
    let root = tempfile::tempdir().assert_value();
    let executable = failing_restic(root.path());
    let empty_path = root.path().join("empty-path");
    std::fs::create_dir(&empty_path).assert_value();

    run_discovery_probe(Some(&executable), &empty_path, Some(executable.as_path()));
    run_discovery_probe(None, root.path(), Some(executable.as_path()));
    run_discovery_probe(None, &empty_path, None);

    let repository = Repository::new(
        ResticProgram::test(executable, Vec::new()).assert_value(),
        root.path().join("state"),
    );
    let error = repository
        .run(root.path(), &arguments(["cat"]))
        .await
        .unwrap_err();
    assert!(error.to_string().starts_with("restic cat failed ("));
    assert!(error.to_string().contains("19"));
    assert!(!error.to_string().contains("private child diagnostic"));
    let report = saved_report(&repository, &error);
    assert!(report.contains("private child diagnostic"));

    std::fs::remove_file(&repository.program.executable).assert_value();
    let error = repository
        .run(root.path(), &arguments(["cat"]))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert_eq!(error.to_string(), "restic cat start failed (NotFound)");
}

fn saved_report(repository: &Repository, error: &io::Error) -> String {
    let error = error.to_string();
    let location = error
        .split_once("; private diagnostic: ")
        .expect("failure must name its private diagnostic")
        .1
        .strip_suffix(" in checkpoint repository")
        .unwrap();
    let path = Path::new(location);
    assert!(path.is_relative());
    assert_eq!(path.parent(), Some(Path::new("diagnostics")));
    let path = repository.root.join(path);
    let mut file = platform::private_file(&path, FileAccess::Read).assert_value();
    let mut report = String::new();
    file.read_to_string(&mut report).assert_value();
    assert!(report.len() <= MAX_STDERR_BYTES);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    report
}

#[tokio::test]
async fn private_stderr_report_drains_bounds_redacts_and_sanitizes() {
    // Force an overlapping, short ambient redaction in an isolated process; never mutate the
    // test runner's environment while other checkpoint tests are using it.
    const PROBE: &str = "ZEROSHOT_RESTIC_PRIVATE_REPORT_PROBE";
    if std::env::var(PROBE).as_deref() != Ok("1") {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native_v2_supervisor::checkpoints::restic::tests::private_stderr_report_drains_bounds_redacts_and_sanitizes",
                "--nocapture",
            ])
            .env(PROBE, "1")
            .env("NODE_USE_ENV_PROXY", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let root = tempfile::tempdir().assert_value();
    let repository = Repository::new(
        ResticProgram::fake(root.path()).assert_value(),
        root.path().join("state"),
    )
    .with_redactions(vec!["bootstrap-only-credential".to_owned()]);
    repository.prepare_paths().assert_value();
    std::fs::write(
        repository.password_path(),
        format!("{}\n", "a1".repeat(PASSWORD_BYTES)),
    )
    .unwrap();
    let password = std::fs::read_to_string(repository.password_path()).assert_value();
    let password = password.trim();
    let mut bytes = vec![b'x'; MAX_STDERR_BYTES * 1_000];
    bytes.extend_from_slice(
        format!("\nTHE REASON: {password} bootstrap-only-credential\x1b[31m\r\0\n").as_bytes(),
    );
    let expected_tail = bytes[bytes.len() - MAX_STDERR_BYTES..].to_vec();
    let mut tail = TailBuffer::new(MAX_STDERR_BYTES);
    retain_stderr(std::io::Cursor::new(bytes), &mut tail)
        .await
        .assert_value();
    assert_eq!(tail.snapshot().bytes, expected_tail);
    assert!(tail.snapshot().truncated);
    let path = repository
        .save_failure(&tail)
        .await
        .expect("private report saved");
    let error = io::Error::other(format!(
        "failed; private diagnostic: {} in checkpoint repository",
        path.display()
    ));
    let report = saved_report(&repository, &error);
    assert!(report.contains("THE REASON: [REDACTED]"));
    assert!(!report.contains(password));
    assert!(!report.contains("bootstrap-only-credential"));
    assert!(
        !report
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\t'))
    );

    // A retained tail can start in the middle of a credential.
    let mut tail = TailBuffer::new(MAX_STDERR_BYTES);
    tail.append(password.as_bytes());
    tail.append(&vec![b'x'; MAX_STDERR_BYTES - password.len() + 7]);
    let path = repository.save_failure(&tail).await.unwrap();
    let report = std::fs::read_to_string(repository.root.join(path)).unwrap();
    assert!(!report.contains(password));
    assert!(
        report.starts_with("stderr (truncated tail): [REDACTED]"),
        "unexpected redacted framing: {:?}",
        report.chars().take(100).collect::<String>()
    );
    assert!(!report.contains(&password[7..]));
}

#[cfg(unix)]
#[tokio::test]
async fn exit_three_retains_cause_and_failed_diagnostic_write_preserves_failure() {
    for writable in [true, false] {
        let root = tempfile::tempdir().assert_value();
        let repository = scripted_repository(
            root.path(),
            "#!/bin/sh\nprintf '%s' 'recognizable Restic cause' >&2\nexit 3\n",
        );
        repository.prepare_paths().assert_value();
        if !writable {
            std::fs::write(repository.root.join("diagnostics"), "blocked").unwrap();
        }
        let error = repository
            .run(root.path(), &arguments(["backup"]))
            .await
            .unwrap_err();
        let public = error.to_string();
        assert!(public.starts_with("restic backup failed (exit status: 3)"));
        assert!(!public.contains("recognizable Restic cause"));
        assert!(!public.contains(&repository.root.display().to_string()));
        if writable {
            assert!(saved_report(&repository, &error).contains("recognizable Restic cause"));
        } else {
            assert_eq!(public, "restic backup failed (exit status: 3)");
        }
    }
}

#[cfg(unix)]
#[test]
fn private_failure_report_waits_for_blocking_pool_without_stalling_async_work() {
    let root = tempfile::tempdir().assert_value();
    let repository = scripted_repository(
        root.path(),
        "#!/bin/sh\nprintf '%s' 'deferred private cause' >&2\nexit 3\n",
    );
    repository.prepare_paths().assert_value();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .assert_value();
    let (entered, ready) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    let blocker = runtime.spawn_blocking(move || {
        entered.send(()).unwrap();
        let _ = held.recv_timeout(Duration::from_secs(10));
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let (deferred, error) = runtime.block_on(async {
        let args = arguments(["backup"]);
        let failure = repository.run(root.path(), &args);
        tokio::pin!(failure);
        // The only blocking worker is occupied. The async timer must still run while the
        // report waits for that worker, and the original operation must finish after release.
        let result = tokio::time::timeout(Duration::from_secs(1), &mut failure).await;
        let deferred = result.is_err();
        release.send(()).unwrap();
        let error = match result {
            Ok(result) => result.unwrap_err(),
            Err(_) => failure.await.unwrap_err(),
        };
        blocker.await.unwrap();
        (deferred, error)
    });
    assert!(deferred, "private report bypassed the blocking pool");
    assert!(
        error
            .to_string()
            .starts_with("restic backup failed (exit status: 3)")
    );
    assert!(!error.to_string().contains("deferred private cause"));
    assert!(saved_report(&repository, &error).contains("deferred private cause"));
}

#[test]
fn late_stop_skips_optional_private_report_and_allows_discard() {
    let root = tempfile::tempdir().assert_value();
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let repository = Repository::new(
        ResticProgram::fake(root.path()).assert_value(),
        root.path().join("state"),
    )
    .with_stop_request(Some(stopping));
    repository.prepare_paths().assert_value();
    let mut tail = TailBuffer::new(MAX_STDERR_BYTES);
    tail.append(b"late-stop cause");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .assert_value();
    let (entered, ready) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    let blocker = runtime.spawn_blocking(move || {
        entered.send(()).unwrap();
        let _ = held.recv_timeout(Duration::from_secs(10));
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let skipped_before_release = runtime.block_on(async {
        let saving = repository.save_failure(&tail);
        tokio::pin!(saving);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut saving)
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_dir(repository.root.join("diagnostics"))
                .unwrap()
                .count(),
            1
        );
        stop.send_replace(true);
        let after_stop = tokio::time::timeout(Duration::from_millis(300), &mut saving).await;
        let skipped = matches!(&after_stop, Ok(None));
        if skipped {
            assert_eq!(
                std::fs::read_dir(repository.root.join("diagnostics"))
                    .unwrap()
                    .count(),
                0
            );
            super::super::discard_lineage(&root.path().join("catalog"), &repository.root).unwrap();
            assert!(!repository.root.exists());
        }
        release.send(()).unwrap();
        if after_stop.is_err() {
            let _ = saving.await;
        }
        blocker.await.unwrap();
        tokio::task::spawn_blocking(|| {}).await.unwrap();
        skipped
    });
    assert!(
        skipped_before_release,
        "late stop waited for the optional report writer"
    );
    assert!(
        !repository.root.exists(),
        "late report recreated discarded storage"
    );
}

#[cfg(unix)]
#[test]
fn late_stop_preserves_restic_failure_after_confirmed_child_cleanup() {
    let root = tempfile::tempdir().assert_value();
    let pid_path = root.path().join("pid");
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let repository = scripted_repository(
        root.path(),
        &format!(
            "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nprintf '%s' 'late-stop cause' >&2\nexit 3\n",
            pid_path.display(),
        ),
    )
    .with_stop_request(Some(stopping));
    repository.prepare_paths().assert_value();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .assert_value();
    let (entered, ready) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    let blocker = runtime.spawn_blocking(move || {
        entered.send(()).unwrap();
        let _ = held.recv_timeout(Duration::from_secs(10));
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let (completed_before_release, error) = runtime.block_on(async {
        let args = arguments(["backup"]);
        let failure = repository.run(root.path(), &args);
        tokio::pin!(failure);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), &mut failure)
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_dir(repository.root.join("diagnostics"))
                .unwrap()
                .count(),
            1
        );
        let pid: libc::pid_t = std::fs::read_to_string(&pid_path).unwrap().parse().unwrap();
        assert_ne!(
            unsafe { libc::kill(pid, 0) },
            0,
            "reporting started before child cleanup"
        );
        stop.send_replace(true);
        let after_stop = tokio::time::timeout(Duration::from_millis(300), &mut failure).await;
        let completed = after_stop.is_ok();
        release.send(()).unwrap();
        let error = match after_stop {
            Ok(result) => result.unwrap_err(),
            Err(_) => failure.await.unwrap_err(),
        };
        blocker.await.unwrap();
        tokio::task::spawn_blocking(|| {}).await.unwrap();
        (completed, error)
    });
    assert!(
        completed_before_release,
        "late stop waited for the optional report writer"
    );
    assert_eq!(error.to_string(), "restic backup failed (exit status: 3)");
    assert!(!super::super::CheckpointError(error).was_stopped());
    assert_eq!(
        std::fs::read_dir(repository.root.join("diagnostics"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn private_failure_report_freezes_password_before_deferred_write() {
    let root = tempfile::tempdir().assert_value();
    let repository = Repository::new(
        ResticProgram::fake(root.path()).assert_value(),
        root.path().join("state"),
    );
    repository.prepare_paths().assert_value();
    // Avoid incidental masking by the one-character ambient redaction used in CI.
    let password = "az".repeat(PASSWORD_BYTES);
    std::fs::write(repository.password_path(), &password).assert_value();
    let mut tail = TailBuffer::new(MAX_STDERR_BYTES);
    tail.append(format!("frozen repository credential: {password}").as_bytes());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .assert_value();
    let (entered, ready) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    let blocker = runtime.spawn_blocking(move || {
        entered.send(()).unwrap();
        let _ = held.recv_timeout(Duration::from_secs(10));
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let path = runtime.block_on(async {
        let saving = repository.save_failure(&tail);
        tokio::pin!(saving);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut saving)
                .await
                .is_err()
        );
        std::fs::remove_file(repository.password_path()).unwrap();
        release.send(()).unwrap();
        let path = saving.await.expect("private report saved");
        blocker.await.unwrap();
        path
    });
    let error = io::Error::other(format!(
        "failed; private diagnostic: {} in checkpoint repository",
        path.display()
    ));
    let report = saved_report(&repository, &error);
    assert!(report.contains("frozen repository credential: [REDACTED]"));
    assert!(!report.contains(&password));
}

#[test]
fn dropped_private_report_cannot_recreate_discarded_lineage() {
    for keep_delivery in [false, true] {
        let root = tempfile::tempdir().assert_value();
        let repository = Repository::new(
            ResticProgram::fake(root.path()).assert_value(),
            root.path().join("state"),
        );
        repository.prepare_paths().assert_value();
        let delivery = repository.root.join("delivery-state.json");
        if keep_delivery {
            std::fs::write(&delivery, "retained delivery receipt").unwrap();
        }
        let mut tail = TailBuffer::new(MAX_STDERR_BYTES);
        tail.append(b"cancelled private report");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .assert_value();
        let (entered, ready) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel();
        let blocker = runtime.spawn_blocking(move || {
            entered.send(()).unwrap();
            let _ = held.recv_timeout(Duration::from_secs(10));
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        runtime.block_on(async {
            let report;
            {
                let saving = repository.save_failure(&tail);
                tokio::pin!(saving);
                assert!(
                    tokio::time::timeout(Duration::from_millis(200), &mut saving)
                        .await
                        .is_err()
                );
                let mut reports = std::fs::read_dir(repository.root.join("diagnostics")).unwrap();
                report = reports.next().unwrap().unwrap().path();
                assert!(reports.next().is_none());
                assert_eq!(std::fs::metadata(&report).unwrap().len(), 0);
            }
            assert!(
                !report.exists(),
                "dropped future retained its partial report"
            );
            let catalog = root.path().join("catalog");
            if keep_delivery {
                super::super::discard_workspace_lineage(&catalog, &repository.root).unwrap();
            } else {
                super::super::discard_lineage(&catalog, &repository.root).unwrap();
                assert!(!repository.root.exists());
            }
            release.send(()).unwrap();
            blocker.await.unwrap();
            tokio::task::spawn_blocking(|| {}).await.unwrap();
        });
        assert!(!repository.root.join("diagnostics").exists());
        assert!(!repository.password_path().exists());
        if keep_delivery {
            assert_eq!(
                std::fs::read_to_string(delivery).unwrap(),
                "retained delivery receipt"
            );
            assert_eq!(std::fs::read_dir(&repository.root).unwrap().count(), 1);
        } else {
            assert!(
                !repository.root.exists(),
                "deferred write recreated disposed storage"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn dropped_restore_report_cannot_recreate_discarded_lineage() {
    use super::super::{
        CheckpointRestore, CheckpointRestoreSelection, ResticCheckpointStore,
        ResticCheckpointStoreTestConfig, RunCheckpointStore, discard_lineage, restore,
    };

    let root = tempfile::tempdir().assert_value();
    let failure = root.path().join("fail");
    let repository = scripted_repository(
        root.path(),
        &format!(
            "#!/bin/sh\nif [ -f \"$1\" ]; then /bin/cat \"$RESTIC_PASSWORD_FILE\" >&2; printf '%s' 'delayed restore cause' >&2; exit 3; fi\nprintf '%s\\n' '{{\"message_type\":\"summary\",\"snapshot_id\":\"{}\"}}'\n",
            "a".repeat(64),
        ),
    );
    let program = ResticProgram::test(
        repository.program.executable.clone(),
        vec![failure.to_str().unwrap().to_owned()],
    )
    .unwrap();
    let catalog = root.path().join("catalog");
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let store = ResticCheckpointStore::with_program(ResticCheckpointStoreTestConfig {
        directory: catalog.clone(),
        repository: repository.root.clone(),
        workspace: workspace.clone(),
        writers: Default::default(),
        program,
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .assert_value();
    runtime.block_on(store.finish(&[])).unwrap();
    std::fs::write(failure, "fail").unwrap();
    let (entered, ready) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    let blocker = runtime.spawn_blocking(move || {
        entered.send(()).unwrap();
        let _ = held.recv_timeout(Duration::from_secs(10));
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    runtime.block_on(async {
        let selection = CheckpointRestore {
            directory: catalog.clone(),
            selection: CheckpointRestoreSelection::Latest,
        };
        {
            let restoring = restore(&selection, &workspace);
            tokio::pin!(restoring);
            assert!(
                tokio::time::timeout(Duration::from_secs(1), &mut restoring)
                    .await
                    .is_err()
            );
            assert_eq!(
                std::fs::read_dir(repository.root.join("diagnostics"))
                    .unwrap()
                    .count(),
                1
            );
        }
        discard_lineage(&catalog, &repository.root).unwrap();
        assert!(!repository.root.exists());
        release.send(()).unwrap();
        blocker.await.unwrap();
        tokio::task::spawn_blocking(|| {}).await.unwrap();
    });
    assert!(!catalog.exists());
    assert!(
        !repository.root.exists(),
        "dropped restore recreated its lineage"
    );
}

#[test]
fn program_and_snapshot_validation_rejects_ambiguous_executables_and_identities() {
    let root = tempfile::tempdir().assert_value();
    let directory_error = ResticProgram::from_path(root.path().to_path_buf()).unwrap_err();
    assert_eq!(directory_error.kind(), io::ErrorKind::Other);
    assert_eq!(
        ResticProgram::from_path(root.path().join("missing"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );

    let executable = std::env::current_exe().assert_value();
    let program = ResticProgram::from_path(executable.clone()).assert_value();
    assert_eq!(
        program.executable,
        std::fs::canonicalize(executable).assert_value()
    );
    assert!(program.arguments.is_empty());

    let valid = "a".repeat(64);
    assert_eq!(
        SnapshotId::parse(valid.clone()).assert_value().as_str(),
        valid
    );
    for invalid in ["a".repeat(63), "A".repeat(64), "g".repeat(64)] {
        assert_eq!(
            SnapshotId::parse(invalid).unwrap_err().kind(),
            io::ErrorKind::Other
        );
    }
}

#[test]
fn repository_rejects_an_unsafe_password_entry() {
    let root = tempfile::tempdir().assert_value();
    let repository = Repository::new(
        ResticProgram::fake(root.path()).assert_value(),
        root.path().join("restic"),
    );

    std::fs::create_dir_all(repository.password_path()).assert_value();
    assert!(repository.ensure_password().is_err());
}

#[tokio::test]
async fn summary_capture_drains_progress_and_retains_only_the_latest_summary() {
    let expected = "b".repeat(64);
    let first = "a".repeat(64);
    let summary = format!("{{\"message_type\":\"summary\",\"snapshot_id\":\"{expected}\"}}");
    let mut output =
        format!("{{\"message_type\":\"summary\",\"snapshot_id\":\"{first}\"}}\n").into_bytes();
    // One overlong progress record and aggregate progress above the old 2 MiB cap.
    output.extend(std::iter::repeat_n(b'x', MAX_RECORD_BYTES + 1));
    output.push(b'\n');
    output.extend_from_slice(summary.as_bytes());
    output.push(b'\n');
    for _ in 0..100_000 {
        output.extend_from_slice(b"{\"message_type\":\"status\",\"seconds_elapsed\":0}\n");
    }
    let retained = operation_output(std::io::Cursor::new(output), true)
        .await
        .assert_value();
    assert_eq!(retained, summary.as_bytes());
    assert_eq!(backup_snapshot(&retained).assert_value().as_str(), expected);
    assert!(
        operation_output(std::io::Cursor::new(vec![0; MAX_RECORD_BYTES + 1]), false)
            .await
            .assert_value()
            .is_empty()
    );
    discard_output(std::io::Cursor::new(vec![0; MAX_RECORD_BYTES + 1]))
        .await
        .assert_value();
}

#[tokio::test]
async fn summary_capture_accepts_chunk_boundaries_and_requires_valid_snapshot_identity() {
    let (mut writer, reader) = tokio::io::duplex(32);
    let expected = "b".repeat(64);
    let summary = format!("{{\"message_type\":\"summary\",\"snapshot_id\":\"{expected}\"}}");
    let write = async {
        for byte in summary.as_bytes() {
            writer.write_all(&[*byte]).await.assert_value();
        }
        writer.shutdown().await.assert_value();
    };
    let ((), retained) = tokio::join!(write, operation_output(reader, true));
    assert_eq!(
        backup_snapshot(&retained.assert_value())
            .assert_value()
            .as_str(),
        expected
    );
    for output in [
        b"not-json\n".as_slice(),
        br#"{"message_type":"status"}"#.as_slice(),
        br#"{"message_type":"summary","snapshot_id":"bad"}"#.as_slice(),
    ] {
        let retained = operation_output(std::io::Cursor::new(output), true)
            .await
            .assert_value();
        assert!(backup_snapshot(&retained).is_err());
    }
}

#[test]
fn pipe_and_start_failures_keep_stage_and_kind_without_private_error_text() {
    for stage in ["start", "stdout read", "stderr read", "wait"] {
        let error = operation_io_error(
            "backup",
            stage,
            io::Error::new(io::ErrorKind::BrokenPipe, "private repository credential"),
        );
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(
            error.to_string(),
            format!("restic backup {stage} failed (BrokenPipe)")
        );
        assert!(!error.to_string().contains("private"));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_child_does_not_leave_a_detached_helper_running() {
    let root = tempfile::tempdir().assert_value();
    let pid = root.path().join("helper-pid");
    let script = format!(
        "#!/bin/sh\n/bin/sleep 60 </dev/null >/dev/null 2>&1 &\nprintf '%s' \"$!\" > '{}'\nexit 1\n",
        pid.display()
    );
    let repository = scripted_repository(root.path(), &script);
    let error = repository
        .run_with_timeout(root.path(), &arguments(["backup"]), Duration::from_secs(30))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "restic backup failed (exit status: 1)");
    let helper: libc::pid_t = std::fs::read_to_string(pid)
        .assert_value()
        .parse()
        .assert_value();
    assert_ne!(
        unsafe { libc::kill(helper, 0) },
        0,
        "the helper of a failed Restic is still running"
    );
}

#[tokio::test]
async fn a_preparation_deadline_replaces_the_per_operation_limit() {
    let root = tempfile::tempdir().assert_value();
    let program = ResticProgram::fake(root.path()).assert_value();
    let repository = Repository::new(program.clone(), root.path().to_owned());
    assert_eq!(repository.operation_timeout(), OPERATION_TIMEOUT);

    let generous = Repository::new(program.clone(), root.path().to_owned())
        .with_deadline(Instant::now() + Duration::from_secs(3_600));
    assert!(generous.operation_timeout() > Duration::from_secs(3_500));
    let expired = Repository::new(program, root.path().to_owned()).with_deadline(Instant::now());
    assert_eq!(expired.operation_timeout(), Duration::ZERO);
}

#[cfg(unix)]
fn scripted_repository(root: &Path, script: &str) -> Repository {
    let executable = root.join("restic");
    openengine_cluster_testkit::fixture::write_executable(&executable, script, 0o700)
        .assert_value();
    Repository::new(
        ResticProgram::test(executable, Vec::new()).assert_value(),
        root.join("state"),
    )
}

#[cfg(unix)]
#[tokio::test]
async fn real_child_large_progress_and_stderr_cannot_hide_committed_summary() {
    let root = tempfile::tempdir().assert_value();
    let id = "c".repeat(64);
    let script = format!(
        r#"#!/bin/sh
exec /usr/bin/awk 'BEGIN {{
  for (i = 0; i < 60000; i++) print "{{\"message_type\":\"status\",\"bytes_done\":123456789}}";
  print "{{\"message_type\":\"summary\",\"snapshot_id\":\"{id}\"}}";
  for (i = 0; i < 60000; i++) {{
    print "{{\"message_type\":\"status\",\"bytes_done\":123456789}}";
    print "private repository credential and child diagnostic" > "/dev/stderr";
  }}
}}'
"#
    );
    let repository = scripted_repository(root.path(), &script);
    let result = repository.backup(root.path(), None).await.assert_value();
    assert_eq!(result.as_str(), id);
    assert!(!repository.root.join("diagnostics").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn timed_out_child_is_reaped_and_timeout_does_not_leak_stderr() {
    let root = tempfile::tempdir().assert_value();
    let pid = root.path().join("pid");
    let script = format!(
        "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nprintf '%s' 'private timeout credential' >&2\nexec /bin/sleep 60\n",
        pid.display()
    );
    let repository = scripted_repository(root.path(), &script);
    let error = repository
        .run_with_timeout(
            root.path(),
            &arguments(["backup"]),
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(
        error
            .to_string()
            .starts_with("restic backup timed out; private diagnostic: ")
    );
    assert!(!error.to_string().contains("private timeout credential"));
    assert!(saved_report(&repository, &error).contains("private timeout credential"));
    let pid = std::fs::read_to_string(pid).assert_value();
    let result = std::process::Command::new("/bin/kill")
        .args(["-0", &pid])
        .status()
        .assert_value();
    assert!(
        !result.success(),
        "Restic child still running after timeout"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_child_keeps_confirmed_stop_and_drains_its_tree() {
    let root = tempfile::tempdir().assert_value();
    let pid_path = root.path().join("pid");
    let script = format!(
        "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nprintf '%s' 'private cancellation cause' >&2\nexec /bin/sleep 60\n",
        pid_path.display()
    );
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let repository = scripted_repository(root.path(), &script).with_stop_request(Some(stopping));
    let args = arguments(["backup"]);
    let operation = repository.run_with_timeout(root.path(), &args, Duration::from_secs(5));
    tokio::pin!(operation);
    tokio::select! {
        result = &mut operation => panic!("operation ended before cancellation: {result:?}"),
        () = async {
            while !pid_path.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        } => {}
    }
    stop.send_replace(true);
    let error = super::super::CheckpointError(operation.await.unwrap_err());
    assert!(error.was_stopped());
    assert_eq!(
        error.to_string(),
        "workspace checkpoint is unavailable: controller is stopping"
    );
    let pid: libc::pid_t = std::fs::read_to_string(pid_path).unwrap().parse().unwrap();
    assert_ne!(unsafe { libc::kill(pid, 0) }, 0);
}

#[cfg(unix)]
#[tokio::test]
async fn exit_three_keeps_private_cause_regression() {
    let root = tempfile::tempdir().assert_value();
    let repository = scripted_repository(
        root.path(),
        "#!/bin/sh\nprintf '%s' 'recognizable Restic cause' >&2\nexit 3\n",
    );
    let error = repository
        .run(root.path(), &arguments(["backup"]))
        .await
        .unwrap_err();
    let public = error.to_string();
    assert!(public.starts_with("restic backup failed (exit status: 3)"));
    assert!(!public.contains("recognizable Restic cause"));
    assert!(
        public.contains("private diagnostic: diagnostics/"),
        "{public}"
    );
    let reports = std::fs::read_dir(repository.root.join("diagnostics"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(reports.len(), 1);
    let report = std::fs::read_to_string(reports[0].path()).unwrap();
    assert!(report.len() <= 4096);
    assert!(report.contains("recognizable Restic cause"));
}
