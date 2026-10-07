use super::*;

fn shell(source: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .env_clear()
        .env("GH_TOKEN", "test-token")
        .args(["-c", source]);
    command
}

#[tokio::test]
async fn silent_status_success_requires_no_json_response() {
    command_status(
        shell("exit 0"),
        Duration::from_secs(2),
        GitHubCredential("test-token"),
    )
    .await
    .expect("silent GitHub command succeeded");
}

#[tokio::test]
async fn failed_status_commands_keep_native_http_and_transport_semantics() {
    for (message, status, retryable) in [
        (
            "HTTP 401: Bad credentials (https://api.github.com/graphql)",
            Some(401),
            false,
        ),
        (
            "HTTP 429: Too Many Requests (https://api.github.com/graphql)",
            Some(429),
            true,
        ),
        (
            "HTTP 503: Service Unavailable (https://api.github.com/graphql)",
            Some(503),
            true,
        ),
        (
            "HTTP 403: You have exceeded a secondary rate limit. (https://api.github.com/graphql)",
            Some(403),
            true,
        ),
        (
            "HTTP 403: Resource not accessible by integration (https://api.github.com/graphql)",
            Some(403),
            false,
        ),
        (
            "GraphQL: Merge commits are not allowed on this repository. (mergePullRequest)",
            None,
            false,
        ),
        ("error connecting to api.github.com", None, true),
        (
            "Post https://api.github.com/graphql: TLS handshake timeout",
            None,
            true,
        ),
    ] {
        let mut command = shell("printf '%s\n' \"$1\" \"$GH_TOKEN\" >&2; exit 1");
        command.args(["gh-fixture", message]);
        let error = command_status(
            command,
            Duration::from_secs(2),
            GitHubCredential("test-token"),
        )
        .await
        .expect_err("command failed");
        assert!(matches!(error, GitHubAuthorityError::Api(_)));
        assert_eq!(error.api_status(), status, "{message}");
        assert_eq!(error.retryable_operation(), retryable, "{message}");
        assert_eq!(
            error.authentication_failed(),
            status == Some(401),
            "{message}"
        );
        assert!(error.to_string().contains(message));
        assert!(error.to_string().contains("[REDACTED]"));
        assert!(!error.to_string().contains("test-token"));
    }
}

#[tokio::test]
async fn timed_out_status_does_not_claim_an_incomplete_authentication_response() {
    let command = shell("printf '%s\n' 'HTTP 401: Bad credentials' >&2; exec /bin/sleep 30");
    let error = command_status(
        command,
        Duration::from_millis(100),
        GitHubCredential("test-token"),
    )
    .await
    .expect_err("command timed out");
    assert!(matches!(error, GitHubAuthorityError::Api(_)));
    assert_eq!(error.api_status(), None);
    assert!(error.retryable_operation());
    assert!(!error.authentication_failed());
}

#[tokio::test]
async fn status_spawn_failure_stays_in_the_api_boundary() {
    let root = tempfile::tempdir().expect("temporary command directory");
    let command = Command::new(root.path().join("missing-test-token-gh"));
    let error = command_status(
        command,
        Duration::from_secs(2),
        GitHubCredential("test-token"),
    )
    .await
    .expect_err("missing executable");
    assert!(matches!(error, GitHubAuthorityError::Api(_)));
    assert!(
        error
            .to_string()
            .contains("could not start contained command")
    );
    assert!(error.to_string().contains("missing-[REDACTED]-gh"));
    assert!(!error.retryable_operation());
    assert!(!error.to_string().contains("test-token"));
}

#[cfg(target_os = "linux")]
fn helper_command(marker: &std::path::Path, ending: &str) -> Command {
    let mut command = shell(&format!(
        "/bin/sleep 30 >/dev/null 2>&1 & printf '%s' $! > \"$1\"; {ending}"
    ));
    command.arg("gh-helper").arg(marker);
    command
}

#[cfg(target_os = "linux")]
async fn wait_for_helper_state(ready: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut ticks = tokio::time::interval(Duration::from_millis(5));
        while !ready() {
            ticks.tick().await;
        }
    })
    .await
    .expect("helper reached the expected state");
}

#[cfg(target_os = "linux")]
async fn assert_helper_stopped(marker: &std::path::Path) {
    let pid = std::fs::read_to_string(marker).expect("helper PID");
    wait_for_helper_state(|| {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map_or(true, |text| text.split_whitespace().nth(2) == Some("Z"))
    })
    .await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn status_completion_failure_and_timeout_stop_owned_helpers() {
    for ending in ["exit 0", "exit 17", "wait"] {
        let root = tempfile::tempdir().expect("temporary command directory");
        let marker = root.path().join("helper-pid");
        let result = command_status(
            helper_command(&marker, ending),
            Duration::from_millis(250),
            GitHubCredential("test-token"),
        )
        .await;
        assert_eq!(result.is_ok(), ending == "exit 0");
        if ending == "wait" {
            assert!(result.expect_err("timed out").retryable_operation());
        }
        assert_helper_stopped(&marker).await;
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancelling_status_capture_stops_owned_helpers() {
    let root = tempfile::tempdir().expect("temporary command directory");
    let marker = root.path().join("helper-pid");
    let command = helper_command(&marker, "wait");
    let task = tokio::spawn(async move {
        command_status(
            command,
            Duration::from_secs(30),
            GitHubCredential("test-token"),
        )
        .await
    });
    wait_for_helper_state(|| {
        std::fs::read_to_string(&marker).is_ok_and(|pid| pid.parse::<u32>().is_ok())
    })
    .await;
    task.abort();
    assert!(task.await.expect_err("capture cancelled").is_cancelled());
    assert_helper_stopped(&marker).await;
}

#[tokio::test]
async fn truncated_status_diagnostics_are_bounded_and_do_not_claim_http_authority() {
    let root = tempfile::tempdir().expect("temporary command directory");
    let token = "test-token";
    let encoded = encode_basic_credential(token);
    let output = root.path().join("stderr");
    std::fs::write(
        &output,
        format!(
            "HTTP 401: Bad credentials (https://api.github.com/graphql)\n{token} {encoded}\n{}",
            "x".repeat(64 * 1024)
        ),
    )
    .expect("large stderr fixture");
    let mut command = shell("/bin/cat \"$1\" >&2; exit 1");
    command.arg("gh-fixture").arg(output);
    let error = command_status(command, Duration::from_secs(2), GitHubCredential(token))
        .await
        .expect_err("truncated failure");
    let diagnostic = error.to_string();
    assert_eq!(error.api_status(), None);
    assert!(!error.authentication_failed());
    assert!(!error.retryable_operation());
    assert!(diagnostic.contains("stderr (truncated)"));
    assert!(diagnostic.len() < 32 * 1024);
    assert!(!diagnostic.contains(token));
    assert!(!diagnostic.contains(&encoded));
}
