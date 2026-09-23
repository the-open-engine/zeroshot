use super::*;

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
async fn timeout_includes_inherited_pipe_drain() {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "(/usr/bin/sleep 2) & exit 17"]);
    let started = std::time::Instant::now();
    let error = capture(&mut command, Duration::from_millis(20))
        .await
        .expect_err("deadline");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(error.context.contains("timed out"));
    assert!(error.stderr_truncated);
}
