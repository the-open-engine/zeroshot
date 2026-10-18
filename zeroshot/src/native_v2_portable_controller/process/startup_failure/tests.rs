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
    assert!(report.len() <= MAX_STARTUP_FAILURE_BYTES);
    assert!(report.starts_with("portable controller I/O failed: xxx"));
}

#[test]
fn startup_failure_ignores_oversized_or_absent_reports() {
    let (_root, paths) = storage();
    assert_eq!(take_startup_failure(&paths), None);
    write_private_new_file(
        &paths.startup_failure(),
        &vec![b'x'; MAX_STARTUP_FAILURE_BYTES + 1],
    )
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
        "en_US.UTF-8",
        "C.UTF-8",
        "enabled",
        "git-user",
    ] {
        assert!(!redactions.iter().any(|value| value == public), "{public}");
    }
}
