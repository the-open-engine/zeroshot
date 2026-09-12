//! Bounded command capture shared by trusted delivery Git operations.
use std::fmt;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

const MAX_DIAGNOSTIC_BYTES: usize =
    4 * crate::native_v2_target_authority::MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitCommandFailure {
    command: Box<str>,
    working_directory: Box<str>,
    pub(crate) exit_status: Option<i32>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
    context: Box<str>,
}

impl fmt::Display for GitCommandFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "command: {}\nworkingDirectory: {}\nexitStatus: {:?}\n{}\nstdout{}:\n{}\nstderr{}:\n{}",
            self.command,
            self.working_directory,
            self.exit_status,
            self.context,
            if self.stdout_truncated {
                " (truncated)"
            } else {
                ""
            },
            self.stdout,
            if self.stderr_truncated {
                " (truncated)"
            } else {
                ""
            },
            self.stderr
        )
    }
}

impl std::error::Error for GitCommandFailure {}

impl GitCommandFailure {
    pub(crate) fn operator_stderr(&self) -> String {
        format!(
            "stderr{}:\n{}\ncommand: {}\nworkingDirectory: {}\nexitStatus: {:?}\n{}",
            if self.stderr_truncated {
                " (truncated)"
            } else {
                ""
            },
            self.stderr,
            self.command,
            self.working_directory,
            self.exit_status,
            self.context,
        )
    }

    pub(crate) fn require_success(self) -> Result<Self, Self> {
        if self.exit_status == Some(0) {
            Ok(self)
        } else {
            Err(self)
        }
    }
}

#[derive(Default)]
struct CapturedBytes {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Captures both pipes through process exit, bounds memory, and kills the child when dropped.
/// Nonzero statuses are returned intact so callers can recognize command-specific expected exits.
pub(crate) async fn capture(
    command: &mut Command,
    deadline: Duration,
) -> Result<GitCommandFailure, GitCommandFailure> {
    let secrets = command_secrets(command);
    let mut diagnostic = command_diagnostic(command, &secrets);
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    let mut child = command.spawn().map_err(|error| {
        diagnostic.context = format!("could not start command: {error}").into_boxed_str();
        diagnostic.clone()
    })?;
    let _process_group = ProcessGroup(child.id());
    let Some(stdout) = child.stdout.take() else {
        return Err(diagnostic);
    };
    let Some(stderr) = child.stderr.take() else {
        return Err(diagnostic);
    };
    let mut captured_stdout = CapturedBytes::default();
    let mut captured_stderr = CapturedBytes::default();
    let retain = MAX_DIAGNOSTIC_BYTES + secrets.iter().map(String::len).max().unwrap_or(0);
    let result = tokio::time::timeout(deadline, async {
        let (status, (), ()) = tokio::join!(
            child.wait(),
            drain(stdout, &mut captured_stdout, retain),
            drain(stderr, &mut captured_stderr, retain),
        );
        status
    })
    .await;
    diagnostic.exit_status = result
        .as_ref()
        .ok()
        .and_then(|r| r.as_ref().ok())
        .and_then(ExitStatus::code);
    if result.is_err() {
        captured_stdout.truncated = true;
        captured_stderr.truncated = true;
    }
    (diagnostic.stdout, diagnostic.stdout_truncated) = sanitize(captured_stdout, &secrets);
    (diagnostic.stderr, diagnostic.stderr_truncated) = sanitize(captured_stderr, &secrets);
    match result {
        Ok(Ok(_)) => Ok(diagnostic),
        other => {
            let _ = child.start_kill();
            diagnostic.context = match other {
                Err(_) => format!("command timed out after {} seconds", deadline.as_secs()),
                Ok(Err(error)) => format!("command status unavailable: {error}"),
                Ok(Ok(_)) => unreachable!(),
            }
            .into_boxed_str();
            Err(diagnostic)
        }
    }
}

fn command_diagnostic(command: &Command, secrets: &[String]) -> GitCommandFailure {
    let command = command.as_std();
    let arguments = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|value| format!("{:?}", value.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");
    GitCommandFailure {
        command: redact(&arguments, secrets).into_boxed_str(),
        working_directory: command
            .get_current_dir()
            .map_or_else(
                || "inherited; repository selected by -C".to_owned(),
                |path| path.display().to_string(),
            )
            .into_boxed_str(),
        exit_status: None,
        stdout: String::new(),
        stderr: String::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        context: Box::<str>::default(),
    }
}

fn command_secrets(command: &Command) -> Vec<String> {
    let token = command.as_std().get_envs().find_map(|(name, value)| {
        (name == "GH_TOKEN")
            .then_some(value)
            .flatten()
            .map(|v| v.to_string_lossy().into_owned())
    });
    match token {
        Some(token) => vec![super::git_auth::encode_basic_credential(&token), token],
        None => Vec::new(),
    }
}

async fn drain(mut reader: impl AsyncRead + Unpin, captured: &mut CapturedBytes, retain: usize) {
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => {
                captured.truncated = true;
                break;
            }
        };
        let count = retain.saturating_sub(captured.bytes.len()).min(read);
        captured.bytes.extend_from_slice(&buffer[..count]);
        captured.truncated |= count < read;
    }
}

fn sanitize(mut captured: CapturedBytes, secrets: &[String]) -> (String, bool) {
    if captured.truncated {
        let trim = secrets
            .iter()
            .filter(|s| !s.is_empty() && !captured.bytes.ends_with(s.as_bytes()))
            .flat_map(|s| (1..s.len()).map(move |n| &s.as_bytes()[..n]))
            .filter(|prefix| captured.bytes.ends_with(prefix))
            .map(<[u8]>::len)
            .max()
            .unwrap_or(0);
        captured
            .bytes
            .truncate(captured.bytes.len().saturating_sub(trim));
    }
    let mut text = redact(&String::from_utf8_lossy(&captured.bytes), secrets);
    let truncated = captured.truncated || text.len() > MAX_DIAGNOSTIC_BYTES;
    let mut boundary = text.len().min(MAX_DIAGNOSTIC_BYTES);
    while !text.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    text.truncate(boundary);
    (text, truncated)
}

fn redact(value: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .filter(|s| !s.is_empty())
        .fold(value.to_owned(), |text, secret| {
            text.replace(secret, "[REDACTED]")
        })
}

pub(crate) fn local_git_command(program: &Path, workspace: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .kill_on_drop(true)
        .env_clear()
        .env("LANG", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .arg("-c")
        .arg("maintenance.autoDetach=false")
        .arg("-c")
        .arg(format!("safe.directory={}", workspace.display()))
        .arg("-C")
        .arg(workspace)
        .stdin(Stdio::null());
    command
}

// Dropping a cancelled capture must stop helpers such as git-remote-https as well as Git itself.
struct ProcessGroup(Option<u32>);

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self
            .0
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|pid| *pid > 0)
        {
            // The process was started in its own group; no launcher process belongs to this group.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}

pub(super) fn diagnostic_text(bytes: Vec<u8>, truncated: bool, token: &str) -> (String, bool) {
    sanitize(
        CapturedBytes { bytes, truncated },
        &[
            token.to_owned(),
            super::git_auth::encode_basic_credential(token),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let task =
            tokio::spawn(async move { capture(&mut command, Duration::from_secs(30)).await });
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
}
