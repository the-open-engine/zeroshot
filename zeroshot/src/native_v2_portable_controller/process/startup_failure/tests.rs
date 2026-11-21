use std::collections::BTreeMap;
use std::io;

use openengine_cluster_protocol::RunId;
use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

fn storage() -> (tempfile::TempDir, PortableControllerPaths) {
    let root = tempfile::tempdir().assert_value();
    let paths = PortableControllerPaths::new(root.path());
    (root, paths)
}

#[test]
fn startup_failure_is_redacted_control_safe_and_consumed_once() {
    let (_root, paths) = storage();
    let error = PortableControllerError::Io(io::Error::new(
        io::ErrorKind::NotFound,
        "restic rejected token-secret-value\u{1b}[31m with\0bytes\nand\tlines",
    ));

    record_startup_failure(&paths, &error, &["token-secret-value".to_owned()]);

    let report = take_startup_failure(&paths).assert_value();
    assert_eq!(
        report,
        "portable controller I/O failed: restic rejected [REDACTED] [31m with\u{fffd}bytes and lines"
    );
    assert!(!paths.startup_failure().exists());
    assert_eq!(take_startup_failure(&paths), None);
}

#[test]
fn startup_failure_keeps_its_first_cause_and_stays_bounded() {
    let (_root, paths) = storage();
    let first = PortableControllerError::Io(io::Error::other("x".repeat(64 * 1024)));
    record_startup_failure(&paths, &first, &[]);
    record_startup_failure(&paths, &PortableControllerError::Bootstrap, &[]);

    let report = take_startup_failure(&paths).assert_value();
    assert!(report.len() <= MAX_FAILURE_BYTES);
    assert!(report.starts_with("portable controller I/O failed: xxx"));
}

#[test]
fn startup_failure_ignores_oversized_or_absent_reports() {
    let (_root, paths) = storage();
    assert_eq!(take_startup_failure(&paths), None);
    write_private_new_file(&paths.startup_failure(), &vec![b'x'; MAX_FAILURE_BYTES + 1])
        .assert_value();
    assert_eq!(take_startup_failure(&paths), None);
    assert!(!paths.startup_failure().exists());
}

#[test]
fn startup_redactions_cover_bootstrap_credentials_but_not_ordinary_context() {
    let root = tempfile::tempdir().assert_value();
    let mut bootstrap = crate::native_v2_portable_controller::tests::bootstrap(
        RunId::new("run-startup-redactions"),
        crate::native_v2_portable_controller::tests::submission("startup-redactions"),
        root.path().join("workspace"),
        root.path().join("storage"),
    );
    bootstrap.github_token = Some("github-token-value".to_owned());
    bootstrap.git_credentials = Some(GitTransportCredentials::Https {
        username: "git-user".to_owned(),
        password: "git-password-value".to_owned(),
    });
    // The local bootstrap carries the whole invoking shell.
    bootstrap.native_environment = BTreeMap::from([
        (
            "ANTHROPIC_API_KEY".to_owned(),
            "native-key-value".to_owned(),
        ),
        ("OPENAI_BASE_URL".to_owned(), "https://u:p@x".to_owned()),
        ("GH_TOKEN".to_owned(), "shell-github-token".to_owned()),
        (
            "AWS_SECRET_ACCESS_KEY".to_owned(),
            "shell-aws-secret".to_owned(),
        ),
        (
            "UNFAMILIAR_SETTING".to_owned(),
            "unfamiliar-value".to_owned(),
        ),
        ("Home".to_owned(), "/Users/someone".to_owned()),
        ("PWD".to_owned(), "/Users/someone/project".to_owned()),
        (
            "SSL_CERT_FILE".to_owned(),
            "/private-ca/missing.pem".to_owned(),
        ),
        ("SSL_CERT_DIR".to_owned(), "/private-ca/certs".to_owned()),
        ("LC_ALL".to_owned(), "en_US.UTF-8".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("FEATURE_FLAG".to_owned(), "enabled".to_owned()),
    ]);

    let redactions = startup_redactions(&bootstrap);

    for secret in [
        "github-token-value",
        "git-password-value",
        "native-key-value",
        "https://u:p@x",
        "shell-github-token",
        "shell-aws-secret",
        "unfamiliar-value",
    ] {
        assert!(redactions.iter().any(|value| value == secret), "{secret}");
    }
    for public in [
        "/Users/someone",
        "/Users/someone/project",
        "/private-ca/missing.pem",
        "/private-ca/certs",
        "en_US.UTF-8",
        "C.UTF-8",
        "enabled",
        "git-user",
    ] {
        assert!(!redactions.iter().any(|value| value == public), "{public}");
    }
}

#[tokio::test]
async fn runtime_failure_keeps_the_first_cause_private_bounded_and_read_only() {
    let (_root, paths) = storage();
    let recorder = Arc::new(LocalRuntimeFailureReport::new(
        paths.clone(),
        vec!["runtime-secret".to_owned()],
    ));
    recorder.record(&format!(
        "original runtime-secret\u{1b}[31m\0\n\t{}",
        "é".repeat(16 * 1024)
    ));
    recorder.record("later cleanup failure");
    recorder.wait_written().await;
    let report = read_runtime_failure(&paths).assert_value();
    assert!(
        report.starts_with("original [REDACTED] [31m\u{fffd}  "),
        "{report}"
    );
    assert!(!report.contains("runtime-secret"));
    assert!(!report.contains("cleanup"));
    assert!(!report.chars().any(char::is_control));
    let bytes = std::fs::read(paths.runtime_failure()).assert_value();
    assert!(bytes.len() <= MAX_FAILURE_BYTES);
    let before = std::fs::metadata(paths.runtime_failure()).assert_value();
    for _ in 0..2 {
        assert_eq!(
            read_runtime_failure(&paths).as_deref(),
            Some(report.as_str())
        );
    }
    let after = std::fs::metadata(paths.runtime_failure()).assert_value();
    assert_eq!(std::fs::read(paths.runtime_failure()).assert_value(), bytes);
    assert_eq!(
        before.modified().assert_value(),
        after.modified().assert_value()
    );
    assert_eq!(before.permissions(), after.permissions());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(after.permissions().mode() & 0o777, 0o600);
    }
    assert!(!paths.startup_failure().exists());
}

#[test]
fn runtime_failure_ignores_unreadable_or_invalid_notes_without_consuming_them() {
    let (_root, paths) = storage();
    assert_eq!(read_runtime_failure(&paths), None);
    // A failed report must leave an existing path alone.
    std::fs::create_dir(paths.runtime_failure()).assert_value();
    write_failure(&paths.runtime_failure(), "cannot be written", &[]);
    assert_eq!(read_runtime_failure(&paths), None);
    assert!(paths.runtime_failure().is_dir());
    std::fs::remove_dir(paths.runtime_failure()).assert_value();
    for bytes in [vec![0xff], vec![b'x'; MAX_FAILURE_BYTES + 1]] {
        write_private_new_file(&paths.runtime_failure(), &bytes).assert_value();
        assert_eq!(read_runtime_failure(&paths), None);
        assert_eq!(std::fs::read(paths.runtime_failure()).assert_value(), bytes);
        std::fs::remove_file(paths.runtime_failure()).assert_value();
    }
}

#[cfg(unix)]
#[test]
fn runtime_failure_refuses_redirected_and_nonprivate_notes() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (root, paths) = storage();
    let external = root.path().join("external");
    write_private_new_file(&external, b"external note").assert_value();
    symlink(&external, paths.runtime_failure()).assert_value();
    write_failure(&paths.runtime_failure(), "must not replace a symlink", &[]);
    assert_eq!(read_runtime_failure(&paths), None);
    assert_eq!(std::fs::read(&external).assert_value(), b"external note");
    assert!(
        std::fs::symlink_metadata(paths.runtime_failure())
            .assert_value()
            .is_symlink()
    );
    std::fs::remove_file(paths.runtime_failure()).assert_value();
    write_private_new_file(&paths.runtime_failure(), b"private note").assert_value();
    std::fs::set_permissions(
        paths.runtime_failure(),
        std::fs::Permissions::from_mode(0o644),
    )
    .assert_value();
    assert_eq!(read_runtime_failure(&paths), None);
    assert_eq!(
        std::fs::read(paths.runtime_failure()).assert_value(),
        b"private note"
    );
}
