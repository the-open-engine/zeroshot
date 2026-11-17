use openengine_cluster_testkit::assertions::AssertValue;
use openengine_cluster_testkit::fixture::TemporaryDirectory;

use super::*;
use crate::source_code_provider::adapters::core::ci_diagnostics::{
    MAX_FAILURE_FEEDBACK_CHARS, MAX_JOB_LOG_TAIL_BYTES, include_job_logs,
};

fn check_log_tail(output: &[u8]) -> JobLog {
    super::check_log_tail(output, SourceDeliveryCredential::token(""), &[])
}

#[test]
fn redaction_preserves_raw_annotation_boundaries_through_final_feedback() {
    use crate::native_v2_delivery::{command, git_auth};
    use crate::source_code_provider::SourceGitCredential;

    let raw = format!(
        "setup context\nassertion failed: useful causal detail\n\
         ##[error]supporting annotation\n{}##[error]Process completed with exit code 1.\n{}",
        "noise\n".repeat(24_000),
        "cleanup-must-disappear\n".repeat(20_000),
    );
    for (username, password, summary) in [
        (
            "ordinary-user",
            "separate-private-password",
            "Process completed with exit code 1.",
        ),
        (
            "error",
            "separate-private-password",
            "Process completed with exit code 1.",
        ),
        (
            "ordinary-user",
            "##[error]Process completed",
            "[REDACTED] with exit code 1.",
        ),
        (
            "ordinary-user",
            "exit code 1.\ncleanup-must-disappear",
            "Process completed with [REDACTED]",
        ),
    ] {
        let secrets = vec![
            username.to_owned(),
            password.to_owned(),
            git_auth::encode_user_password(username, password),
        ];
        for native in [false, true] {
            let credential = SourceDeliveryCredential::token("forge-token");
            let credential = if native {
                credential.with_diagnostic_secrets(&secrets)
            } else {
                credential.with_git(SourceGitCredential::Https { username, password })
            };
            let output = super::check_log_tail(raw.as_bytes(), credential, &[]);
            assert!(output.text.len() <= MAX_JOB_LOG_TAIL_BYTES);
            let excerpt = super::super::job_log_excerpt(Ok(output)).assert_value();
            for jobs in [1, 2] {
                let logs = (91..91 + jobs)
                    .map(|job| (job, excerpt.clone()))
                    .collect::<Vec<_>>();
                let mut feedback = "Required CI checks failed".to_owned();
                include_job_logs(&mut feedback, "GitHub Actions", &logs);
                let feedback = command::redact(&feedback, &secrets);
                assert!(feedback.chars().count() <= MAX_FAILURE_FEEDBACK_CHARS);
                assert!(feedback.contains("setup context"));
                assert!(feedback.contains("assertion failed: useful causal detail"));
                assert!(feedback.contains(summary));
                assert!(!feedback.contains("cleanup-must-disappear"));
                assert!(!feedback.contains(password));
            }
        }
    }
}

#[test]
fn credential_collisions_preserve_raw_failure_context_through_both_budgets() {
    use crate::native_v2_delivery::command::redact;
    use crate::source_code_provider::SourceGitCredential;

    for (username, marker) in [
        ("ordinary-user", "assertion failed: causal detail"),
        ("assertion", "assertion failed: causal detail"),
        ("error", "error: unresolved symbol"),
        ("panic", "panic: causal detail"),
        ("panicked", "thread case panicked at source.rs:64"),
        ("traceback", "Traceback (most recent call last)"),
        ("fatal", "fatal: unresolved symbol"),
    ] {
        let secrets = vec![username.to_owned()];
        for native in [false, true] {
            let credential = SourceDeliveryCredential::token("independent-forge-token");
            let credential = if native {
                credential.with_diagnostic_secrets(&secrets)
            } else {
                credential.with_git(SourceGitCredential::Https {
                    username,
                    password: "independent-git-password",
                })
            };
            for (noise, jobs) in [(24_000, 1), (24_000, 8), (8_000, 2)] {
                let logs = (91..91 + jobs)
                    .map(|job| {
                        let raw = format!(
                            "{}{}\nsetup-critical {job} 界🙂\n\u{001b}[31m{marker}\u{001b}[0m\n\
                             causal description {job}\n{}\n##[error]exit code 1\ncleanup sentinel",
                            // Passing negative-test errors must not displace a later assertion.
                            if username == "assertion" {
                                "error: expected fixture rejection\n"
                            } else {
                                ""
                            },
                            // Redaction and control removal both move the raw failure position.
                            format!("\u{001b}[32m{username} 界🙂\u{0000}\n").repeat(300),
                            "noise\n".repeat(noise),
                        );
                        assert_eq!(raw.len() > MAX_JOB_LOG_TAIL_BYTES, noise == 24_000);
                        let log = super::check_log_tail(raw.as_bytes(), credential, &[]);
                        assert!(log.text.len() <= MAX_JOB_LOG_TAIL_BYTES);
                        (job, log)
                    })
                    .collect::<Vec<_>>();
                let mut feedback = "Required CI checks failed".to_owned();
                include_job_logs(&mut feedback, "GitHub Actions", &logs);
                let feedback = redact(&feedback, &secrets);
                assert!(feedback.chars().count() <= MAX_FAILURE_FEEDBACK_CHARS);
                for (job, _) in logs {
                    assert!(
                        feedback.contains(&format!("setup-critical {job} 界🙂")),
                        "{username}"
                    );
                    assert!(
                        feedback.contains(&format!("causal description {job}")),
                        "{username}"
                    );
                }
                assert!(feedback.contains("exit code 1"));
                assert!(!feedback.contains("cleanup sentinel"));
                assert!(!feedback.contains(username));
                assert!(!feedback.contains('\u{fffd}'));
                assert!(
                    feedback
                        .chars()
                        .all(|c| !c.is_control() || matches!(c, '\n' | '\t'))
                );
            }
        }
    }
}

#[test]
fn unsupported_log_option_is_classified_before_redaction() {
    use crate::source_code_provider::SourceGitCredential;

    let message = "unknown flag: --allow-escape-sequences";
    for username in [
        "ordinary-user",
        "flag",
        "unknown",
        "--allow-escape-sequences",
    ] {
        let credential =
            SourceDeliveryCredential::token("forge-token").with_git(SourceGitCredential::Https {
                username,
                password: "private-password",
            });
        let output = ApiOutput {
            stderr: format!("{message}\n").into_bytes(),
            ..Default::default()
        };
        assert!(output.unsupported_escape_sequences());
        let error = output.failure(None, "download failed", credential);
        assert!(!error.authentication_failed());
        assert!(!error.retryable_operation());
        assert!(!error.to_string().contains(username));
    }
    for stderr in [
        "unknown flag: --other-option",
        "prefix unknown flag: --allow-escape-sequences",
        "unexpected EOF",
    ] {
        assert!(
            !ApiOutput {
                stderr: stderr.as_bytes().to_vec(),
                ..Default::default()
            }
            .unsupported_escape_sequences()
        );
    }
    assert!(
        !ApiOutput {
            stdout: message.as_bytes().to_vec(),
            ..Default::default()
        }
        .unsupported_escape_sequences()
    );
}

#[test]
fn api_payload_budget_and_log_tail_are_bounded() {
    let payload = vec![b'x'; 512 * 1024];
    assert_eq!(
        validate_api_output(payload).assert_value().len(),
        512 * 1024
    );
    assert_eq!(
        validate_api_output(vec![b'x'; MAX_API_OUTPUT_BYTES + 1]),
        Err(SourceDeliveryError::Rejected)
    );

    let mut output = b"discard".to_vec();
    output.extend(vec![b'x'; MAX_JOB_LOG_TAIL_BYTES]);
    output.extend_from_slice(b"failure at end");
    let tail = check_log_tail(&output);
    assert_eq!(tail.text.len(), MAX_JOB_LOG_TAIL_BYTES);
    assert!(!tail.text.contains("discard"));
    assert!(tail.text.ends_with("failure at end"));
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

    let unauthorized = SourceDeliveryError::api(Some(401), "HTTP 401: Bad credentials");
    assert!(!unauthorized.retryable_review_sync());
    assert!(unauthorized.authentication_failed());
}

#[test]
fn only_typed_transient_api_failures_are_retryable_during_review_sync() {
    let forbidden =
        github_api_error(b"gh: unusual remote refusal (HTTP 403)\nPlease try again later.");
    assert!(!forbidden.authentication_failed());
    assert!(!forbidden.retryable_review_sync());
    assert!(forbidden.to_string().contains("unusual remote refusal"));
    assert!(forbidden.to_string().contains("Please try again later."));

    let statusless = SourceDeliveryError::api(None, "GitHub returned an invalid response");
    assert!(!statusless.retryable_review_sync());
    assert!(statusless.clone().temporary().retryable_review_sync());
    assert!(SourceDeliveryError::api(Some(429), "rate limited").retryable_review_sync());
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
            SourceDeliveryCredential::token("test-token"),
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
    assert!(!error.retryable_review_sync());
    assert!(!error.authentication_failed());
}

#[cfg(unix)]
fn script(root: &std::path::Path, source: &str) -> PathBuf {
    let program = root.join("gh-fixture");
    openengine_cluster_testkit::fixture::write_executable(
        &program,
        format!("#!/bin/sh\n{source}"),
        0o755,
    )
    .assert_value_with("write GitHub API fixture");
    program
}

#[cfg(unix)]
#[tokio::test]
async fn empty_api_output_remains_an_error() {
    let root = TemporaryDirectory::for_test("github-empty-api");
    let program = script(root.as_path(), "exit 0\n");
    let error = authority(program, Duration::from_secs(1))
        .api_output(
            &["graphql".to_owned()],
            SourceDeliveryCredential::token("test-token"),
        )
        .await
        .err()
        .assert_value();
    assert!(matches!(error, SourceDeliveryError::Api(_)));
    assert!(!error.retryable_operation());
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
        .api_output(&[], SourceDeliveryCredential::token("test-token"))
        .await
        .err()
        .assert_value();
    assert!(error.to_string().contains("could not start command:"));
    assert!(error.to_string().contains(&expected));
    assert!(!error.retryable_review_sync());
}

#[cfg(unix)]
#[test]
fn explicit_rate_limit_403s_are_temporary_but_permission_403_is_terminal() {
    use std::os::unix::process::ExitStatusExt;

    for (message, retryable) in [
        ("API rate limit exceeded for user ID 123.", true),
        (
            "You have exceeded a secondary rate limit. Please wait a few minutes before you try again.",
            true,
        ),
        ("Resource not accessible by integration", false),
    ] {
        let error = ApiOutput {
            stdout: Vec::new(),
            stderr: format!("gh: {message} (HTTP 403)\n").into_bytes(),
            ..Default::default()
        }
        .failure(
            Some(std::process::ExitStatus::from_raw(1 << 8)),
            "fixture failure",
            SourceDeliveryCredential::token("test-token"),
        );

        assert_eq!(error.api_status(), Some(403));
        assert_eq!(error.retryable_review_sync(), retryable);
        assert_eq!(error.retryable_operation(), retryable);
    }
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
        .api_output(&[], SourceDeliveryCredential::token("test-token"))
        .await
        .err()
        .assert_value();
    let diagnostic = error.to_string();
    assert!(
        diagnostic.contains("command timed out after 1000 milliseconds"),
        "unexpected diagnostic: {diagnostic}"
    );
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
    // Emitting and draining both bounded multi-megabyte streams is intentionally heavier under
    // coverage instrumentation; the assertion is about capture bounds, not scheduler latency.
    let error = authority(program, Duration::from_secs(30))
        .api_output(&[], SourceDeliveryCredential::token("test-token"))
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
async fn bare_gh_api_http_status_keeps_transient_and_refusal_classification() {
    // gh api prints a non-JSON error body to stdout and only the status to stderr.
    for (status, retryable) in [(503, true), (429, true), (401, false), (403, false)] {
        let root = TemporaryDirectory::for_test("github-api");
        let program = script(
            root.as_path(),
            &format!("printf 'Service Unavailable'\nprintf 'gh: HTTP {status}\\n' >&2\nexit 1\n"),
        );
        let error = authority(program, Duration::from_secs(10))
            .api_output(
                &["repos/acme/project/pulls/1538".to_owned()],
                SourceDeliveryCredential::token("test-token"),
            )
            .await
            .err()
            .assert_value();
        assert_eq!(error.api_status(), Some(status));
        assert_eq!(error.retryable_operation(), retryable, "HTTP {status}");
        assert_eq!(error.authentication_failed(), status == 401);
        assert!(error.to_string().contains(&format!("gh: HTTP {status}")));
    }
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
        .api_output(&[], SourceDeliveryCredential::token("test-token"))
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
        let failure = output.failure(None, &error, SourceDeliveryCredential::token("test-token"));
        let diagnostic = failure.to_string();
        assert!(diagnostic.contains(&format!(
            "could not read command {stream}: fixture read failure: [REDACTED]"
        )));
        assert!(diagnostic.contains(&format!(
            "{stream} (truncated=true):\nretained output: [REDACTED]"
        )));
        assert!(diagnostic.contains("partial secret:"));
        assert!(!diagnostic.contains("test-to"));
        assert!(!failure.retryable_review_sync());
        assert!(!failure.authentication_failed());
    }
}

#[test]
fn failed_step_output_survives_large_github_cleanup_logs() {
    let mut output =
        b"setup\nassertion: expected 400, received 202\n2026-09-14 ##[error]exit code 1\n".to_vec();
    output.extend_from_slice(&vec![b'x'; MAX_JOB_LOG_TAIL_BYTES * 2]);
    let excerpt = check_log_tail(&output);
    assert!(
        excerpt
            .text
            .contains("assertion: expected 400, received 202")
    );
    assert!(excerpt.text.ends_with("##[error]exit code 1\n"));
    assert!(!excerpt.text.contains("xxxx"));
}

#[test]
fn last_github_error_is_retained_without_rewriting_its_output() {
    let output = b"first ##[error]earlier failure\nraw final failure\nlast ##[error]later failure";
    assert_eq!(check_log_tail(output).text.as_bytes(), output);
}

#[test]
fn early_failure_survives_noisy_capture_and_final_feedback() {
    for jobs in [1, 2, 8] {
        let logs = (91..91 + jobs)
            .map(|job| {
                let output = format!(
                    "fatal: Not a valid commit name fixture-ref\n\
                     error: expected fixture rejection\n\
                     test source::error::validates_assertion_messages ... ok\n{}\nsetup context for job {job}\n\
                     thread 'case-{job}' panicked at fixture.rs:64:9:\n\
                     \u{001b}[31massertion `left == right` failed\u{001b}[0m\n\
                     recovery record detail for job {job}: résumé 界\n\
                     {}\n##[error]Process completed with exit code 1.\n\
                     cleanup must disappear{}",
                    "preamble\n".repeat(1_000),
                    "unrelated output\n".repeat(10_000),
                    "cleanup\n".repeat(10_000),
                );
                let excerpt = check_log_tail(output.as_bytes());
                assert!(excerpt.text.len() <= MAX_JOB_LOG_TAIL_BYTES);
                assert!(excerpt.text.contains("[intermediate log output truncated]"));
                (job, excerpt)
            })
            .collect::<Vec<_>>();
        let mut feedback = "Required CI checks failed".to_owned();
        include_job_logs(&mut feedback, "GitHub Actions", &logs);
        assert!(feedback.chars().count() <= MAX_FAILURE_FEEDBACK_CHARS);
        assert!(!feedback.contains("cleanup must disappear"));
        assert!(!feedback.contains('\u{fffd}'));
        assert!(
            feedback
                .chars()
                .all(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        );
        for (job, _) in logs {
            assert!(feedback.contains(&format!("GitHub Actions job {job} log excerpt")));
            assert!(feedback.contains(&format!("setup context for job {job}")));
            assert!(
                feedback.contains(&format!("thread 'case-{job}' panicked at fixture.rs:64:9:"))
            );
            assert!(feedback.contains("assertion `left == right` failed"));
            assert!(feedback.contains(&format!("recovery record detail for job {job}: résumé 界")));
        }
        assert_eq!(
            feedback
                .matches("##[error]Process completed with exit code 1.")
                .count(),
            jobs as usize
        );
    }
}

#[test]
fn feedback_budget_preserves_failures_from_individually_uncut_ascii_logs() {
    let logs = (91..93)
        .map(|job| {
            let output = format!(
                "fatal: Not a valid commit name fixture-ref\n\
                 error: expected fixture rejection\n\
                 test expected_git_refusal ... ok\n{}\n\
                 setup context for job {job}\nassertion failed in job {job}\n\
                 error detail for job {job}\n{}\n##[error]exit code 1\n",
                "preamble\n".repeat(1_000),
                "noise\n".repeat(8_000),
            );
            assert!(output.is_ascii());
            assert!(output.len() < MAX_JOB_LOG_TAIL_BYTES);
            let excerpt = check_log_tail(output.as_bytes());
            assert_eq!(excerpt.text, output);
            (job, excerpt)
        })
        .collect::<Vec<_>>();
    assert!(logs.iter().map(|(_, log)| log.text.len()).sum::<usize>() > MAX_FAILURE_FEEDBACK_CHARS);
    for selected in [&logs[..1], &logs[1..], &logs[..]] {
        let mut feedback = "Required CI checks failed".to_owned();
        include_job_logs(&mut feedback, "GitHub Actions", selected);
        assert!(feedback.chars().count() <= MAX_FAILURE_FEEDBACK_CHARS);
        for (job, _) in selected {
            assert!(feedback.contains(&format!("GitHub Actions job {job} log excerpt")));
            assert!(feedback.contains(&format!("setup context for job {job}")));
            assert!(feedback.contains(&format!("assertion failed in job {job}")));
            assert!(feedback.contains(&format!("error detail for job {job}")));
        }
        if selected.len() == 2 {
            assert_eq!(
                feedback
                    .matches("[intermediate log output truncated]")
                    .count(),
                2
            );
        }
    }
}

#[test]
fn generic_errors_remain_a_fallback_without_panic_or_assertion_evidence() {
    for failure in [
        "error: compiler failed",
        "error[E0433]: unresolved item",
        "fatal: build tool failed",
        "##[error]build tool failed",
    ] {
        let output = format!(
            "setup context\n{failure}\nerror detail\n{}\n##[error]exit code 1\n",
            "noise\n".repeat(24_000),
        );
        let excerpt = check_log_tail(output.as_bytes());
        assert!(excerpt.text.len() <= MAX_JOB_LOG_TAIL_BYTES);
        let mut feedback = "Required CI checks failed".to_owned();
        include_job_logs(&mut feedback, "GitHub Actions", &[(91, excerpt)]);
        assert!(feedback.chars().count() <= MAX_FAILURE_FEEDBACK_CHARS);
        assert!(feedback.contains("setup context"));
        assert!(feedback.contains(failure));
        assert!(feedback.contains("error detail"));
        assert!(feedback.contains("##[error]exit code 1"));
        assert!(feedback.contains("[intermediate log output truncated]"));
    }
}

#[test]
fn unrecognized_unicode_failures_keep_a_valid_bounded_tail() {
    let output = format!(
        "discard this preamble\n{}\nunknown termination 界🙂",
        "界🙂\n".repeat(30_000)
    );
    let excerpt = check_log_tail(output.as_bytes());
    assert!(excerpt.text.len() <= MAX_JOB_LOG_TAIL_BYTES);
    assert!(excerpt.text.starts_with("[earlier log output truncated]"));
    assert!(excerpt.text.ends_with("unknown termination 界🙂"));
    assert!(!excerpt.text.contains("discard this preamble"));
    assert!(!excerpt.text.contains('\u{fffd}'));
    let logs = (91..99)
        .map(|job| (job, excerpt.clone()))
        .collect::<Vec<_>>();
    let mut feedback = "Required CI checks failed".to_owned();
    include_job_logs(&mut feedback, "GitHub Actions", &logs);
    assert!(feedback.chars().count() <= MAX_FAILURE_FEEDBACK_CHARS);
    assert_eq!(feedback.matches("unknown termination 界🙂").count(), 8);
    assert_eq!(feedback.matches("log excerpt ---").count(), 8);
    assert!(!feedback.contains('\u{fffd}'));
}

#[test]
fn separate_git_credentials_cannot_be_split_into_visible_log_fragments() {
    use crate::native_v2_delivery::{command::redact, git_auth};
    use crate::source_code_provider::SourceGitCredential;

    let token = "forge-token-independent-of-git-0123456789";
    let github_git_token = "github-git-token-independent-of-forge-0123456789";
    let username = "git-username-private-0123456789";
    let password = "git-password-private-0123456789-abcdefghijklmnopqrstuvwxyz";
    let private_key = "-----BEGIN OPENSSH PRIVATE KEY-----\n\
        private-key-first-line-0123456789-abcdefghijklmnopqrstuvwxyz\n\
        private-key-second-line-0123456789-abcdefghijklmnopqrstuvwxyz\n\
        -----END OPENSSH PRIVATE KEY-----";
    let known_hosts = "git.example.invalid ssh-ed25519 private-host-key-0123456789";
    let proxy = "proxy-secret-independent-of-git-0123456789";
    for (git, mut secrets) in [
        (
            SourceGitCredential::Https { username, password },
            vec![
                username.to_owned(),
                password.to_owned(),
                git_auth::encode_user_password(username, password),
            ],
        ),
        (
            SourceGitCredential::Ssh {
                private_key,
                known_hosts,
            },
            [private_key, known_hosts]
                .into_iter()
                .chain(private_key.lines())
                .map(str::to_owned)
                .collect(),
        ),
    ] {
        let credential = SourceDeliveryCredential::token(token)
            .with_github_git_token(github_git_token)
            .with_git(git);
        secrets.extend([
            token.to_owned(),
            git_auth::encode_basic_credential(token),
            github_git_token.to_owned(),
            git_auth::encode_basic_credential(github_git_token),
            proxy.to_owned(),
        ]);
        for secret in &secrets {
            // Cross the 8 KiB context boundary at either truncation stage.
            // Final redaction alone cannot recognize the retained prefix.
            let fragment = &secret[..24];
            for noise_lines in [8_000, 24_000] {
                let mut raw = "setup context\nassertion failed: credential boundary\n".to_owned();
                raw.push_str(&"x".repeat(8 * 1024 - fragment.len() - raw.len()));
                raw.push_str(secret);
                raw.push('\n');
                raw.push_str(&"noise\n".repeat(noise_lines));
                raw.push_str("##[error]exit code 1\n");
                assert_eq!(raw.len() > MAX_JOB_LOG_TAIL_BYTES, noise_lines == 24_000);
                let excerpt =
                    super::check_log_tail(raw.as_bytes(), credential, &[proxy.to_owned()]);
                assert!(excerpt.text.len() <= MAX_JOB_LOG_TAIL_BYTES);
                for jobs in [1, 2] {
                    let logs = (91..91 + jobs)
                        .map(|job| (job, excerpt.clone()))
                        .collect::<Vec<_>>();
                    let mut feedback = "Required CI checks failed".to_owned();
                    include_job_logs(&mut feedback, "GitHub Actions", &logs);
                    let feedback = redact(&feedback, &secrets);
                    assert!(feedback.chars().count() <= MAX_FAILURE_FEEDBACK_CHARS);
                    assert!(feedback.contains("setup context"));
                    assert!(feedback.contains("assertion failed: credential boundary"));
                    assert!(feedback.contains("##[error]exit code 1"));
                    assert!(feedback.contains("[REDACTED]"));
                    assert!(
                        !feedback.contains(fragment),
                        "credential fragment survived selection"
                    );
                }
            }
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn job_log_capture_opts_in_and_feedback_removes_terminal_controls() {
    use crate::native_v2_delivery::git_auth;
    use crate::source_code_provider::SourceGitCredential;

    let root = TemporaryDirectory::for_test("github-job-log");
    let credential = SourceDeliveryCredential::token("test-token")
        .with_github_git_token("separate-github-git-token")
        .with_git(SourceGitCredential::Https {
            username: "delivery-user",
            password: "separate-git-password",
        });
    let payload = format!(
        "\x1b[31massertion failed: expected 400, got 202\x1b[0m\n\
         credential: test-token; encoded: {}\n\
         git: delivery-user separate-git-password {}; legacy: separate-github-git-token {}\n\
         \x1b]8;;https://example.invalid\x07link\x1b]8;;\x07\n",
        git_auth::encode_basic_credential("test-token"),
        git_auth::encode_user_password("delivery-user", "separate-git-password"),
        git_auth::encode_basic_credential("separate-github-git-token"),
    );
    std::fs::write(root.as_path().join("payload"), &payload).assert_value();
    let program = script(
        root.as_path(),
        r#"case "$*" in
  'api repos/acme/project/actions/jobs/91/logs --method GET --allow-escape-sequences --header X-GitHub-Api-Version: 2026-03-10') ;;
  *) printf '%s\n' 'the response contains terminal escape sequences' >&2; exit 1 ;;
esac
exec /bin/cat "$HOME/payload"
"#,
    );
    let output = authority(program, Duration::from_secs(10))
        .job_log_output("acme/project", 91, credential)
        .await
        .assert_value();
    assert_eq!(
        output.text.as_bytes(),
        concat!(
            "\x1b[31massertion failed: expected 400, got 202\x1b[0m\n",
            "credential: [REDACTED]; encoded: [REDACTED]\n",
            "git: [REDACTED] [REDACTED] [REDACTED]; legacy: [REDACTED] [REDACTED]\n",
            "\x1b]8;;https://example.invalid\x07link\x1b]8;;\x07\n"
        )
        .as_bytes()
    );
    let mut snapshot = PolicySnapshot {
        pull_request_id: "PR_node_17".to_owned(),
        is_in_merge_queue: false,
        auto_merge_enabled: false,
        state: SourceReviewState::Open {
            checks: SourceChecks::Failed {
                diagnostic: "Required CI checks failed".to_owned(),
            },
        },
        failed_job_ids: vec![91],
        is_merge_queue_enabled: false,
        head_update: None,
        pull_request_ready: false,
        observed_required_checks: Default::default(),
    };
    include_check_logs(&mut snapshot, &[(91, output)]);
    let SourceReviewState::Open {
        checks: SourceChecks::Failed { diagnostic },
    } = snapshot.state
    else {
        panic!("expected failed required checks");
    };
    assert!(diagnostic.contains("GitHub Actions job 91 log excerpt"));
    assert!(diagnostic.contains("assertion failed: expected 400, got 202"));
    assert!(diagnostic.contains("credential: [REDACTED]; encoded: [REDACTED]"));
    assert!(!diagnostic.contains("test-token"));
    assert!(
        diagnostic
            .chars()
            .all(|character| !character.is_control() || matches!(character, '\n' | '\t'))
    );
}

#[test]
fn native_git_credentials_remain_redacted_through_capture_and_aggregation() {
    let password = "native-password-private-0123456789-abcdefghijklmnopqrstuvwxyz";
    let secrets = vec![password.to_owned(), encode_basic_credential(password)];
    let credential =
        SourceDeliveryCredential::token("forge-token").with_diagnostic_secrets(&secrets);
    assert!(credential.git().is_none());
    for secret in &secrets {
        for (noise, jobs) in [(24_000, 1), (8_000, 2)] {
            let mut raw = "setup context\nassertion failed: native password boundary\n".to_owned();
            raw.push_str(&"x".repeat(8 * 1024 - 24 - raw.len()));
            raw.push_str(secret);
            raw.push_str(&format!(
                "\n{}\n##[error]exit code 1\n",
                "noise\n".repeat(noise)
            ));
            assert_eq!(raw.len() > MAX_JOB_LOG_TAIL_BYTES, jobs == 1);
            let logs = (91..91 + jobs)
                .map(|job| (job, super::check_log_tail(raw.as_bytes(), credential, &[])))
                .collect::<Vec<_>>();
            for label in ["GitHub Actions", "GitLab"] {
                let mut feedback = "Required CI checks failed".to_owned();
                include_job_logs(&mut feedback, label, &logs);
                let feedback = crate::native_v2_delivery::command::redact(&feedback, &secrets);
                assert!(feedback.chars().count() <= MAX_FAILURE_FEEDBACK_CHARS);
                assert!(feedback.contains("assertion failed: native password boundary"));
                assert!(feedback.contains("setup context"));
                assert!(feedback.contains("##[error]exit code 1"));
                assert!(!feedback.contains(&secret[..24]));
            }
        }
    }
}

#[test]
fn interrupted_log_streams_redact_all_credentials_before_diagnostic_bounds() {
    use crate::source_code_provider::SourceGitCredential;

    let password = "interrupted-private-password-0123456789-abcdefghijklmnopqrstuvwxyz";
    let secrets = vec![password.to_owned()];
    for native in [false, true] {
        let credential = SourceDeliveryCredential::token("forge-token");
        let credential = if native {
            credential.with_diagnostic_secrets(&secrets)
        } else {
            credential.with_git(SourceGitCredential::Https {
                username: "git-user",
                password,
            })
        };
        for stderr in ["unexpected EOF", "error connecting to api.github.com"] {
            let mut stdout = "assertion failed: interrupted log\n".to_owned();
            stdout.push_str(&"x".repeat(MAX_API_DIAGNOSTIC_STREAM_BYTES - 24 - stdout.len()));
            stdout.push_str(password);
            stdout.push_str(&"noise\n".repeat(8_000));
            let error = ApiOutput {
                stdout: stdout.into_bytes(),
                stderr: stderr.as_bytes().to_vec(),
                ..Default::default()
            }
            .failure(None, "could not read command stdout", credential);
            assert_eq!(error.api_status(), None);
            assert!(!error.authentication_failed());
            assert_eq!(
                error.retryable_operation(),
                stderr.starts_with("error connecting")
            );
            let log = JobLog::unavailable(format!("GitHub job log unavailable: {error}"));
            assert!(log.text.contains(stderr));
            assert!(!log.text.contains(&password[..24]));
            let logs = (91..99).map(|job| (job, log.clone())).collect::<Vec<_>>();
            let mut feedback = "Required CI checks failed".to_owned();
            include_job_logs(&mut feedback, "GitHub Actions", &logs);
            let feedback = crate::native_v2_delivery::command::redact(&feedback, &secrets);
            assert!(!feedback.contains(&password[..24]));
            assert!(feedback.contains("--- GitHub Actions job 91 log excerpt ---"));
            assert!(feedback.chars().count() <= MAX_FAILURE_FEEDBACK_CHARS);
        }
    }
    // Secret replacement must not hide a native transport classification.
    let transport = "error connecting to api.github.com".to_owned();
    let credential = SourceDeliveryCredential::token("forge-token")
        .with_diagnostic_secrets(std::slice::from_ref(&transport));
    let error = ApiOutput {
        stderr: transport.as_bytes().to_vec(),
        ..Default::default()
    }
    .failure(None, "download failed", credential);
    assert!(error.retryable_operation());
    assert!(!error.to_string().contains(&transport));
}

#[cfg(unix)]
#[tokio::test]
async fn interrupted_job_log_download_redacts_partial_stdout_before_feedback() {
    use crate::source_code_provider::SourceGitCredential;

    let root = TemporaryDirectory::for_test("github-partial-log");
    let password = "interrupted-private-password-0123456789-abcdefghijklmnopqrstuvwxyz";
    let mut payload = "assertion failed: interrupted log\n".to_owned();
    payload.push_str(&"x".repeat(MAX_API_DIAGNOSTIC_STREAM_BYTES - 24 - payload.len()));
    payload.push_str(password);
    std::fs::write(root.as_path().join("payload"), payload).assert_value();
    let program = script(
        root.as_path(),
        "cat \"$HOME/payload\"\nprintf 'unexpected EOF\\n' >&2\nexit 1\n",
    );
    let authority = authority(program, Duration::from_secs(10));
    let secrets = vec![password.to_owned()];
    for native in [false, true] {
        let credential = SourceDeliveryCredential::token("forge-token");
        let credential = if native {
            credential.with_diagnostic_secrets(&secrets)
        } else {
            credential.with_git(SourceGitCredential::Https {
                username: "git-user",
                password,
            })
        };
        let error = authority
            .job_log_output("acme/project", 91, credential)
            .await
            .unwrap_err();
        assert_eq!(error.api_status(), None);
        assert!(!error.authentication_failed());
        assert!(!error.retryable_operation());
        let log = super::super::job_log_excerpt(Err(error)).assert_value();
        assert!(log.text.contains("unexpected EOF"));
        assert!(log.text.contains("job log unavailable"));
        assert!(!log.text.contains(&password[..24]));
        let logs = (91..99).map(|job| (job, log.clone())).collect::<Vec<_>>();
        let mut feedback = "Required CI checks failed".to_owned();
        include_job_logs(&mut feedback, "GitHub Actions", &logs);
        let feedback = crate::native_v2_delivery::command::redact(&feedback, &secrets);
        assert!(!feedback.contains(&password[..24]));
        assert!(feedback.contains("--- GitHub Actions job 91 log excerpt ---"));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn job_log_capture_supports_older_gh_without_the_opt_in_flag() {
    use crate::source_code_provider::SourceGitCredential;

    for username in ["ordinary-user", "flag", "unknown", "error"] {
        let root = TemporaryDirectory::for_test("github-job-log");
        let program = script(
            root.as_path(),
            r#"printf 'call\n' >> "$HOME/calls"
case "$*" in
  *--allow-escape-sequences*) printf '%s\n' 'unknown flag: --allow-escape-sequences' >&2; exit 1 ;;
esac
printf 'older CLI failure details test-token\n##[error]exit code 1\ncleanup-must-disappear\n'
"#,
        );
        let output = authority(program, Duration::from_secs(10))
            .job_log_output(
                "acme/project",
                91,
                SourceDeliveryCredential::token("test-token").with_git(
                    SourceGitCredential::Https {
                        username,
                        password: "private-password",
                    },
                ),
            )
            .await
            .assert_value();
        let expected = crate::native_v2_delivery::command::redact(
            "older CLI failure details test-token\n##[error]exit code 1\n",
            &["test-token".to_owned(), username.to_owned()],
        );
        assert_eq!(output.text, expected);
        assert_eq!(
            std::fs::read_to_string(root.as_path().join("calls")).assert_value(),
            "call\ncall\n"
        );
    }
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
        .job_log_output(
            "acme/project",
            91,
            SourceDeliveryCredential::token("test-token"),
        )
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

#[test]
fn schema_failures_preserve_bounded_redacted_response_details() {
    let token = "sensitive-test-token";
    let value = serde_json::json!({
        "unexpected": format!("{token} {} {}", encode_basic_credential(token), "λ🦀".repeat(20_000))
    });
    let error = authority(PathBuf::from("unused"), Duration::from_secs(1))
        .decode_response::<GitReferenceWire>(value, SourceDeliveryCredential::token(token))
        .err()
        .expect("wrong schema must fail");
    let diagnostic = error.to_string();
    assert!(diagnostic.contains("missing field"));
    assert!(diagnostic.contains("unexpected"));
    assert!(diagnostic.contains("truncated=true"));
    assert!(diagnostic.contains("[REDACTED]"));
    assert!(!diagnostic.contains(token));
    assert!(!diagnostic.contains(&encode_basic_credential(token)));
    assert!(diagnostic.len() < 32 * 1024);
    assert!(!error.retryable_operation());
}

#[test]
fn native_cli_http_errors_preserve_status_and_retry_classification() {
    for (text, status, retryable) in [
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
        ("HTTP 503 (https://api.github.com/graphql)", Some(503), true),
        ("gh: HTTP 503", Some(503), true),
        ("gh: HTTP 502", Some(502), true),
        ("gh: HTTP 429", Some(429), true),
        ("gh: HTTP 401", Some(401), false),
        ("gh: HTTP 403", Some(403), false),
        ("gh: HTTP 404", Some(404), false),
        (
            r#"non-200 OK status code: 503 Service Unavailable body: "Service Unavailable""#,
            Some(503),
            true,
        ),
        (
            r#"non-200 OK status code: 502 Bad Gateway body: "<html>Bad Gateway</html>""#,
            Some(502),
            true,
        ),
        (
            r#"non-200 OK status code: 401 Unauthorized body: "{\"message\": \"Bad credentials\"}""#,
            Some(401),
            false,
        ),
        (
            r#"non-200 OK status code: 403 Forbidden body: "{\"message\": \"Forbidden\"}""#,
            Some(403),
            false,
        ),
        (
            "HTTP 403: API rate limit exceeded for user ID 123. (https://api.github.com/graphql)",
            Some(403),
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
            "GraphQL: Resource not accessible by integration (mergePullRequest)",
            None,
            false,
        ),
        ("HTTP unavailable", None, false),
    ] {
        let error = github_api_error(text.as_bytes());
        assert_eq!(error.api_status(), status, "{text}");
        assert_eq!(error.retryable_operation(), retryable, "{text}");
        assert_eq!(error.authentication_failed(), status == Some(401), "{text}");
    }
}

fn proxy_cutoff_fixture() -> (super::super::super::network::ForgeNetwork, [String; 3]) {
    let password = format!("proxy-password-boundary-{}", "p".repeat(96));
    let url = format!("proxy-user:{password}@proxy.invalid:8080");
    let basic = crate::native_v2_delivery::git_auth::encode_user_password("proxy-user", &password);
    let network = super::super::super::network::ForgeNetwork::from_snapshot(&BTreeMap::from([(
        "HTTPS_PROXY".into(),
        url.clone(),
    )]));
    (network, [password, url, basic])
}

fn assert_proxy_cutoff_redacted(error: SourceDeliveryError, echoed: &str) {
    let diagnostic = format!("{error:?}\n{error}");
    assert!(diagnostic.contains("truncated=true"), "{diagnostic}");
    assert!(diagnostic.contains("[REDACTED]"), "{diagnostic}");
    assert!(
        !diagnostic.contains(&echoed[..24]),
        "partial proxy credential leaked"
    );
}

#[test]
fn schema_failures_redact_proxy_credentials_before_diagnostic_cutoff() {
    let (network, echoes) = proxy_cutoff_fixture();
    let authority =
        authority(PathBuf::from("unused"), Duration::from_secs(1)).with_network(network);
    for echoed in echoes {
        let value = serde_json::json!({"unexpected": format!("{}{echoed}{}",
            "x".repeat(MAX_API_DIAGNOSTIC_STREAM_BYTES - 48), "z".repeat(1024))});
        let error = authority
            .decode_response::<GitReferenceWire>(
                value,
                SourceDeliveryCredential::token("test-token"),
            )
            .err()
            .expect("schema must be rejected");
        assert_proxy_cutoff_redacted(error, &echoed);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn malformed_success_redacts_proxy_credentials_before_diagnostic_cutoff() {
    let root = TemporaryDirectory::for_test("github-proxy-cutoff");
    let program = script(root.as_path(), "cat \"$HOME/payload\"\n");
    let (network, echoes) = proxy_cutoff_fixture();
    let authority = authority(program, Duration::from_secs(10)).with_network(network);
    for echoed in echoes {
        std::fs::write(
            root.as_path().join("payload"),
            format!(
                "{}{echoed}{}",
                "x".repeat(MAX_API_DIAGNOSTIC_STREAM_BYTES - 48),
                "z".repeat(1024)
            ),
        )
        .assert_value();
        let error = authority
            .api(
                &["repos/acme/project".into()],
                SourceDeliveryCredential::token("test-token"),
            )
            .await
            .expect_err("malformed JSON must be rejected");
        assert_proxy_cutoff_redacted(error, &echoed);
    }
}

fn plan_unavailable_output() -> ApiOutput {
    ApiOutput {
        // Captured shape from real gh api --paginate --slurp, including the first-page error wrapper.
        stdout: br#"[{"message":"Upgrade to GitHub Pro or make this repository public to enable this feature.","documentation_url":"https://docs.github.com/rest/repos/rules#get-rules-for-a-branch","status":"403"}]"#.to_vec(),
        stderr: b"gh: Upgrade to GitHub Pro or make this repository public to enable this feature. (HTTP 403)\n".to_vec(),
        ..Default::default()
    }
}

#[test]
fn branch_rules_entitlement_requires_a_complete_standalone_protocol_response() {
    let output = plan_unavailable_output();
    assert!(output.branch_rules_unavailable());
    let value: Value = serde_json::from_slice(&output.stdout).assert_value();
    let error = &value[0];
    let mut permission = error.clone();
    permission["message"] = serde_json::json!("Resource not accessible by integration");
    let mut wrong_status = error.clone();
    wrong_status["status"] = serde_json::json!("401");
    for value in [
        Value::Null,
        serde_json::json!([]),
        error.clone(),
        serde_json::json!([[], error]),
        serde_json::json!([[{"type":"required_status_checks","parameters":{"required_status_checks":[{"context":"required","integration_id":15368}]}}], error]),
        serde_json::json!([error, error]),
        serde_json::json!([permission]),
        serde_json::json!([wrong_status]),
        serde_json::json!([{"message":error["message"]}]),
    ] {
        let output = ApiOutput {
            stdout: value.to_string().into_bytes(),
            ..plan_unavailable_output()
        };
        assert!(!output.branch_rules_unavailable(), "{value}");
    }
    for stdout in [
        Vec::new(),
        output.stdout[..output.stdout.len() - 1].to_vec(),
        [output.stdout.as_slice(), b"trailing garbage"].concat(),
        [output.stdout.as_slice(), &[0xff]].concat(),
        [output.stdout.as_slice(), &vec![b' '; MAX_API_OUTPUT_BYTES]].concat(),
    ] {
        assert!(
            !ApiOutput {
                stdout,
                ..plan_unavailable_output()
            }
            .branch_rules_unavailable()
        );
    }
    for stderr in [
        Vec::new(),
        b"gh: Resource not accessible by integration (HTTP 403)\n".to_vec(),
        b"gh: API rate limit exceeded for user ID 123. (HTTP 403)\n".to_vec(),
        [output.stderr.as_slice(), b"another failure\n"].concat(),
        [output.stderr.as_slice(), &[0xff]].concat(),
        [output.stderr.as_slice(), &vec![b' '; MAX_API_ERROR_BYTES]].concat(),
    ] {
        assert!(
            !ApiOutput {
                stderr,
                ..plan_unavailable_output()
            }
            .branch_rules_unavailable()
        );
    }
}

#[cfg(unix)]
fn plan_unavailable_script(root: &std::path::Path, ending: &str) -> PathBuf {
    let output = plan_unavailable_output();
    script(
        root,
        &format!(
            "printf '%s' '{}'\nprintf '%s' '{}' >&2\n{ending}\n",
            String::from_utf8(output.stdout).assert_value(),
            String::from_utf8(output.stderr).assert_value(),
        ),
    )
}

#[cfg(unix)]
#[tokio::test]
async fn branch_rules_entitlement_is_endpoint_scoped_and_precedes_redaction() {
    let root = TemporaryDirectory::for_test("github-rules-entitlement");
    // Matching command-path prose must neither grant nor suppress the exception.
    let directory = root.as_path().join("gh: Upgrade to GitHub Pro or make this repository public to enable this feature. (HTTP 403)");
    std::fs::create_dir(&directory).assert_value();
    let program = plan_unavailable_script(&directory, "exit 1");
    let provider = authority(program, Duration::from_secs(10));
    for secret in ["test-token", "Upgrade to GitHub Pro", "403", "message"] {
        let credential = SourceDeliveryCredential::token(secret);
        let rules = provider
            .branch_rules(&super::super::unit::review(), credential)
            .await
            .assert_value();
        assert_eq!(rules, serde_json::json!([[]]));
        let error = provider
            .api(&["repos/acme/project/pulls".to_owned()], credential)
            .await
            .err()
            .assert_value();
        assert_eq!(error.api_status(), Some(403));
        assert!(!error.retryable_operation());
        assert!(!error.to_string().contains(secret));
    }
    // Identical command context and credential cannot convert a permission response to no rules.
    script(
        &directory,
        "printf '%s' '[{\"message\":\"Resource not accessible by integration\",\"status\":\"403\"}]'\nprintf '%s\\n' 'gh: Resource not accessible by integration (HTTP 403)' >&2\nexit 1\n",
    );
    let error = provider
        .branch_rules(
            &super::super::unit::review(),
            SourceDeliveryCredential::token("Resource not accessible by integration"),
        )
        .await
        .err()
        .assert_value();
    assert_eq!(error.api_status(), Some(403));
    assert!(!error.retryable_operation());
}

#[cfg(unix)]
#[tokio::test]
async fn timed_out_branch_rules_entitlement_remains_unknown_and_retryable() {
    let root = TemporaryDirectory::for_test("github-rules-timeout");
    let program = plan_unavailable_script(root.as_path(), "exec /bin/sleep 30");
    let error = authority(program, Duration::from_secs(1))
        .branch_rules(
            &super::super::unit::review(),
            SourceDeliveryCredential::token("test-token"),
        )
        .await
        .err()
        .assert_value();
    assert_eq!(error.api_status(), None);
    assert!(error.retryable_operation());
    assert!(error.to_string().contains("command timed out"));
}
