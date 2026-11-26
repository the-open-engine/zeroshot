use std::sync::atomic::{AtomicUsize, Ordering};

use openengine_cluster_testkit::assertions::AssertValue;
use reqwest::StatusCode;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use super::super::super::http_fixture::{self, IntoResponse, Json, Response};
use super::*;
use crate::native_v2_delivery::git_auth::encode_basic_credential;
use crate::source_code_provider::adapters::core::ci_diagnostics::{
    MAX_FAILURE_FEEDBACK_CHARS, MAX_JOB_LOG_TAIL_BYTES, include_job_logs,
};

#[path = "mutation_tests.rs"]
mod mutation_tests;
#[path = "policy_read_tests.rs"]
mod policy_read_tests;
#[path = "rest_pagination_tests.rs"]
mod rest_pagination_tests;

const TOKEN: &str = "fixture-token";
const REST_PATH: &[&str] = &["repos", "acme", "project", "pulls"];

fn authority(deadline: Duration) -> GhCliDeliveryAuthority {
    GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
        api_deadline: deadline,
        ..GhCliAuthorityConfig::hosted(std::env::temp_dir())
    })
}

struct ApiFixture {
    origin: String,
    server: JoinHandle<()>,
    attempts: Arc<AtomicUsize>,
}

impl ApiFixture {
    async fn serve(
        handler: impl Fn(usize, http_fixture::Request) -> Response + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let attempts = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&attempts);
        let server = tokio::spawn(http_fixture::serve(listener, move |request| {
            handler(calls.fetch_add(1, Ordering::Relaxed), request)
        }));
        Self {
            origin,
            server,
            attempts,
        }
    }

    fn calls(&self) -> usize {
        self.attempts.load(Ordering::Relaxed)
    }

    fn authority(&self) -> GhCliDeliveryAuthority {
        authority(Duration::from_secs(10)).with_test_api(&self.origin)
    }
}

impl Drop for ApiFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn credential() -> SourceDeliveryCredential<'static> {
    SourceDeliveryCredential::token(TOKEN)
}

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
        ("error", "error\x1b[0m\x1b[1m: unresolved symbol"),
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
fn log_tail_retains_the_end_within_the_capture_budget() {
    let mut output = b"discard".to_vec();
    output.extend(vec![b'x'; MAX_JOB_LOG_TAIL_BYTES]);
    output.extend_from_slice(b"failure at end");
    let tail = check_log_tail(&output);
    assert_eq!(tail.text.len(), MAX_JOB_LOG_TAIL_BYTES);
    assert!(!tail.text.contains("discard"));
    assert!(tail.text.ends_with("failure at end"));
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
fn schema_failures_preserve_bounded_redacted_response_details() {
    let token = "sensitive-test-token";
    let value = serde_json::json!({
        "unexpected": format!("{token} {} {}", encode_basic_credential(token), "λ🦀".repeat(20_000))
    });
    let error = authority(Duration::from_secs(1))
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
    let authority = authority(Duration::from_secs(1)).with_network(network);
    for echoed in echoes {
        let value = serde_json::json!({"unexpected": format!("{}{echoed}{}",
            "x".repeat(MAX_API_DIAGNOSTIC_BYTES - 48), "z".repeat(1024))});
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

#[tokio::test]
async fn native_job_log_retains_annotations_and_feedback_removes_terminal_controls() {
    use crate::native_v2_delivery::git_auth;
    use crate::source_code_provider::SourceGitCredential;

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
    let fixture = ApiFixture::serve(move |_, request| {
        assert_eq!(
            request.uri.path(),
            "/repos/acme/project/actions/jobs/91/logs"
        );
        assert_eq!(request.method, reqwest::Method::GET);
        Response::raw(payload.clone().into_bytes())
    })
    .await;
    let output = fixture
        .authority()
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
        queue_recovery: None,
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

#[tokio::test]
async fn job_log_deadline_preserves_bounded_operation_retry() {
    let disconnected = Arc::new(tokio::sync::Notify::new());
    let fixture =
        ApiFixture::serve(move |_, _| Response::stall_until_disconnect(disconnected.clone())).await;
    let provider = authority(Duration::from_secs(1)).with_test_api(&fixture.origin);
    let error = provider
        .job_log_output("acme/project", 91, credential())
        .await
        .unwrap_err();
    assert!(error.retryable_operation(), "{error}");
    assert!(error.to_string().contains("API deadline"), "{error}");
    assert_eq!(fixture.calls(), 1);
    assert!(super::super::job_log_excerpt(Err(error)).is_err());
}

#[tokio::test]
async fn signed_job_log_failures_retry_without_claiming_forge_authentication() {
    let root = tempfile::tempdir().unwrap();
    let ca = root.path().join("ca.pem");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("https://{}", listener.local_addr().unwrap());
    let tls = http_fixture::loopback_tls(&ca);
    let server = tokio::spawn(http_fixture::serve_tls(listener, tls, |request| {
        if request.uri.path() == "/api/v3/meta" {
            return Json(json!({"installed_version":"3.22.2"})).into_response();
        }
        if let Some(code) = request.uri.path().strip_prefix("/signed/") {
            assert!(!request.headers.contains_key("authorization"));
            assert_eq!(request.uri.query(), Some("sig=private-download-signature"));
            return (
                code.parse::<StatusCode>().unwrap(),
                Json(json!({"message":"failure with private-download-signature"})),
            )
                .into_response();
        }
        assert_eq!(request.headers["authorization"], "Bearer fixture-token");
        let path: Vec<_> = request.uri.path().split('/').collect();
        let job = path[path.len() - 2];
        StatusCode::FOUND.into_response().header(
            "Location",
            &format!("/signed/{job}?sig=private-download-signature"),
        )
    }));
    let network = super::super::super::network::ForgeNetwork::from_snapshot(&BTreeMap::from([(
        "SSL_CERT_FILE".into(),
        ca.to_string_lossy().into_owned(),
    )]));
    let provider = authority(Duration::from_secs(5))
        .with_base_url(openengine_cluster_protocol::SourceForgeBaseUrl::new(origin).unwrap())
        .unwrap()
        .with_network(network);
    for (status, retry) in [(401, false), (403, false), (429, true), (503, true)] {
        let error = provider
            .job_log_output("acme/project", status, credential())
            .await
            .unwrap_err();
        assert_eq!(error.api_status(), None);
        assert!(!error.authentication_failed(), "{error}");
        assert_eq!(error.retryable_operation(), retry, "{error}");
        assert!(
            error.to_string().contains(&format!("HTTP {status}")),
            "{error}"
        );
        assert!(
            !error.to_string().contains("private-download-signature"),
            "{error}"
        );
    }
    server.abort();
}

#[tokio::test]
async fn interrupted_job_log_download_redacts_partial_body_before_feedback() {
    use crate::source_code_provider::SourceGitCredential;

    let password = "interrupted-private-password-0123456789-abcdefghijklmnopqrstuvwxyz";
    let mut payload = "assertion failed: interrupted log\n".to_owned();
    payload.push_str(&"x".repeat(MAX_API_DIAGNOSTIC_BYTES - 24 - payload.len()));
    payload.push_str(password);
    let fixture = ApiFixture::serve(move |_, _| {
        Response::raw(payload.clone().into_bytes()).content_length(payload.len() + 1)
    })
    .await;
    let authority = fixture.authority();
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
        assert!(log.text.contains("partial response: assertion failed"));
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

#[tokio::test]
async fn http_status_and_rate_limits_determine_read_and_review_sync_recovery() {
    let cases = [
        (401, "Bad credentials", None, false, false),
        (
            403,
            "Resource not accessible by integration",
            None,
            false,
            false,
        ),
        (
            403,
            "API rate limit exceeded for user ID 123.",
            None,
            true,
            true,
        ),
        (
            403,
            "You have exceeded a secondary rate limit. Please wait.",
            None,
            true,
            true,
        ),
        (403, "Forbidden", Some(("retry-after", "1")), true, true),
        (
            403,
            "Forbidden",
            Some(("x-ratelimit-remaining", "0")),
            true,
            true,
        ),
        (404, "Not found", None, false, true),
        (409, "Conflict", None, false, true),
        (422, "Validation failed", None, false, true),
        (429, "Too Many Requests", None, true, true),
        (502, "Bad Gateway", None, true, true),
        (503, "Service Unavailable", None, true, true),
    ];
    for (status, message, header, retry_operation, retry_review_sync) in cases {
        let status = StatusCode::from_u16(status).unwrap();
        let fixture = ApiFixture::serve(move |_, _| {
            let response = if status.is_server_error() {
                Response::raw(format!("<html>{message}</html>").into_bytes()).status(status)
            } else {
                (
                    status,
                    Json(json!({
                        "message": message,
                        "errors": [{"message": "Head sha can't be blank"}],
                    })),
                )
                    .into_response()
            };
            if let Some((name, value)) = header {
                response.header(name, value)
            } else {
                response
            }
        })
        .await;
        let error = fixture
            .authority()
            .rest(Request::get(REST_PATH), credential())
            .await
            .unwrap_err();
        assert_eq!(error.api_status(), Some(status.as_u16()), "{message}");
        assert_eq!(
            error.authentication_failed(),
            status == StatusCode::UNAUTHORIZED
        );
        assert_eq!(error.retryable_operation(), retry_operation, "{message}");
        assert_eq!(
            error.retryable_review_sync(),
            retry_review_sync,
            "{message}"
        );
        assert!(error.to_string().contains(message));
        if status == StatusCode::UNPROCESSABLE_ENTITY {
            assert!(error.to_string().contains("Head sha can't be blank"));
        }
    }
}

#[tokio::test]
async fn complete_empty_and_malformed_replies_stay_distinct_from_a_lost_reply() {
    for (body, interrupted, retryable_sync) in [
        ("", false, false),
        ("HTTP 401: Bad credentials", false, false),
        (r#"{"key":"value"}"#, true, true),
        (r#"{"key":"#, false, true),
    ] {
        let fixture = ApiFixture::serve(move |_, _| {
            let response = Response::raw(body.as_bytes().to_vec());
            if interrupted {
                response.content_length(body.len() + 1)
            } else {
                response
            }
        })
        .await;
        let error = fixture
            .authority()
            .rest_review_sync(Request::get(REST_PATH), credential())
            .await
            .unwrap_err();
        assert_eq!(error.api_status(), None);
        assert!(!error.authentication_failed());
        assert!(!error.retryable_operation());
        assert_eq!(error.retryable_review_sync(), retryable_sync, "{body:?}");
    }
}

#[tokio::test]
async fn malformed_success_redacts_proxy_credentials_before_diagnostic_cutoff() {
    let (network, echoes) = proxy_cutoff_fixture();
    for echoed in echoes {
        let body = format!(
            "{}{echoed}{}",
            "x".repeat(MAX_API_DIAGNOSTIC_BYTES - 48),
            "z".repeat(1024),
        );
        let fixture = ApiFixture::serve(move |_, _| Response::raw(body.clone().into_bytes())).await;
        let authority = authority(Duration::from_secs(10))
            .with_network(network.clone())
            .with_test_api(&fixture.origin);
        let error = authority
            .rest(Request::get(REST_PATH), credential())
            .await
            .unwrap_err();
        let diagnostic = format!("{error:?}\n{error}");
        assert!(diagnostic.contains("[REDACTED]"), "{diagnostic}");
        assert!(!diagnostic.contains(&echoed[..24]));
    }
}

#[tokio::test]
async fn complete_error_body_retains_status_and_redacts_credentials_before_bounding() {
    let encoded = encode_basic_credential(TOKEN);
    let body = format!(
        "Bad credentials\ntoken: {TOKEN}\nencoded: {encoded}\n{}",
        "long response 々".repeat(3_500)
    );
    let fixture = ApiFixture::serve(move |_, _| {
        Response::raw(body.clone().into_bytes())
            .status(StatusCode::UNAUTHORIZED)
            .header("x-github-request-id", "fixture-id")
    })
    .await;
    let error = fixture
        .authority()
        .rest(Request::get(REST_PATH), credential())
        .await
        .unwrap_err();
    let diagnostic = error.to_string();
    assert_eq!(error.api_status(), Some(401));
    assert!(error.authentication_failed());
    assert!(!error.retryable_operation());
    assert!(diagnostic.contains("fixture-id"));
    assert!(diagnostic.contains("Bad credentials"));
    assert!(diagnostic.contains("token: [REDACTED]"));
    assert!(diagnostic.contains("encoded: [REDACTED]"));
    assert!(!diagnostic.contains(TOKEN));
    assert!(!diagnostic.contains(&encoded));
    assert!(diagnostic.len() < crate::source_code_provider::MAX_SOURCE_API_DIAGNOSTIC_BYTES);
}

#[tokio::test]
async fn logs_do_not_retry_unrelated_api_errors() {
    let fixture = ApiFixture::serve(move |_, _| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"message":"log unavailable"})),
        )
            .into_response()
    })
    .await;
    let error = fixture
        .authority()
        .job_log_output("acme/project", 91, credential())
        .await
        .unwrap_err();
    assert_eq!(error.api_status(), Some(422));
    assert_eq!(fixture.calls(), 1);
}

const RULES_PATH: &[&str] = &["repos", "acme", "project", "rules", "branches", "main"];
const UPGRADE: &str =
    "Upgrade to GitHub Pro or make this repository public to enable this feature.";

#[tokio::test]
async fn branch_rules_entitlement_is_endpoint_scoped_and_precedes_redaction() {
    let fixture = ApiFixture::serve(|index, _| {
        let mut body = json!({"message": UPGRADE});
        if index % 4 >= 2 {
            body["status"] = json!("403");
        }
        (StatusCode::FORBIDDEN, Json(body)).into_response()
    })
    .await;
    let authority = fixture.authority();
    for token in [TOKEN, "Upgrade to GitHub Pro", "403", "message"] {
        let credential = SourceDeliveryCredential::token(token);
        assert_eq!(
            authority
                .rest_pages(RULES_PATH, credential, true)
                .await
                .unwrap(),
            json!([[]])
        );
        let error = authority
            .rest(Request::get(REST_PATH), credential)
            .await
            .unwrap_err();
        assert_eq!(error.api_status(), Some(403));
        assert!(!error.retryable_operation());
        assert!(!error.to_string().contains(token));
    }
}

#[tokio::test]
async fn branch_rules_entitlement_requires_a_complete_first_page_and_matching_status() {
    let denied = || {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"message": UPGRADE, "status":"403"})),
        )
            .into_response()
    };
    for (body, status, incomplete, linked, expected_status) in [
        (json!({"message": UPGRADE}), 401, false, false, Some(401)),
        (
            json!({"message": UPGRADE, "status":"401"}),
            403,
            false,
            false,
            Some(403),
        ),
        (
            json!({"message":"Resource not accessible by integration"}),
            403,
            false,
            false,
            Some(403),
        ),
        (json!({"message": UPGRADE}), 403, true, false, None),
        (json!({"message": UPGRADE}), 403, false, true, Some(403)),
        (
            json!([[], {"message": UPGRADE}]),
            403,
            false,
            false,
            Some(403),
        ),
    ] {
        let fixture = ApiFixture::serve(move |_, _| {
            let bytes = serde_json::to_vec(&body).unwrap();
            let len = bytes.len();
            let mut response = Response::raw(bytes).status(StatusCode::from_u16(status).unwrap());
            if incomplete {
                response = response.content_length(len + 1);
            }
            if linked {
                response =
                    response.header("link", "<https://different.invalid?page=2>; rel=\"next\"");
            }
            response
        })
        .await;
        let error = fixture
            .authority()
            .rest_pages(RULES_PATH, credential(), true)
            .await
            .unwrap_err();
        assert_eq!(error.api_status(), expected_status);
        assert_eq!(error.retryable_operation(), incomplete);
    }
    let fixture = ApiFixture::serve(move |_, request| {
        if request.uri.query_pairs().any(|(key, _)| key == "page") {
            denied()
        } else {
            let target = request.uri.path();
            Json(json!([{"type":"required_status_checks"}]))
                .into_response()
                .header(
                    "link",
                    &format!("<{target}?per_page=100&page=2>; rel=\"next\""),
                )
        }
    })
    .await;
    let error = fixture
        .authority()
        .rest_pages(RULES_PATH, credential(), true)
        .await
        .unwrap_err();
    assert_eq!(error.api_status(), Some(403), "{error}");
    assert_eq!(fixture.calls(), 2);
}

#[tokio::test]
async fn refused_job_log_connection_is_retryable_without_http_authority() {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    // Reserve the port without starting a listener, so no other fixture accepts this request.
    let origin = format!("http://{}", socket.local_addr().unwrap());
    let api = authority(Duration::from_secs(2)).with_test_api(&origin);
    let result = api.job_log_output("acme/project", 91, credential()).await;
    drop(socket);

    let error = super::super::job_log_excerpt(result).unwrap_err();
    assert_eq!(error.api_status(), None);
    assert!(!error.authentication_failed());
    assert!(error.retryable_operation());
}
