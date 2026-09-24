use openengine_cluster_testkit::assertions::AssertValue;
use tokio::io::AsyncWriteExt;

use super::*;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
const DISCOVERY_PROBE_MODE: &str = "ZEROSHOT_RESTIC_TEST_DISCOVERY";
#[cfg(unix)]
const DISCOVERY_PROBE_SENTINEL: &str = "zeroshot-restic-discovery-probe-ran";

#[cfg(unix)]
fn failing_restic(root: &Path) -> PathBuf {
    let executable = root.join("restic");
    std::fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s' 'private child diagnostic' >&2\nexit 19\n",
    )
    .assert_value();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).assert_value();
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
    assert_eq!(error.to_string(), "restic operation failed");
    assert!(!error.to_string().contains("private child diagnostic"));
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
async fn output_boundaries_accept_complete_data_and_reject_oversize_or_invalid_responses() {
    let (mut writer, reader) = tokio::io::duplex(32);
    writer.write_all(b"restic output").await.assert_value();
    writer.shutdown().await.assert_value();

    assert_eq!(
        bounded_output(reader).await.assert_value(),
        b"restic output"
    );
    assert_eq!(
        bounded_output(std::io::Cursor::new(vec![0; MAX_OUTPUT_BYTES as usize + 1]))
            .await
            .unwrap_err()
            .kind(),
        io::ErrorKind::Other
    );

    assert_eq!(
        backup_snapshot(b"not-json\n").unwrap_err().kind(),
        io::ErrorKind::Other
    );
    assert_eq!(
        backup_snapshot(br#"{"message_type":"status"}"#)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Other
    );
    let expected = "b".repeat(64);
    let output = format!(
        "{{\"message_type\":\"status\"}}\n{{\"message_type\":\"summary\",\"snapshot_id\":\"{expected}\"}}\n"
    );
    assert_eq!(
        backup_snapshot(output.as_bytes()).assert_value().as_str(),
        expected
    );
}
