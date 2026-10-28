//! Bounded command capture shared by trusted delivery Git operations.
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use crate::execution::process::platform::{
    self, ProcessContainment, ProcessTreeHandle, terminate_process_tree,
};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

const MAX_DIAGNOSTIC_BYTES: usize =
    4 * crate::native_v2_target_authority::MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES;

pub use crate::source_code_provider::GitCommandFailure;

/// Private command output separates protocol bytes from printable diagnostics.
/// Only trusted Git parsers read stdout; Debug and Display expose the sanitized record.
pub(crate) struct GitCommandOutput {
    raw_stdout: String,
    raw_stdout_truncated: bool,
    diagnostic: GitCommandFailure,
}

impl GitCommandOutput {
    pub(crate) fn stdout(&self) -> &str {
        &self.raw_stdout
    }

    pub(crate) const fn stdout_truncated(&self) -> bool {
        self.raw_stdout_truncated
    }

    pub(crate) const fn diagnostic(&self) -> &GitCommandFailure {
        &self.diagnostic
    }

    pub(crate) fn into_diagnostic(self) -> GitCommandFailure {
        self.diagnostic
    }

    pub(crate) fn require_success(self) -> Result<Self, GitCommandFailure> {
        let Self {
            raw_stdout,
            raw_stdout_truncated,
            diagnostic,
        } = self;
        Ok(Self {
            raw_stdout,
            raw_stdout_truncated,
            diagnostic: diagnostic.require_success()?,
        })
    }
}

impl std::fmt::Debug for GitCommandOutput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GitCommandOutput")
            .field("diagnostic", &self.diagnostic)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for GitCommandOutput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.diagnostic, formatter)
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
    capture_with_secrets(command, deadline, &[]).await
}

pub(crate) async fn capture_with_secrets(
    command: &mut Command,
    deadline: Duration,
    private_secrets: &[String],
) -> Result<GitCommandFailure, GitCommandFailure> {
    capture_sensitive(command, deadline, private_secrets)
        .await
        .map(GitCommandOutput::into_diagnostic)
}

pub(crate) async fn capture_sensitive(
    command: &mut Command,
    deadline: Duration,
    private_secrets: &[String],
) -> Result<GitCommandOutput, GitCommandFailure> {
    capture_exchange(command, deadline, private_secrets, None).await
}

/// Sends bounded private request bytes concurrently with output and confirmed process cleanup.
pub(crate) async fn capture_sensitive_with_input(
    command: &mut Command,
    deadline: Duration,
    private_secrets: &[String],
    input: &[u8],
) -> Result<GitCommandOutput, GitCommandFailure> {
    capture_exchange(command, deadline, private_secrets, Some(input)).await
}

async fn capture_exchange(
    command: &mut Command,
    deadline: Duration,
    private_secrets: &[String],
    input: Option<&[u8]>,
) -> Result<GitCommandOutput, GitCommandFailure> {
    let mut secrets = command_secrets(command);
    secrets.extend_from_slice(private_secrets);
    let mut diagnostic = command_diagnostic(command, &secrets);
    configure_capture(command, input.is_some());
    let (mut child, process_group) = spawn_contained(command).map_err(|error| {
        diagnostic.context = format!("could not start contained command: {error}").into_boxed_str();
        diagnostic.clone()
    })?;
    let (stdout, stderr, stdin) = take_pipes(&mut child).ok_or_else(|| diagnostic.clone())?;
    let mut captured_stdout = CapturedBytes::default();
    let mut captured_stderr = CapturedBytes::default();
    let retain = MAX_DIAGNOSTIC_BYTES + secrets.iter().map(String::len).max().unwrap_or(0);
    // The input exchange serves bounded forge JSON, whose private protocol bytes can be
    // larger than printable diagnostics. Existing Git captures retain their prior bound.
    let stdout_retain = input.map_or(retain, |_| 8 * 1024 * 1024);
    let result = tokio::time::timeout(deadline, async {
        let completion = wait_and_cleanup(&mut child, &process_group, &mut diagnostic);
        let (status, (), (), sent) = tokio::join!(
            completion,
            drain(stdout, &mut captured_stdout, stdout_retain),
            drain(stderr, &mut captured_stderr, retain),
            send_input(stdin, input),
        );
        sent?;
        status
    })
    .await;
    diagnostic.exit_status = result
        .as_ref()
        .ok()
        .and_then(|r| r.as_ref().ok())
        .and_then(ExitStatus::code);
    if result.is_err() {
        diagnostic.timed_out = true;
        captured_stdout.truncated = true;
        captured_stderr.truncated = true;
    }
    let raw_stdout = String::from_utf8_lossy(&captured_stdout.bytes).into_owned();
    let raw_stdout_truncated = captured_stdout.truncated;
    (diagnostic.stdout, diagnostic.stdout_truncated) = sanitize(captured_stdout, &secrets);
    diagnostic.classify_stderr(&String::from_utf8_lossy(&captured_stderr.bytes));
    (diagnostic.stderr, diagnostic.stderr_truncated) = sanitize(captured_stderr, &secrets);
    match result {
        Ok(Ok(_)) => Ok(GitCommandOutput {
            raw_stdout,
            raw_stdout_truncated,
            diagnostic,
        }),
        other => {
            let cleanup = terminate_process_tree(&process_group.tree, &mut child).await;
            diagnostic.cleanup_unconfirmed |= !cleanup.cleanup.proves_tree_empty();
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

fn configure_capture(command: &mut Command, sends_input: bool) {
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if sends_input {
        command.stdin(Stdio::piped());
    }
}

fn take_pipes(
    child: &mut tokio::process::Child,
) -> Option<(
    tokio::process::ChildStdout,
    tokio::process::ChildStderr,
    Option<tokio::process::ChildStdin>,
)> {
    Some((
        child.stdout.take()?,
        child.stderr.take()?,
        child.stdin.take(),
    ))
}

async fn send_input(
    stdin: Option<tokio::process::ChildStdin>,
    input: Option<&[u8]>,
) -> std::io::Result<()> {
    if let Some(input) = input {
        let mut stdin = stdin.ok_or_else(|| std::io::Error::other("command stdin unavailable"))?;
        stdin.write_all(input).await?;
        stdin.shutdown().await?;
    }
    Ok(())
}

async fn wait_and_cleanup(
    child: &mut tokio::process::Child,
    group: &ProcessGroup,
    diagnostic: &mut GitCommandFailure,
) -> std::io::Result<ExitStatus> {
    let status = child.wait().await;
    // Main-process exit is not stream completion: a helper may still hold both pipes.
    let cleanup = terminate_process_tree(&group.tree, child).await;
    diagnostic.cleanup_unconfirmed = !cleanup.cleanup.proves_tree_empty();
    if diagnostic.cleanup_unconfirmed || cleanup.error.is_some() {
        return Err(std::io::Error::other(cleanup.error.unwrap_or_else(|| {
            "Git process cleanup is unconfirmed".to_owned()
        })));
    }
    status
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
        timed_out: false,
        cleanup_unconfirmed: false,
        classification: None,
    }
}

pub(super) fn native_credential_name(name: &str) -> bool {
    #[cfg(windows)]
    let name = name.to_ascii_uppercase();
    name == "GIT_USERNAME"
        || name.ends_with("_TOKEN")
        || name.ends_with("_PASSWORD")
        || name.ends_with("_API_KEY")
}

fn command_secrets(command: &Command) -> Vec<String> {
    command
        .as_std()
        .get_envs()
        .filter(|(name, _)| name.to_str().is_some_and(native_credential_name))
        .filter_map(|(_, value)| value)
        .filter(|value| !value.is_empty())
        .flat_map(|value| {
            let value = value.to_string_lossy().into_owned();
            [super::git_auth::encode_basic_credential(&value), value]
        })
        .collect()
}

async fn drain(mut reader: impl AsyncRead + Unpin, captured: &mut CapturedBytes, retain: usize) {
    // Keep pipe storage out of the nested admission and delivery futures.
    let mut buffer = vec![0_u8; 8 * 1024];
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

pub(super) fn redact(value: &str, secrets: &[String]) -> String {
    let mut secrets = secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .collect::<Vec<_>>();
    secrets
        .sort_unstable_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    secrets.dedup();
    let mut redacted = String::with_capacity(value.len());
    let mut skip_until = 0;
    for (position, character) in value.char_indices() {
        if position < skip_until {
            continue;
        }
        if let Some(secret) = secrets
            .iter()
            .find(|secret| value[position..].starts_with(secret.as_str()))
        {
            redacted.push_str("[REDACTED]");
            skip_until = position + secret.len();
        } else {
            redacted.push(character);
        }
    }
    redacted
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
        .arg("gc.autoDetach=false")
        .arg("-c")
        .arg(format!("safe.directory={}", workspace.display()))
        .arg("-C")
        .arg(workspace)
        .stdin(Stdio::null());
    let mut environment = std::collections::BTreeMap::new();
    crate::execution::platform::process_environment(&mut environment);
    command.envs(environment);
    pin_delivery_identity(&mut command);
    command
}

pub(super) fn pin_delivery_identity(command: &mut Command) {
    command
        .env("GIT_AUTHOR_NAME", "Zeroshot")
        .env("GIT_AUTHOR_EMAIL", "delivery@zeroshot.invalid")
        .env("GIT_COMMITTER_NAME", "Zeroshot")
        .env("GIT_COMMITTER_EMAIL", "delivery@zeroshot.invalid");
}

/// Hosted Git writes as the allocated workspace owner, including on failed or cancelled operations.
/// Admission excludes concurrent writers and their cleanup completes before delivery starts.
pub(crate) fn local_git_command_with_identity(
    program: &Path,
    workspace: &Path,
    identity: Option<crate::execution::process::HostedProcessIdentity>,
) -> Command {
    let mut command = local_git_command(program, workspace);
    if let Some(identity) = identity {
        // The launcher may be in a root-only directory. The existing containment clears
        // supplementary groups/capabilities and prevents helpers from regaining privilege.
        command.current_dir("/");
        identity.configure_command(&mut command);
    }
    command
}

fn spawn_contained(
    command: &mut Command,
) -> std::io::Result<(tokio::process::Child, ProcessGroup)> {
    let registration = platform::register_process_tree_for(ProcessContainment::ProcessGroup)?;
    platform::configure_process(command, ProcessContainment::ProcessGroup);
    let mut child = command.spawn()?;
    let tree = platform::capture_process_tree(registration, &mut child)?;
    let process_group = ProcessGroup {
        tree,
        #[cfg(unix)]
        pid: child.id(),
    };
    Ok((child, process_group))
}

// Explicit completion awaits the shared containment cleanup. Drop also stops helpers when the
// caller cancels capture; the enclosing hosted writer domain confirms its remaining cleanup.
struct ProcessGroup {
    tree: ProcessTreeHandle,
    #[cfg(unix)]
    pid: Option<u32>,
}

#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Some(pid) = self
            .pid
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|pid| *pid > 0)
        {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}

pub(crate) fn diagnostic_text(bytes: Vec<u8>, truncated: bool, token: &str) -> (String, bool) {
    sanitize(
        CapturedBytes { bytes, truncated },
        &[
            token.to_owned(),
            super::git_auth::encode_basic_credential(token),
        ],
    )
}

#[cfg(test)]
#[path = "command/tests.rs"]
mod tests;

pub(crate) fn preparation_failure(
    command: &Command,
    context: impl std::fmt::Display,
) -> GitCommandFailure {
    let mut failure = command_diagnostic(command, &command_secrets(command));
    failure.context =
        format!("private Git transport preparation failed: {context}").into_boxed_str();
    failure
}
