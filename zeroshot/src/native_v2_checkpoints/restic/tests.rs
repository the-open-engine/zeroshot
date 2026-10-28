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

    std::fs::remove_file(&repository.program.executable).assert_value();
    let error = repository
        .run(root.path(), &arguments(["cat"]))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert_eq!(error.to_string(), "restic cat start failed (NotFound)");
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
    assert_eq!(error.to_string(), "restic backup timed out");
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
