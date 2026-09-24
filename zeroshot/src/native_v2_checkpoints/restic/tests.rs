use openengine_cluster_testkit::assertions::AssertValue;
use tokio::io::AsyncWriteExt;

use super::*;

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
    assert_eq!(program.executable, executable);
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
