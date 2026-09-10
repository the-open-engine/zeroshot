use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::time::timeout;

use crate::native_v2_delivery::git_auth::encode_basic_credential;
use crate::native_v2_target_authority::{MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES, NewOperatorDiagnostic};

use super::{
    GhCliDeliveryAuthority, GitHubAuthorityError, GitHubCredential, GitHubPushRequest,
    authenticated_git_command,
};

const REDACTED: &str = "[REDACTED]";
const READ_BUFFER_BYTES: usize = 8 * 1024;

pub(super) async fn push_branch(
    authority: &GhCliDeliveryAuthority,
    request: &GitHubPushRequest,
    credential: GitHubCredential<'_>,
) -> Result<(), GitHubAuthorityError> {
    let basic_credential = encode_basic_credential(credential.expose());
    let secrets = [credential.expose().as_bytes(), basic_credential.as_bytes()];
    let mut command = authenticated_git_command(&authority.config, &request.workspace, credential);
    command
        .arg("push")
        .arg("--porcelain")
        .arg("--no-verify")
        .arg(format!(
            "https://github.com/{}.git",
            request.target.repository
        ))
        .arg(format!("HEAD:refs/heads/{}", request.head_branch))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match capture(command, authority.config.push_deadline, &secrets).await {
        Ok(()) => Ok(()),
        Err(failure) => {
            authority.record_push_failure(failure.diagnostic);
            Err(failure.error)
        }
    }
}

struct PushFailure {
    error: GitHubAuthorityError,
    diagnostic: CommandDiagnostic,
}

impl PushFailure {
    fn unavailable(message: &str) -> Self {
        Self {
            error: GitHubAuthorityError::Unavailable,
            diagnostic: CommandDiagnostic::message(message),
        }
    }
}

struct CommandDiagnostic {
    exit_status: Option<i32>,
    stdout: CapturedText,
    stderr: CapturedText,
}

impl CommandDiagnostic {
    fn message(message: &str) -> Self {
        Self {
            exit_status: None,
            stdout: CapturedText::default(),
            stderr: CapturedText {
                text: message.to_owned(),
                truncated: false,
            },
        }
    }
}

#[derive(Default)]
struct CapturedText {
    text: String,
    truncated: bool,
}

struct CapturedBytes {
    bytes: Vec<u8>,
    retain_bytes: usize,
    truncated: bool,
    read_failed: bool,
}

impl CapturedBytes {
    fn new(retain_bytes: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(retain_bytes),
            retain_bytes,
            truncated: false,
            read_failed: false,
        }
    }
}

struct ChildOutputs {
    stdout: ChildStdout,
    stderr: ChildStderr,
}

struct CapturedStreams {
    stdout: CapturedBytes,
    stderr: CapturedBytes,
}

enum Termination {
    Exited(ExitStatus),
    TimedOut,
    Unavailable,
}

async fn capture(
    mut command: Command,
    deadline: Duration,
    secrets: &[&[u8]],
) -> Result<(), PushFailure> {
    let retain_bytes = MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES
        .saturating_add(secrets.iter().map(|secret| secret.len()).max().unwrap_or(0));
    let mut child = command
        .spawn()
        .map_err(|_| PushFailure::unavailable("git push could not be started"))?;
    let outputs = match (child.stdout.take(), child.stderr.take()) {
        (Some(stdout), Some(stderr)) => ChildOutputs { stdout, stderr },
        _ => {
            stop_child(&mut child);
            return Err(PushFailure::unavailable(
                "git push output capture was unavailable",
            ));
        }
    };
    let mut captured = CapturedStreams {
        stdout: CapturedBytes::new(retain_bytes),
        stderr: CapturedBytes::new(retain_bytes),
    };
    let termination = wait_for_capture(&mut child, outputs, &mut captured, deadline).await;
    capture_result(termination, captured, secrets)
}

fn capture_result(
    termination: Termination,
    captured: CapturedStreams,
    secrets: &[&[u8]],
) -> Result<(), PushFailure> {
    let diagnostic = CommandDiagnostic {
        exit_status: match &termination {
            Termination::Exited(status) => status.code(),
            Termination::TimedOut | Termination::Unavailable => None,
        },
        stdout: sanitize_output(captured.stdout, secrets),
        stderr: sanitize_output(captured.stderr, secrets),
    };
    match termination {
        Termination::Exited(status) if status.success() => Ok(()),
        Termination::Exited(_) => Err(PushFailure {
            error: GitHubAuthorityError::Rejected,
            diagnostic,
        }),
        Termination::TimedOut => Err(PushFailure {
            error: GitHubAuthorityError::Unavailable,
            diagnostic: with_context(diagnostic, "git push timed out"),
        }),
        Termination::Unavailable => Err(PushFailure {
            error: GitHubAuthorityError::Unavailable,
            diagnostic: with_context(diagnostic, "git push status was unavailable"),
        }),
    }
}

async fn wait_for_capture(
    child: &mut Child,
    mut outputs: ChildOutputs,
    captured: &mut CapturedStreams,
    deadline: Duration,
) -> Termination {
    let operation = async {
        let (status, (), ()) = tokio::join!(
            child.wait(),
            drain_bounded(&mut outputs.stdout, &mut captured.stdout),
            drain_bounded(&mut outputs.stderr, &mut captured.stderr),
        );
        status
    };
    match timeout(deadline, operation).await {
        Ok(Ok(status)) => Termination::Exited(status),
        Ok(Err(_)) => {
            stop_child(child);
            Termination::Unavailable
        }
        Err(_) => {
            captured.stdout.truncated = true;
            captured.stderr.truncated = true;
            stop_child(child);
            Termination::TimedOut
        }
    }
}

fn stop_child(child: &mut Child) {
    let _ = child.start_kill();
}

async fn drain_bounded(reader: &mut (impl AsyncRead + Unpin), captured: &mut CapturedBytes) {
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    loop {
        let read = match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => {
                captured.read_failed = true;
                break;
            }
        };
        let retained = captured
            .retain_bytes
            .saturating_sub(captured.bytes.len())
            .min(read);
        captured.bytes.extend_from_slice(&buffer[..retained]);
        captured.truncated |= retained < read;
    }
}

fn sanitize_output(mut captured: CapturedBytes, secrets: &[&[u8]]) -> CapturedText {
    if captured.truncated || captured.read_failed {
        trim_trailing_secret_prefix(&mut captured.bytes, secrets);
    }
    let mut text = String::from_utf8_lossy(&captured.bytes).into_owned();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        if let Ok(secret) = std::str::from_utf8(secret) {
            text = text.replace(secret, REDACTED);
        }
    }
    let truncated = captured.truncated || captured.read_failed;
    let (text, text_truncated) = truncate_text(text);
    CapturedText {
        text,
        truncated: truncated || text_truncated,
    }
}

fn trim_trailing_secret_prefix(bytes: &mut Vec<u8>, secrets: &[&[u8]]) {
    let trim = secrets
        .iter()
        .filter(|secret| !secret.is_empty() && !bytes.ends_with(secret))
        .flat_map(|secret| (1..secret.len()).map(move |length| &secret[..length]))
        .filter(|prefix| bytes.ends_with(prefix))
        .map(<[u8]>::len)
        .max()
        .unwrap_or(0);
    bytes.truncate(bytes.len().saturating_sub(trim));
}

fn truncate_text(mut text: String) -> (String, bool) {
    if text.len() <= MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES {
        return (text, false);
    }
    let mut boundary = MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES;
    while !text.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    text.truncate(boundary);
    (text, true)
}

fn with_context(mut diagnostic: CommandDiagnostic, context: &str) -> CommandDiagnostic {
    if diagnostic.stderr.text.is_empty() {
        diagnostic.stderr.text = context.to_owned();
    }
    diagnostic
}

impl GhCliDeliveryAuthority {
    fn record_push_failure(&self, diagnostic: CommandDiagnostic) {
        let Some(reporter) = &self.operator_diagnostics else {
            return;
        };
        reporter.store.record(NewOperatorDiagnostic {
            run_id: reporter.run_id.clone(),
            code: "git_push_failed",
            operation: "delivery.git_push",
            exit_status: diagnostic.exit_status,
            stdout: diagnostic.stdout.text,
            stderr: diagnostic.stderr.text,
            stdout_truncated: diagnostic.stdout.truncated,
            stderr_truncated: diagnostic.stderr.truncated,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::sync::Arc;
    #[cfg(unix)]
    use std::time::Instant;

    #[cfg(unix)]
    use openengine_cluster_protocol::RunId;
    #[cfg(unix)]
    use openengine_cluster_testkit::assertions::AssertValue;

    #[cfg(unix)]
    use crate::native_v2_candidate::test_support::TestGitRepository;
    #[cfg(unix)]
    use crate::native_v2_delivery::GhCliAuthorityConfig;
    #[cfg(unix)]
    use crate::native_v2_target_authority::OperatorDiagnosticStore;

    #[test]
    fn truncation_never_retains_a_partial_registered_secret() {
        let secret = b"credential-value".as_slice();
        let captured = CapturedBytes {
            bytes: b"safe output\ncredential-val".to_vec(),
            retain_bytes: MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES,
            truncated: true,
            read_failed: false,
        };

        let sanitized = sanitize_output(captured, &[secret]);

        assert_eq!(sanitized.text, "safe output\n");
        assert!(sanitized.truncated);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_push_records_bounded_redacted_stdout_and_stderr() {
        let repository = TestGitRepository::delivery();
        let git_program = executable(
            repository.root.path(),
            "failing-git",
            r#"#!/bin/sh
/usr/bin/printf 'stdout-safe-ref=refs/heads/rejected\n'
/usr/bin/printf 'stdout-token=%s\n' "$GH_TOKEN"
/usr/bin/printf 'stdout-auth=%s\n' "$GIT_CONFIG_VALUE_1"
for argument in "$@"; do /usr/bin/printf 'arg=%s\n' "$argument"; done
/usr/bin/head -c 131072 /dev/zero | /usr/bin/tr '\000' x
/usr/bin/printf 'stderr-safe=remote rejected update\n' >&2
/usr/bin/printf 'stderr-token=%s\n' "$GH_TOKEN" >&2
/usr/bin/printf 'stderr-auth=%s\n' "$GIT_CONFIG_VALUE_1" >&2
/usr/bin/head -c 131072 /dev/zero | /usr/bin/tr '\000' y >&2
exit 17
"#,
        );
        let store = Arc::new(OperatorDiagnosticStore::default());
        let run_id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c991");
        let authority =
            diagnostic_authority(&repository, git_program, run_id.clone(), store.clone());
        let request = push_request(&repository);
        let token = "raw-github-token";
        let basic = encode_basic_credential(token);

        assert_eq!(
            push_branch(&authority, &request, GitHubCredential(token)).await,
            Err(GitHubAuthorityError::Rejected)
        );

        let snapshot = store.snapshot(&run_id);
        assert_eq!(snapshot.diagnostics.len(), 1);
        let diagnostic = &snapshot.diagnostics[0];
        assert_eq!(diagnostic.code, "git_push_failed");
        assert_eq!(diagnostic.operation, "delivery.git_push");
        assert_eq!(diagnostic.exit_status, Some(17));
        assert!(diagnostic.stdout_truncated);
        assert!(diagnostic.stderr_truncated);
        assert!(diagnostic.stdout.len() <= MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES);
        assert!(diagnostic.stderr.len() <= MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES);
        assert!(
            diagnostic
                .stdout
                .contains("stdout-safe-ref=refs/heads/rejected")
        );
        assert!(
            diagnostic
                .stderr
                .contains("stderr-safe=remote rejected update")
        );
        assert!(
            diagnostic
                .stdout
                .contains("arg=https://github.com/acme/project.git")
        );
        assert!(diagnostic.stdout.contains(REDACTED));
        assert!(diagnostic.stderr.contains(REDACTED));
        assert!(!diagnostic.stdout.contains(token));
        assert!(!diagnostic.stderr.contains(token));
        assert!(!diagnostic.stdout.contains(&basic));
        assert!(!diagnostic.stderr.contains(&basic));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_push_does_not_record_a_diagnostic() {
        let repository = TestGitRepository::delivery();
        let store = Arc::new(OperatorDiagnosticStore::default());
        let run_id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c991");
        let authority = diagnostic_authority(
            &repository,
            "/usr/bin/true".into(),
            run_id.clone(),
            store.clone(),
        );

        push_branch(
            &authority,
            &push_request(&repository),
            GitHubCredential("raw-github-token"),
        )
        .await
        .assert_value();

        assert!(store.snapshot(&run_id).diagnostics.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn capture_deadline_includes_drain_completion() {
        let repository = TestGitRepository::delivery();
        let program = executable(
            repository.root.path(),
            "inherited-pipe-git",
            "#!/bin/sh\n(/usr/bin/sleep 2) &\nexit 17\n",
        );
        let mut command = Command::new(program);
        command
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let started = Instant::now();

        let failure = capture(command, Duration::from_millis(20), &[])
            .await
            .expect_err("inherited output pipes exceed the deadline");

        assert_eq!(failure.error, GitHubAuthorityError::Unavailable);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(failure.diagnostic.stdout.truncated);
        assert!(failure.diagnostic.stderr.truncated);
    }

    #[cfg(unix)]
    fn push_request(repository: &TestGitRepository) -> GitHubPushRequest {
        let review = super::super::test_review_request();
        GitHubPushRequest {
            workspace: repository.workspace.clone(),
            target: review.target,
            head_branch: review.head_branch,
            head_revision: review.head_revision,
        }
    }

    #[cfg(unix)]
    fn diagnostic_authority(
        repository: &TestGitRepository,
        git_program: std::path::PathBuf,
        run_id: RunId,
        store: Arc<OperatorDiagnosticStore>,
    ) -> GhCliDeliveryAuthority {
        GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
            git_program,
            gh_program: "/usr/bin/false".into(),
            home_directory: repository.root.path().to_owned(),
            api_deadline: Duration::from_secs(10),
            push_deadline: Duration::from_secs(10),
        })
        .with_operator_diagnostics(run_id, store)
    }

    #[cfg(unix)]
    fn executable(directory: &std::path::Path, name: &str, contents: &str) -> std::path::PathBuf {
        let path = directory.join(name);
        fs::write(&path, contents).assert_value();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).assert_value();
        path
    }
}
