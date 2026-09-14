use openengine_cluster_testkit::assertions::AssertValue;
use openengine_cluster_testkit::fixture::TemporaryDirectory;

use super::*;

#[test]
fn api_payload_budget_and_log_tail_are_bounded() {
    let payload = vec![b'x'; 512 * 1024];
    assert_eq!(
        validate_api_output(payload).assert_value().len(),
        512 * 1024
    );
    assert_eq!(
        validate_api_output(vec![b'x'; MAX_API_OUTPUT_BYTES + 1]),
        Err(GitHubAuthorityError::Rejected)
    );

    let mut output = b"discard".to_vec();
    output.extend(vec![b'x'; MAX_CHECK_LOG_TAIL_BYTES]);
    output.extend_from_slice(b"failure at end");
    let tail = check_log_tail(&output);
    assert_eq!(tail.len(), MAX_CHECK_LOG_TAIL_BYTES);
    assert!(!tail.contains("discard"));
    assert!(tail.ends_with("failure at end"));
}

#[test]
fn api_error_parser_preserves_bounded_provider_status_and_reason() {
    let error = github_api_error(
        br#"gh: Validation Failed (HTTP 422)
{
  "message":"Validation Failed",
  "errors":[{
    "resource":"PullRequest",
    "field":"head",
    "code":"invalid",
    "message":"Head sha can't be blank"
  }],
  "status":"422"
}
"#,
    );
    assert!(error.to_string().contains("Head sha can't be blank"));
    assert!(error.to_string().contains("Validation Failed (HTTP 422)"));
    assert!(error.retryable_review_sync());

    let unauthorized = GitHubAuthorityError::api(Some(401), "HTTP 401: Bad credentials");
    assert!(!unauthorized.retryable_review_sync());
    assert!(unauthorized.authentication_failed());
}

#[test]
fn unfamiliar_errors_and_rate_limits_remain_repairable() {
    let error = github_api_error(b"gh: unusual remote refusal (HTTP 403)\nPlease try again later.");
    assert!(!error.authentication_failed());
    assert!(error.retryable_review_sync());
    assert!(error.to_string().contains("unusual remote refusal"));
    assert!(error.to_string().contains("Please try again later."));
}

fn authority(program: PathBuf, deadline: Duration) -> GhCliDeliveryAuthority {
    let home = program.parent().assert_value().to_path_buf();
    GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
        gh_program: program,
        api_deadline: deadline,
        ..GhCliAuthorityConfig::hosted(home)
    })
}

fn spawn_error(program: &std::path::Path) -> String {
    std::process::Command::new(program)
        .spawn()
        .err()
        .assert_value()
        .to_string()
}

#[tokio::test]
async fn missing_api_executable_preserves_os_error_and_redacts_command() {
    let root = TemporaryDirectory::for_test("github-api");
    let program = root.as_path().join("missing-test-token-gh");
    let expected = spawn_error(&program);
    let error = authority(program, Duration::from_secs(1))
        .api_output(
            &["repos/acme/project/pulls".to_owned()],
            GitHubCredential("test-token"),
        )
        .await
        .err()
        .assert_value();
    let diagnostic = error.to_string();
    assert!(diagnostic.contains("could not start command:"));
    assert!(diagnostic.contains(&expected));
    assert!(diagnostic.contains("missing-[REDACTED]-gh"));
    assert!(diagnostic.contains("repos/acme/project/pulls"));
    assert!(!diagnostic.contains("test-token"));
    assert_eq!(error.api_status(), None);
    assert!(error.retryable_review_sync());
    assert!(!error.authentication_failed());
}

#[cfg(unix)]
fn script(root: &std::path::Path, source: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let program = root.join("gh-fixture");
    std::fs::write(&program, format!("#!/bin/sh\n{source}")).assert_value();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).assert_value();
    program
}

#[cfg(unix)]
#[tokio::test]
async fn nonexecutable_api_program_preserves_permission_error() {
    use std::os::unix::fs::PermissionsExt;

    let root = TemporaryDirectory::for_test("github-api");
    let program = script(root.as_path(), "exit 0\n");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o644)).assert_value();
    let expected = spawn_error(&program);
    let error = authority(program, Duration::from_secs(1))
        .api_output(&[], GitHubCredential("test-token"))
        .await
        .err()
        .assert_value();
    assert!(error.to_string().contains("could not start command:"));
    assert!(error.to_string().contains(&expected));
    assert!(error.retryable_review_sync());
}

#[cfg(unix)]
#[tokio::test]
async fn interrupted_api_preserves_redacted_partial_streams_without_claiming_auth_refusal() {
    let root = TemporaryDirectory::for_test("github-api");
    let program = script(
        root.as_path(),
        r#"printf 'partial stdout %s\n' "$GH_TOKEN"
printf 'gh: partial response (HTTP 401) %s\npartial secret: test-to' "$GH_TOKEN" >&2
exec /bin/sleep 30
"#,
    );
    let error = authority(program, Duration::from_secs(1))
        .api_output(&[], GitHubCredential("test-token"))
        .await
        .err()
        .assert_value();
    let diagnostic = error.to_string();
    assert!(diagnostic.contains("command timed out after 1000 milliseconds"));
    assert!(diagnostic.contains("stdout (truncated=true):\npartial stdout [REDACTED]"));
    assert!(
        diagnostic.contains("stderr (truncated=true):\ngh: partial response (HTTP 401) [REDACTED]")
    );
    assert!(diagnostic.contains("partial secret:"));
    assert!(!diagnostic.contains("test-to"));
    assert_eq!(error.api_status(), None);
    assert!(error.retryable_review_sync());
    assert!(!error.authentication_failed());
}

#[cfg(unix)]
#[tokio::test]
async fn failed_api_reserves_diagnostic_space_for_both_streams_and_retains_http_status() {
    let root = TemporaryDirectory::for_test("github-api");
    let mut stdout = b"stdout retained: ".to_vec();
    stdout.resize(MAX_API_DIAGNOSTIC_STREAM_BYTES - 4, b'x');
    stdout.extend_from_slice(b"test-token");
    stdout.resize(MAX_API_OUTPUT_BYTES + 1, b'x');
    let mut stderr = b"gh: fixture failure (HTTP 401)\nstderr retained: test-token\n".to_vec();
    let encoded = encode_basic_credential("test-token");
    stderr.extend_from_slice(format!("encoded credential: {encoded}\n").as_bytes());
    stderr.extend(vec![b'y'; MAX_API_ERROR_BYTES + 1]);
    std::fs::write(root.as_path().join("stdout"), stdout).assert_value();
    std::fs::write(root.as_path().join("stderr"), stderr).assert_value();
    let program = script(
        root.as_path(),
        "/bin/cat \"$HOME/stdout\"\n/bin/cat \"$HOME/stderr\" >&2\nexit 7\n",
    );
    let error = authority(program, Duration::from_secs(10))
        .api_output(&[], GitHubCredential("test-token"))
        .await
        .err()
        .assert_value();
    let diagnostic = error.to_string();
    assert!(diagnostic.contains("exitStatus: Some(7)"));
    assert!(diagnostic.contains("stdout (truncated=true):\nstdout retained:"));
    assert!(diagnostic.contains("stderr (truncated=true):\ngh: fixture failure (HTTP 401)"));
    assert!(diagnostic.contains("stderr retained: [REDACTED]"));
    assert!(diagnostic.contains("encoded credential: [REDACTED]"));
    assert!(!diagnostic.contains("test-"));
    assert!(!diagnostic.contains(&encoded));
    assert!(diagnostic.len() < 32 * 1024);
    assert_eq!(error.api_status(), Some(401));
    assert!(!error.retryable_review_sync());
    assert!(error.authentication_failed());
}

#[cfg(unix)]
#[tokio::test]
async fn successful_api_retains_large_raw_response_without_redacting_or_truncating_it() {
    let root = TemporaryDirectory::for_test("github-api");
    let mut expected = b"raw response: test-token\0\xff".to_vec();
    expected.extend(vec![b'x'; 512 * 1024]);
    std::fs::write(root.as_path().join("payload"), &expected).assert_value();
    let program = script(root.as_path(), "exec /bin/cat \"$HOME/payload\"\n");
    let output = authority(program, Duration::from_secs(10))
        .api_output(&[], GitHubCredential("test-token"))
        .await
        .assert_value();
    assert_eq!(output, expected);
}

struct FailingReader(&'static [u8]);

impl AsyncRead for FailingReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if self.0.is_empty() {
            return std::task::Poll::Ready(Err(std::io::Error::other(
                "fixture read failure: test-token",
            )));
        }
        let count = self.0.len().min(buffer.remaining());
        buffer.put_slice(&self.0[..count]);
        self.0 = &self.0[count..];
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn api_pipe_failure_preserves_stream_and_partial_output_with_redaction() {
    for stream in ["stdout", "stderr"] {
        let mut output = ApiOutput::default();
        let bytes = if stream == "stdout" {
            &mut output.stdout
        } else {
            &mut output.stderr
        };
        let error = collect_bounded(
            FailingReader(b"retained output: test-token\npartial secret: test-to"),
            bytes,
            MAX_API_ERROR_BYTES,
            stream,
        )
        .await
        .err()
        .assert_value();
        let failure = output.failure(None, &error, GitHubCredential("test-token"));
        let diagnostic = failure.to_string();
        assert!(diagnostic.contains(&format!(
            "could not read command {stream}: fixture read failure: [REDACTED]"
        )));
        assert!(diagnostic.contains(&format!(
            "{stream} (truncated=true):\nretained output: [REDACTED]"
        )));
        assert!(diagnostic.contains("partial secret:"));
        assert!(!diagnostic.contains("test-to"));
        assert!(failure.retryable_review_sync());
        assert!(!failure.authentication_failed());
    }
}

#[test]
fn failed_step_output_survives_large_github_cleanup_logs() {
    let mut output =
        b"setup\nassertion: expected 400, received 202\n2026-09-14 ##[error]exit code 1\n".to_vec();
    output.extend_from_slice(&vec![b'x'; MAX_CHECK_LOG_TAIL_BYTES * 2]);
    let excerpt = check_log_tail(&output);
    assert!(excerpt.contains("assertion: expected 400, received 202"));
    assert!(excerpt.ends_with("##[error]exit code 1\n"));
    assert!(!excerpt.contains("xxxx"));
}

#[test]
fn last_github_error_is_retained_without_rewriting_its_output() {
    let output = b"first ##[error]earlier failure\nraw final failure\nlast ##[error]later failure";
    assert_eq!(check_log_tail(output).as_bytes(), output);
}

#[cfg(unix)]
#[tokio::test]
async fn job_log_capture_opts_in_and_feedback_removes_terminal_controls() {
    let root = TemporaryDirectory::for_test("github-job-log");
    let expected = concat!(
        "\x1b[31massertion failed: expected 400, got 202\x1b[0m\n",
        "\x1b]8;;https://example.invalid\x07link\x1b]8;;\x07\n"
    )
    .as_bytes();
    std::fs::write(root.as_path().join("payload"), expected).assert_value();
    let program = script(
        root.as_path(),
        r#"case "$*" in
  'api repos/acme/project/actions/jobs/91/logs --method GET --allow-escape-sequences') ;;
  *) printf '%s\n' 'the response contains terminal escape sequences' >&2; exit 1 ;;
esac
exec /bin/cat "$HOME/payload"
"#,
    );
    let output = authority(program, Duration::from_secs(10))
        .job_log_output("acme/project", 91, GitHubCredential("test-token"))
        .await
        .assert_value();
    assert_eq!(output, expected);
    let mut snapshot = PolicySnapshot {
        state: GitHubReviewState::Open {
            checks: GitHubChecks::Failed {
                diagnostic: "Required CI checks failed".to_owned(),
            },
        },
        failed_job_ids: vec![91],
        merge_method: None,
        head_update: None,
    };
    include_check_logs(&mut snapshot, &[(91, check_log_tail(&output))]);
    let GitHubReviewState::Open {
        checks: GitHubChecks::Failed { diagnostic },
    } = snapshot.state
    else {
        panic!("expected failed required checks");
    };
    assert!(diagnostic.contains("GitHub Actions job 91 log excerpt"));
    assert!(diagnostic.contains("assertion failed: expected 400, got 202"));
    assert!(
        diagnostic
            .chars()
            .all(|character| !character.is_control() || matches!(character, '\n' | '\t'))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn job_log_capture_supports_older_gh_without_the_opt_in_flag() {
    let root = TemporaryDirectory::for_test("github-job-log");
    let program = script(
        root.as_path(),
        r#"printf 'call\n' >> "$HOME/calls"
case "$*" in
  *--allow-escape-sequences*) printf '%s\n' 'unknown flag: --allow-escape-sequences' >&2; exit 1 ;;
esac
printf 'older CLI failure details\n'
"#,
    );
    let output = authority(program, Duration::from_secs(10))
        .job_log_output("acme/project", 91, GitHubCredential("test-token"))
        .await
        .assert_value();
    assert_eq!(output, b"older CLI failure details\n");
    assert_eq!(
        std::fs::read_to_string(root.as_path().join("calls")).assert_value(),
        "call\ncall\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn job_log_capture_does_not_retry_unrelated_api_failures() {
    let root = TemporaryDirectory::for_test("github-job-log");
    let program = script(
        root.as_path(),
        r#"printf 'call\n' >> "$HOME/calls"
printf 'gh: permission denied (HTTP 403)\n' >&2
exit 1
"#,
    );
    let error = authority(program, Duration::from_secs(10))
        .job_log_output("acme/project", 91, GitHubCredential("test-token"))
        .await
        .err()
        .assert_value();
    assert_eq!(error.api_status(), Some(403));
    assert!(error.to_string().contains("permission denied"));
    assert_eq!(
        std::fs::read_to_string(root.as_path().join("calls")).assert_value(),
        "call\n"
    );
}
