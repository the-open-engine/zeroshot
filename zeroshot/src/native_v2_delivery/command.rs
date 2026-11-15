//! Bounded command capture shared by trusted delivery Git operations.
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use crate::execution::process::platform::{
    self, ProcessContainment, ProcessTreeHandle, terminate_process_tree,
};
use std::time::Duration;

use openengine_cluster_protocol::CommitIdentity;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

const MAX_DIAGNOSTIC_BYTES: usize =
    4 * crate::native_v2_target_authority::MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES;

pub use crate::source_code_provider::GitCommandFailure;

/// Private command output separates protocol bytes from printable diagnostics.
/// Only trusted parsers read protocol streams; Debug and Display expose the sanitized record.
pub(crate) struct GitCommandOutput {
    raw_stdout: String,
    raw_stdout_valid_utf8: bool,
    raw_stdout_truncated: bool,
    raw_stderr: Option<String>,
    diagnostic: GitCommandFailure,
}

impl GitCommandOutput {
    pub(crate) fn stdout(&self) -> &str {
        &self.raw_stdout
    }

    pub(crate) const fn stdout_valid_utf8(&self) -> bool {
        self.raw_stdout_valid_utf8
    }

    pub(crate) const fn stdout_truncated(&self) -> bool {
        self.raw_stdout_truncated
    }

    pub(crate) fn stderr(&self) -> Option<&str> {
        self.raw_stderr.as_deref()
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
            raw_stdout_valid_utf8,
            raw_stdout_truncated,
            raw_stderr,
            diagnostic,
        } = self;
        Ok(Self {
            raw_stdout,
            raw_stdout_valid_utf8,
            raw_stdout_truncated,
            raw_stderr,
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
    capture_exchange(
        command,
        deadline,
        private_secrets,
        CaptureOptions::default(),
    )
    .await
}

pub(crate) async fn capture_sensitive_until_stopped(
    command: &mut Command,
    deadline: Duration,
    private_secrets: &[String],
    stopping: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<GitCommandOutput, GitCommandFailure> {
    capture_exchange(
        command,
        deadline,
        private_secrets,
        CaptureOptions {
            stopping,
            ..Default::default()
        },
    )
    .await
}

/// Stop-aware capture of one bounded forge response, which can exceed printable diagnostics.
pub(crate) async fn capture_sensitive_response_until_stopped(
    command: &mut Command,
    deadline: Duration,
    private_secrets: &[String],
    stopping: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<GitCommandOutput, GitCommandFailure> {
    capture_exchange(
        command,
        deadline,
        private_secrets,
        CaptureOptions {
            stdout_limit: Some(8 * 1024 * 1024),
            stopping,
            ..Default::default()
        },
    )
    .await
}

/// Sends bounded private request bytes concurrently with output and confirmed process cleanup.
pub(crate) async fn capture_sensitive_with_input(
    command: &mut Command,
    deadline: Duration,
    private_secrets: &[String],
    input: &[u8],
) -> Result<GitCommandOutput, GitCommandFailure> {
    capture_exchange(
        command,
        deadline,
        private_secrets,
        CaptureOptions {
            input: Some(input),
            ..Default::default()
        },
    )
    .await
}

/// Local inspection can retain a larger bounded protocol response without changing printable diagnostics.
pub(crate) async fn capture_sensitive_with_stdout_limit(
    command: &mut Command,
    deadline: Duration,
    private_secrets: &[String],
    stdout_limit: usize,
) -> Result<GitCommandOutput, GitCommandFailure> {
    capture_exchange(
        command,
        deadline,
        private_secrets,
        CaptureOptions {
            stdout_limit: Some(stdout_limit.min(8 * 1024 * 1024)),
            ..Default::default()
        },
    )
    .await
}

#[derive(Default)]
struct CaptureOptions<'a> {
    input: Option<&'a [u8]>,
    stdout_limit: Option<usize>,
    stopping: Option<tokio::sync::watch::Receiver<bool>>,
    /// Retains only the NUL-terminated stdout records this accepts, so a long listing can be
    /// filtered without holding all of it.
    stdout_records: Option<fn(&[u8]) -> bool>,
}

async fn capture_exchange(
    command: &mut Command,
    deadline: Duration,
    private_secrets: &[String],
    options: CaptureOptions<'_>,
) -> Result<GitCommandOutput, GitCommandFailure> {
    let input = options.input;
    let mut secrets = command_secrets(command);
    secrets.extend_from_slice(private_secrets);
    let mut diagnostic = command_diagnostic(command, &secrets);
    if options
        .stopping
        .as_ref()
        .is_some_and(|request| *request.borrow())
    {
        diagnostic.context = "command stopped before start".into();
        return Err(diagnostic);
    }
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
    let stdout_retain = options
        .stdout_limit
        .unwrap_or_else(|| input.map_or(retain, |_| 8 * 1024 * 1024));
    let completion = tokio::time::timeout(deadline, async {
        let completion = wait_and_cleanup(&mut child, &process_group, &mut diagnostic);
        let (status, (), (), sent) = tokio::join!(
            completion,
            drain_stdout(
                stdout,
                &mut captured_stdout,
                stdout_retain,
                options.stdout_records
            ),
            drain(stderr, &mut captured_stderr, retain),
            send_input(stdin, input),
        );
        sent?;
        status
    });
    let result = tokio::select! {
        result = completion => result,
        () = crate::execution::process::wait_for_stop_request(options.stopping) => {
            Ok(Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "command stopped")))
        },
    };
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
    let raw_stdout_valid_utf8 = std::str::from_utf8(&captured_stdout.bytes).is_ok();
    let raw_stdout = String::from_utf8_lossy(&captured_stdout.bytes).into_owned();
    let raw_stdout_truncated = captured_stdout.truncated;
    let raw_stderr = (!captured_stderr.truncated)
        .then(|| String::from_utf8(captured_stderr.bytes.clone()).ok())
        .flatten();
    (diagnostic.stdout, diagnostic.stdout_truncated) = sanitize(captured_stdout, &secrets);
    diagnostic.classify_stderr(&String::from_utf8_lossy(&captured_stderr.bytes));
    diagnostic.classify_stdout(&raw_stdout);
    (diagnostic.stderr, diagnostic.stderr_truncated) = sanitize(captured_stderr, &secrets);
    match result {
        Ok(Ok(_)) => Ok(GitCommandOutput {
            raw_stdout,
            raw_stdout_valid_utf8,
            raw_stdout_truncated,
            raw_stderr,
            diagnostic,
        }),
        other => {
            let cleanup = terminate_process_tree(&process_group.tree, &mut child).await;
            diagnostic.cleanup_unconfirmed |=
                !cleanup.cleanup.proves_tree_empty() || cleanup.error.is_some();
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
        || name.eq_ignore_ascii_case("HTTPS_PROXY")
        || name.eq_ignore_ascii_case("HTTP_PROXY")
        || name.eq_ignore_ascii_case("ALL_PROXY")
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
            let mut secrets =
                crate::source_code_provider::adapters::core::network::proxy_secrets(&value);
            secrets.push(super::git_auth::encode_basic_credential(&value));
            secrets
        })
        .collect()
}

async fn drain_stdout(
    reader: impl AsyncRead + Unpin,
    captured: &mut CapturedBytes,
    retain: usize,
    records: Option<fn(&[u8]) -> bool>,
) {
    match records {
        Some(keep) => drain_records(reader, captured, retain, keep).await,
        None => drain(reader, captured, retain).await,
    }
}

/// Keeps accepted NUL-terminated records, each with its terminator, and drains the rest.
/// An oversized, unterminated or unretainable record marks the capture truncated.
async fn drain_records(
    mut reader: impl AsyncRead + Unpin,
    captured: &mut CapturedBytes,
    retain: usize,
    keep: fn(&[u8]) -> bool,
) {
    const MAX_RECORD_BYTES: usize = 64 * 1024;
    let mut buffer = vec![0_u8; 8 * 1024];
    let mut record = Vec::new();
    let mut oversized = false;
    loop {
        let read = match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => {
                captured.truncated = true;
                break;
            }
        };
        for &byte in &buffer[..read] {
            if byte != 0 {
                if record.len() < MAX_RECORD_BYTES {
                    record.push(byte);
                } else {
                    oversized = true;
                }
                continue;
            }
            if oversized {
                captured.truncated = true;
            } else if keep(&record) {
                if captured.bytes.len() + record.len() < retain {
                    captured.bytes.extend_from_slice(&record);
                    captured.bytes.push(0);
                } else {
                    captured.truncated = true;
                }
            }
            record.clear();
            oversized = false;
        }
    }
    captured.truncated |= oversized || !record.is_empty();
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

pub(crate) fn redact(value: &str, secrets: &[String]) -> String {
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

/// Command-line pins for every delivery Git command. Neither feature changes repository content;
/// both would otherwise run a configured program or daemon on index refresh.
const UNIVERSAL_PINS: [&str; 2] = ["core.fsmonitor=false", "core.untrackedCache=false"];

/// Command-line pins for contained delivery, whose workspace configuration it does not author.
/// They outrank repository configuration and reach Git's own child processes. Each keeps a
/// configured program from running, or a feature that starts one from being enabled.
/// [`confine_repository_configuration`] removes the rest, so these remain defence in depth.
/// Later `-c` values still win, so a delivery step can deliberately enable one of them.
const CONTAINED_PINS: [&str; 17] = [
    "credential.helper=",
    "core.askPass=",
    "core.editor=:",
    "sequence.editor=:",
    "core.pager=cat",
    "commit.gpgSign=false",
    "tag.gpgSign=false",
    "push.gpgSign=false",
    "merge.verifySignatures=false",
    "log.showSignature=false",
    "protocol.ext.allow=never",
    "submodule.recurse=false",
    "fetch.recurseSubmodules=false",
    "push.recurseSubmodules=no",
    "diff.trustExitCode=false",
    "gc.auto=0",
    "maintenance.auto=false",
];

pub(crate) fn local_git_command(program: &Path, workspace: &Path) -> Command {
    git_base_command(program, workspace, true)
}

fn git_base_command(program: &Path, workspace: &Path, contained: bool) -> Command {
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
        .arg(format!("safe.directory={}", workspace.display()));
    for pin in UNIVERSAL_PINS
        .iter()
        .chain(if contained { &CONTAINED_PINS[..] } else { &[] })
    {
        command.arg("-c").arg(pin);
    }
    command.arg("-C").arg(workspace).stdin(Stdio::null());
    let mut environment = std::collections::BTreeMap::new();
    crate::execution::platform::process_environment(&mut environment);
    command.envs(environment);
    pin_delivery_identity(&mut command, None);
    command
}

/// Most repository configuration entries contained delivery keeps: the layout `git init`
/// writes, settings that shape the worktree and committed content, and the delivery identity.
/// None of them names a program or loads another file. `extensions.worktreeConfig` is removed
/// because it makes Git read `config.worktree` as well, and `extensions.partialClone` because it
/// makes Git fetch missing objects on demand.
const RETAINED_REPOSITORY_KEYS: [&str; 16] = [
    "core.repositoryformatversion",
    "core.filemode",
    "core.bare",
    "core.logallrefupdates",
    "core.ignorecase",
    "core.precomposeunicode",
    "core.symlinks",
    "core.autocrlf",
    "core.eol",
    "core.safecrlf",
    "core.sparsecheckout",
    "core.sparsecheckoutcone",
    "extensions.objectformat",
    "extensions.refstorage",
    "user.name",
    "user.email",
];

/// More removals than this refuse delivery rather than spend one Git process per entry.
const MAX_CONFINED_REMOVALS: usize = 256;
/// Populated repositories delivery confines in one workspace, the workspace included.
const MAX_CONFINED_REPOSITORIES: usize = 64;
/// Submodule nesting delivery confines below the workspace.
const MAX_CONFINED_SUBMODULE_DEPTH: usize = 8;

/// Whether contained delivery keeps one repository configuration entry, by Git's canonical
/// name (lowercase section and key, verbatim subsection). Remote, branch and submodule
/// locations are kept; delivery's own transport uses explicit URLs.
pub(crate) fn retained_repository_key(name: &str) -> bool {
    if RETAINED_REPOSITORY_KEYS.contains(&name) {
        return true;
    }
    let Some((section, rest)) = name.split_once('.') else {
        return false;
    };
    let Some((subsection, key)) = rest.rsplit_once('.') else {
        return false;
    };
    !subsection.is_empty()
        && matches!(
            (section, key),
            ("remote", "url" | "fetch")
                | ("branch", "remote" | "merge")
                | ("submodule", "url" | "active" | "ignore")
        )
}

/// Reduces a contained workspace's repository configuration to [`retained_repository_key`]
/// entries before a delivery Git command runs.
///
/// Git takes filter, merge and diff drivers, fsmonitor hooks, credential helpers, SSH, proxy and
/// pack programs, URL rewrites and `include` files from repository configuration, and in-tree or
/// `info/attributes` select which drivers apply to which paths. Command-line pins cannot remove a
/// driver delivery does not know the name of, or a URL rewrite. Removing every other entry can,
/// so attributes then select only Git's built-in drivers. Attributes stay as the repository
/// declares them: end-of-line and merge attributes still shape the delivered content.
///
/// The edit runs as the workspace owner, through Git, and touches only the repositories' own
/// configuration files. It is permanent: later turns see the confined configuration, and anything
/// added again is confined again before the next delivery command. Git has no way to read a
/// different repository configuration file instead.
/// `budget` is the delivery command's own deadline: confinement can interrupt it, and returns the
/// part left for the command. Confinement that leaves no time fails as itself, not as the command.
/// Local runs that use the invoking user's Git keep their configuration and their whole deadline.
pub(crate) async fn confine_repository_configuration(
    program: &Path,
    workspace: &Path,
    context: GitCommandContext<'_>,
    budget: ConfinementBudget,
) -> Result<Duration, GitCommandFailure> {
    if context.local_environment.is_some() {
        return Ok(budget.deadline);
    }
    let ConfinementBudget { deadline, stopping } = budget;
    let confinement = Confinement {
        program,
        workspace,
        context,
        end: tokio::time::Instant::now() + deadline,
        stopping,
    };
    // `git add` and worktree status start Git inside every populated submodule, which then
    // reads that repository's own configuration, so each one is confined as well.
    let mut pending = vec![(workspace.to_path_buf(), 0_usize)];
    let mut confined = 0_usize;
    while let Some((repository, depth)) = pending.pop() {
        if !has_git_entry(&repository) {
            // Nothing to confine before `git init`, and Git starts nothing in an unpopulated
            // submodule. Discovery never reaches a parent repository.
            continue;
        }
        confined += 1;
        if confined > MAX_CONFINED_REPOSITORIES || depth > MAX_CONFINED_SUBMODULE_DEPTH {
            return Err(confinement.failure(format!(
                "workspace has more than {MAX_CONFINED_REPOSITORIES} populated repositories or \
                 submodules nested deeper than {MAX_CONFINED_SUBMODULE_DEPTH}"
            )));
        }
        confinement.confine_repository(&repository).await?;
        for path in confinement.populated_gitlinks(&repository).await? {
            // Git starts nothing behind a symlinked submodule path; it reports a type change.
            // Following one could edit a repository outside the workspace.
            if !has_symlinked_component(&repository, &path) {
                pending.push((repository.join(path), depth + 1));
            }
        }
    }
    let remaining = confinement
        .end
        .saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        return Err(confinement.failure(
            "confining workspace Git configuration used the delivery command's whole deadline"
                .to_owned(),
        ));
    }
    Ok(remaining)
}

/// A delivery Git command built after its workspace configuration was confined, and the part of
/// the caller's deadline that confinement left for it.
#[derive(Debug)]
pub(crate) struct ConfinedCommand {
    command: Command,
    deadline: Duration,
}

impl ConfinedCommand {
    pub(crate) const fn new(command: Command, deadline: Duration) -> Self {
        Self { command, deadline }
    }

    /// What remains of the caller's deadline for this command.
    pub(crate) const fn deadline(&self) -> Duration {
        self.deadline
    }

    pub(crate) fn into_parts(self) -> (Command, Duration) {
        (self.command, self.deadline)
    }

    pub(crate) async fn capture(&mut self) -> Result<GitCommandFailure, GitCommandFailure> {
        capture(&mut self.command, self.deadline).await
    }

    pub(crate) async fn capture_sensitive(
        &mut self,
    ) -> Result<GitCommandOutput, GitCommandFailure> {
        capture_sensitive(&mut self.command, self.deadline, &[]).await
    }
}

impl std::ops::Deref for ConfinedCommand {
    type Target = Command;

    fn deref(&self) -> &Command {
        &self.command
    }
}

impl std::ops::DerefMut for ConfinedCommand {
    fn deref_mut(&mut self) -> &mut Command {
        &mut self.command
    }
}

/// Distinct entry names contained delivery removes.
fn unretained(names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|name| !retained_repository_key(name))
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The time and stop request one confinement may use.
pub(crate) struct ConfinementBudget {
    pub(crate) deadline: Duration,
    pub(crate) stopping: Option<tokio::sync::watch::Receiver<bool>>,
}

impl ConfinementBudget {
    pub(crate) const fn within(deadline: Duration) -> Self {
        Self {
            deadline,
            stopping: None,
        }
    }
}

fn has_git_entry(repository: &Path) -> bool {
    !std::fs::symlink_metadata(repository.join(".git"))
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

fn has_symlinked_component(repository: &Path, path: &Path) -> bool {
    let mut current = repository.to_path_buf();
    path.components().any(|component| {
        current.push(component);
        std::fs::symlink_metadata(&current).is_ok_and(|metadata| metadata.file_type().is_symlink())
    })
}

/// One bounded, stoppable confinement of a workspace and its populated submodules.
struct Confinement<'a> {
    program: &'a Path,
    workspace: &'a Path,
    context: GitCommandContext<'a>,
    end: tokio::time::Instant,
    stopping: Option<tokio::sync::watch::Receiver<bool>>,
}

impl Confinement<'_> {
    async fn confine_repository(&self, repository: &Path) -> Result<(), GitCommandFailure> {
        let removed = unretained(&self.names(repository).await?);
        if removed.is_empty() {
            return Ok(());
        }
        if removed.len() > MAX_CONFINED_REMOVALS {
            return Err(self.failure(format!(
                "workspace Git configuration has {} entries delivery does not keep; at most \
                 {MAX_CONFINED_REMOVALS} are removed",
                removed.len()
            )));
        }
        for name in &removed {
            let mut command = self.command(repository);
            command.args(["config", "--local", "--unset-all", name]);
            self.capture(&mut command, CaptureOptions::default())
                .await?
                .require_success()
                .map_err(|failure| {
                    failure.with_context(
                        "could not remove workspace Git configuration before delivery",
                    )
                })?;
        }
        if let Some(name) = unretained(&self.names(repository).await?).first() {
            return Err(self.failure(format!(
                "workspace Git configuration still sets `{name}` after confinement"
            )));
        }
        Ok(())
    }

    /// Every entry name in the repository configuration file, without following includes.
    async fn names(&self, repository: &Path) -> Result<Vec<String>, GitCommandFailure> {
        let mut command = self.command(repository);
        command.args([
            "config",
            "--local",
            "--no-includes",
            "--list",
            "--name-only",
            "-z",
        ]);
        let output = self
            .capture(
                &mut command,
                CaptureOptions {
                    stdout_limit: Some(8 * 1024 * 1024),
                    ..Default::default()
                },
            )
            .await?;
        let output = output.require_success().map_err(|failure| {
            failure.with_context("could not read workspace Git configuration before delivery")
        })?;
        if output.stdout_truncated() || !output.stdout_valid_utf8() {
            return Err(output
                .into_diagnostic()
                .with_context("workspace Git configuration names are unreadable"));
        }
        Ok(output
            .stdout()
            .split_terminator('\0')
            .map(str::to_owned)
            .collect())
    }

    /// The index's submodule paths. Git validates index paths; anything else fails closed.
    async fn populated_gitlinks(
        &self,
        repository: &Path,
    ) -> Result<Vec<PathBuf>, GitCommandFailure> {
        let mut command = self.command(repository);
        command.args(["ls-files", "--stage", "-z"]);
        let output = self
            .capture(
                &mut command,
                CaptureOptions {
                    stdout_limit: Some(1024 * 1024),
                    stdout_records: Some(|record| record.starts_with(b"160000 ")),
                    ..Default::default()
                },
            )
            .await?
            .require_success()
            .map_err(|failure| {
                failure.with_context("could not list workspace submodules before delivery")
            })?;
        let unreadable = || {
            command_diagnostic(&command, &[])
                .with_context("workspace submodule paths are unreadable or exceed delivery's bound")
        };
        if output.stdout_truncated() || !output.stdout_valid_utf8() {
            return Err(unreadable());
        }
        let mut paths = std::collections::BTreeSet::new();
        for record in output.stdout().split_terminator('\0') {
            let path = record
                .split_once('\t')
                .map(|(_, path)| Path::new(path))
                .filter(|path| {
                    path.components()
                        .all(|component| matches!(component, std::path::Component::Normal(_)))
                        && path.components().next().is_some()
                })
                .ok_or_else(unreadable)?;
            paths.insert(path.to_path_buf());
        }
        Ok(paths.into_iter().collect())
    }

    /// Captures one confinement command within the remaining overall budget.
    async fn capture(
        &self,
        command: &mut Command,
        options: CaptureOptions<'_>,
    ) -> Result<GitCommandOutput, GitCommandFailure> {
        let remaining = self
            .end
            .saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            let mut failure = command_diagnostic(command, &[]);
            failure.timed_out = true;
            failure.context =
                "workspace Git configuration confinement exceeded its deadline".into();
            return Err(failure);
        }
        capture_exchange(
            command,
            remaining,
            &[],
            CaptureOptions {
                stopping: self.stopping.clone(),
                ..options
            },
        )
        .await
    }

    fn command(&self, repository: &Path) -> Command {
        // Contained `git_command` already stops discovery at the repository's parent.
        git_command(self.program, repository, self.context)
    }

    fn failure(&self, message: String) -> GitCommandFailure {
        let mut failure = command_diagnostic(&self.command(self.workspace), &[]);
        failure.context = message.into_boxed_str();
        failure
    }
}

/// Delivery commits use the host-selected identity, or the fixed product identity without one.
pub(crate) fn delivery_identity(identity: Option<&CommitIdentity>) -> (&str, &str) {
    identity.map_or(("Zeroshot", "delivery@zeroshot.invalid"), |identity| {
        (identity.name(), identity.email())
    })
}

/// `-c` values that also make the delivery identity the configured Git user for one command.
pub(crate) fn identity_configuration(identity: Option<&CommitIdentity>) -> [String; 2] {
    let (name, email) = delivery_identity(identity);
    [format!("user.name={name}"), format!("user.email={email}")]
}

fn pin_delivery_identity(command: &mut Command, identity: Option<&CommitIdentity>) {
    let (name, email) = delivery_identity(identity);
    command
        .env("GIT_AUTHOR_NAME", name)
        .env("GIT_AUTHOR_EMAIL", email)
        .env("GIT_COMMITTER_NAME", name)
        .env("GIT_COMMITTER_EMAIL", email);
}

/// Repository-selection controls never override an engine-selected workspace. Native Git
/// configuration and authentication settings continue to apply inside that workspace. Platform's
/// machine credential belongs only to target HTTP and is excluded from every Git subprocess.
pub(crate) fn remove_repository_environment(command: &mut std::process::Command) {
    for variable in [
        "ZEROSHOT_API_KEY",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CEILING_DIRECTORIES",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
        "GIT_NAMESPACE",
    ] {
        command.env_remove(variable);
    }
}

/// Trusted subprocess context shared by local workspace and transport Git commands.
#[derive(Clone, Copy)]
pub(crate) struct GitCommandContext<'a> {
    pub(crate) identity: Option<crate::execution::process::HostedProcessIdentity>,
    pub(crate) local_environment: Option<&'a std::collections::BTreeMap<String, String>>,
    pub(crate) commit_identity: Option<&'a CommitIdentity>,
}

/// Selects the same private native Git context for local workspace and transport operations.
/// An absent snapshot retains hosted isolation; the controller's ambient environment is never used.
pub(crate) fn git_command(
    program: &Path,
    workspace: &Path,
    context: GitCommandContext<'_>,
) -> Command {
    let GitCommandContext {
        identity,
        local_environment,
        commit_identity,
    } = context;
    let mut command = with_hosted_identity(
        git_base_command(program, workspace, local_environment.is_none()),
        identity,
    );
    if let Some(environment) = local_environment {
        command.envs(environment);
        if identity.is_none() {
            // Resolve native relative PATH entries in the selected workspace.
            command.current_dir(workspace);
        }
        for variable in ["GIT_CONFIG_NOSYSTEM", "GIT_CONFIG_GLOBAL"] {
            #[cfg(windows)]
            let supplied = environment
                .keys()
                .any(|name| name.eq_ignore_ascii_case(variable));
            #[cfg(not(windows))]
            let supplied = environment.contains_key(variable);
            if !supplied {
                command.env_remove(variable);
            }
        }
    }
    remove_repository_environment(command.as_std_mut());
    if local_environment.is_none() {
        // Contained delivery Git uses the workspace's own repository or none. Without `.git`
        // there is nothing to confine, so discovery must not continue into a parent repository.
        if let Some(parent) = workspace
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            command.env("GIT_CEILING_DIRECTORIES", parent);
        }
    }
    pin_delivery_identity(&mut command, commit_identity);
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("SSH_ASKPASS_REQUIRE", "never");
    command
}

/// Hosted Git writes as the allocated workspace owner, including on failed or cancelled operations.
/// Admission excludes concurrent writers and their cleanup completes before delivery starts.
pub(crate) fn local_git_command_with_identity(
    program: &Path,
    workspace: &Path,
    identity: Option<crate::execution::process::HostedProcessIdentity>,
) -> Command {
    with_hosted_identity(local_git_command(program, workspace), identity)
}

fn with_hosted_identity(
    mut command: Command,
    identity: Option<crate::execution::process::HostedProcessIdentity>,
) -> Command {
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

pub(crate) fn diagnostic_text_with_secrets(
    bytes: Vec<u8>,
    truncated: bool,
    secrets: &[String],
) -> (String, bool) {
    sanitize(CapturedBytes { bytes, truncated }, secrets)
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
