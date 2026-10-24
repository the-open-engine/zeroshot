use super::*;

#[cfg(unix)]
#[tokio::test]
async fn private_input_and_early_output_drain_concurrently() {
    let mut command = Command::new("/usr/bin/python3");
    command.args(["-c", "import sys; sys.stdout.write('x'*262144); sys.stdout.flush(); data=sys.stdin.buffer.read(); print(len(data))"]);
    let input = vec![b'p'; 262144];
    let output = capture_sensitive_with_input(&mut command, Duration::from_secs(5), &[], &input)
        .await
        .unwrap()
        .require_success()
        .unwrap();
    assert!(!output.stdout_truncated());
    assert!(output.stdout().starts_with(&"x".repeat(262144)));
    assert!(output.stdout().ends_with("262144\n"));
    assert!(output.diagnostic().stdout_truncated);
}

fn failure(stderr: impl Into<String>) -> GitCommandFailure {
    GitCommandFailure {
        command: "git fetch".into(),
        working_directory: "/workspace".into(),
        exit_status: Some(1),
        stdout: String::new(),
        stderr: stderr.into(),
        stdout_truncated: false,
        stderr_truncated: false,
        context: Box::<str>::default(),
        timed_out: false,
        cleanup_unconfirmed: false,
        classification: None,
    }
}

fn http_failure(status: u16) -> GitCommandFailure {
    failure(format!(
        "fatal: unable to access 'https://github.com/acme/project': The requested URL returned error: {status}"
    ))
}

#[test]
fn truncated_output_cannot_retain_a_partial_secret() {
    let captured = CapturedBytes {
        bytes: b"safe credential-val".to_vec(),
        truncated: true,
    };
    let (text, truncated) = sanitize(captured, &["credential-value".to_owned()]);
    assert_eq!(text, "safe ");
    assert!(truncated);
}

#[test]
fn overlapping_secret_values_are_redacted_as_complete_values() {
    let username = "git-user";
    let password = "git-user-secret-canary";
    let output = format!("username={username} password={password}");
    for secrets in [
        [username.to_owned(), password.to_owned()],
        [password.to_owned(), username.to_owned()],
    ] {
        let sanitized = sanitize(
            CapturedBytes {
                bytes: output.as_bytes().to_vec(),
                truncated: false,
            },
            &secrets,
        );
        assert_eq!(
            sanitized,
            ("username=[REDACTED] password=[REDACTED]".to_owned(), false),
            "secret insertion order must not expose part of a longer value"
        );
    }
}

#[test]
fn authentication_classification_accepts_only_http_credentials_failures() {
    assert!(http_failure(401).authentication_failed());
    for credential in ["Username", "Password"] {
        assert!(
            failure(format!(
                "fatal: could not read {credential} for 'https://github.com': terminal prompts disabled"
            ))
            .authentication_failed()
        );
    }
    for stderr in [
        "fatal: Authentication failed for 'https://github.com/acme/project'",
        "fatal: could not read Username for 'ssh://github.com': terminal prompts disabled",
        "fatal: could not read Username for 'https://github.com': another reason",
        "remote: Permission to acme/project denied",
    ] {
        assert_eq!(
            failure(stderr).authentication_failed(),
            stderr.starts_with("fatal: Authentication failed"),
            "{stderr}"
        );
    }
}

#[test]
fn retryable_transport_classification_is_narrow_and_complete() {
    for status in [429, 500, 503, 599] {
        assert!(http_failure(status).retryable_transport(), "HTTP {status}");
    }
    for status in [400, 401, 422, 600] {
        assert!(!http_failure(status).retryable_transport(), "HTTP {status}");
    }
    for message in [
        "Could not resolve host: github.com",
        "Failed to connect to github.com port 443",
        "Connection timed out",
        "Recv failure: Connection reset by peer",
        "Empty reply from server",
    ] {
        assert!(
            failure(format!(
                "fatal: unable to access 'https://github.com/acme/project': {message}"
            ))
            .retryable_transport(),
            "{message}"
        );
    }
    assert!(!failure("fatal: repository not found").retryable_transport());
    assert!(!failure("Could not resolve host: github.com").retryable_transport());

    let mut timed_out = failure("");
    timed_out.timed_out = true;
    assert!(timed_out.retryable_transport());
}

#[test]
fn success_and_operator_diagnostics_preserve_bounded_command_context() {
    let mut succeeded = failure("warning");
    succeeded.exit_status = Some(0);
    succeeded.stderr_truncated = true;
    succeeded.context = "trusted delivery".into();
    let diagnostic = succeeded.operator_stderr();
    assert!(diagnostic.contains("stderr (truncated):\nwarning"));
    assert!(diagnostic.contains("command: git fetch"));
    assert!(diagnostic.contains("workingDirectory: /workspace"));
    assert!(diagnostic.contains("trusted delivery"));
    assert!(succeeded.require_success().is_ok());

    let contextual = super::super::SourceDeliveryError::from(failure("repository refused"))
        .with_context("pushing managed branch");
    assert!(matches!(
        &contextual,
        super::super::SourceDeliveryError::Command(_)
    ));
    assert!(contextual.to_string().contains("repository refused"));
    assert!(contextual.to_string().contains("pushing managed branch"));
    assert!(!contextual.retryable_operation());
}

#[cfg(unix)]
#[tokio::test]
async fn raw_failure_is_bounded_and_credentials_are_redacted() {
    let mut command = Command::new("/bin/sh");
    command
        .env_clear()
        .env("GH_TOKEN", "credential-value")
        .args([
            "-c",
            "printf 'unfamiliar error: %s' \"$GH_TOKEN\" >&2; printf 'output'; exit 17",
        ]);
    let output = capture(&mut command, Duration::from_secs(1))
        .await
        .expect("command exited");
    assert_eq!(output.exit_status, Some(17));
    assert_eq!(output.stdout, "output");
    assert_eq!(output.stderr, "unfamiliar error: [REDACTED]");
    assert!(output.require_success().is_err());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancellation_stops_the_owned_git_helper_process() {
    use crate::native_v2_candidate::test_support::TestGitRepository;
    let repository = TestGitRepository::delivery();
    let marker = repository.root.path().join("helper-pid");
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            "/usr/bin/sleep 30 & printf '%s' $! > \"$1\"; wait",
            "git-helper",
        ])
        .arg(&marker);
    let task = tokio::spawn(async move { capture(&mut command, Duration::from_secs(30)).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("helper started");
    let pid = std::fs::read_to_string(marker).expect("helper PID");
    task.abort();
    assert!(task.await.expect_err("capture cancelled").is_cancelled());
    assert_helper_stopped(&pid).await;
}

#[cfg(target_os = "linux")]
async fn assert_helper_stopped(pid: &str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let status = std::fs::read_to_string(format!("/proc/{pid}/stat"));
            if status
                .as_ref()
                .map_or(true, |text| text.split_whitespace().nth(2) == Some("Z"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("helper stopped after capture cancellation");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn completed_commands_cannot_leave_background_helpers_mutating_the_workspace() {
    for status in [0, 17] {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            &format!("/usr/bin/sleep 30 >/dev/null 2>&1 & printf '%s' $!; exit {status}"),
        ]);
        let output = capture(&mut command, Duration::from_secs(2))
            .await
            .expect("command exited");
        assert_eq!(output.exit_status, Some(status));
        assert_helper_stopped(&output.stdout).await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn helper_pipe_inheritance_preserves_the_main_exit_status() {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "(/usr/bin/sleep 2) & exit 17"]);
    let started = std::time::Instant::now();
    let outcome = capture(&mut command, Duration::from_secs(1))
        .await
        .expect("confirmed descendant cleanup must close inherited pipes");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(outcome.exit_status, Some(17));
    assert!(!outcome.timed_out);
    assert!(!outcome.cleanup_unconfirmed);
}

#[cfg(unix)]
#[tokio::test]
async fn sensitive_capture_keeps_protocol_stdout_private_and_errors_sanitized() {
    let stdout = "private-protocol-output-marker";
    let stderr = "private-protocol-error-marker";
    let secrets = [stdout.to_owned(), stderr.to_owned()];
    for status in [0, 17] {
        let mut command = Command::new("/bin/sh");
        command.env_clear().args([
            "-c",
            &format!("printf '{stdout}'; printf '{stderr}' >&2; exit {status}"),
        ]);
        let output = capture_sensitive(&mut command, Duration::from_secs(1), &secrets)
            .await
            .expect("fixture exits with confirmed cleanup");
        for diagnostic in [format!("{output}"), format!("{output:?}")] {
            assert!(!diagnostic.contains(stdout));
            assert!(!diagnostic.contains(stderr));
        }
        assert_eq!(output.diagnostic().stdout, "[REDACTED]");
        assert_eq!(output.diagnostic().stderr, "[REDACTED]");
        assert!(!output.stdout_truncated());
        if status == 0 {
            assert_eq!(
                output
                    .require_success()
                    .expect("successful protocol output")
                    .stdout(),
                stdout
            );
        } else {
            let error = output
                .require_success()
                .expect_err("failed command is diagnostic-only");
            assert_eq!(error.stdout, "[REDACTED]");
            assert_eq!(error.stderr, "[REDACTED]");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn redaction_cannot_change_authentication_http_or_retry_evidence() {
    for (stderr, secret, status, authentication, retryable) in [
        (
            "fatal: Authentication failed for 'https://forge.invalid/project'",
            "a",
            None,
            true,
            false,
        ),
        (
            "fatal: could not read Username for 'https://forge.invalid': terminal prompts disabled",
            "a",
            None,
            true,
            false,
        ),
        (
            "fatal: unable to access 'https://forge.invalid/project': The requested URL returned error: 401",
            "401",
            Some(401),
            true,
            false,
        ),
        (
            "fatal: unable to access 'https://forge.invalid/project': The requested URL returned error: 503",
            "503",
            Some(503),
            false,
            true,
        ),
        (
            "fatal: unable to access 'https://forge.invalid/project': Could not resolve host: forge.invalid",
            "a",
            None,
            false,
            true,
        ),
    ] {
        let mut command = Command::new("/bin/sh");
        command.env_clear().args([
            "-c",
            "printf '%s' \"$1\" >&2; exit 17",
            "classification-fixture",
            stderr,
        ]);
        let failure =
            capture_with_secrets(&mut command, Duration::from_secs(1), &[secret.to_owned()])
                .await
                .expect("fixture exits with confirmed cleanup")
                .require_success()
                .expect_err("fixture emits a Git failure");
        assert_eq!(failure.http_status(), status);
        assert_eq!(failure.authentication_failed(), authentication);
        assert_eq!(failure.retryable_transport(), retryable);
        assert!(failure.stderr.contains("[REDACTED]"));
        assert!(!failure.stderr.contains(secret));
        let mut unconfirmed = failure;
        unconfirmed.cleanup_unconfirmed = true;
        assert!(!unconfirmed.retryable_transport());
    }
}

#[test]
fn redaction_never_rescans_markers_or_duplicate_native_secret_aliases() {
    let aliases = vec!["E".to_owned(); 20];
    assert_eq!(redact("E", &aliases), "[REDACTED]");
    assert_eq!(
        redact(
            "value=E E",
            &["E".to_owned(), "REDACTED".to_owned(), "E".to_owned()],
        ),
        "value=[REDACTED] [REDACTED]"
    );
    assert_eq!(
        redact("value=é界 é", &["é".to_owned(), "é界".to_owned()]),
        "value=[REDACTED] [REDACTED]"
    );
    let (text, truncated) = sanitize(
        CapturedBytes {
            bytes: vec![b'E'; MAX_DIAGNOSTIC_BYTES],
            truncated: false,
        },
        &aliases,
    );
    assert!(truncated);
    assert_eq!(text.len(), MAX_DIAGNOSTIC_BYTES);
    assert!("[REDACTED]".repeat(MAX_DIAGNOSTIC_BYTES).starts_with(&text));
}
