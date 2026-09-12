use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::Duration;

use openengine_cluster_protocol::RunId;

use tokio::process::Command;
use tokio::time::Instant;

use crate::execution::process::{HostedProcessPool, HostedProcessScope};
use crate::native_v2_delivery::DeliveryTarget;
use crate::native_v2_delivery::command::{GitCommandFailure, capture, local_git_command};
use crate::native_v2_portable_controller::WorkspaceIdentity;
use crate::native_v2_target_authority::{NewOperatorDiagnostic, OperatorDiagnosticStore};
use crate::native_v2_delivery::git_auth::encode_basic_credential;
use crate::native_v2_contract::ResolvedSource;

#[cfg(all(test, unix))]
mod tests;

const GIT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_CHECKOUT_ATTEMPTS: u32 = 3;
const CHECKOUT_RETRY_DELAY: Duration = Duration::from_secs(1);

pub(super) struct RepositoryInstall<'a> {
    pub git_program: &'a Path,
    pub source: &'a OsStr,
    pub resolved: &'a ResolvedSource,
    pub workspace: &'a Path,
    pub process_pool: HostedProcessPool,
    pub github_token: Option<&'a str>,
}

pub(super) async fn install_repository(
    request: RepositoryInstall<'_>,
) -> Result<DeliveryTarget, RepositoryInstallError> {
    install_before_deadline(request, Instant::now() + GIT_TIMEOUT, CHECKOUT_RETRY_DELAY).await
}

async fn install_before_deadline(
    request: RepositoryInstall<'_>,
    deadline: Instant,
    retry_delay: Duration,
) -> Result<DeliveryTarget, RepositoryInstallError> {
    let identity = pristine_workspace(request.workspace)?;
    let writer = request
        .process_pool
        .identity(HostedProcessScope::Writer)
        .map_err(|_| RepositoryInstallError::Workspace("writer identity is unavailable"))?;
    let git = GitProcess {
        program: request.git_program,
        token: request.github_token,
        uid: writer.uid(),
        gid: writer.gid(),
        deadline,
    };
    let mut previous_failure = None;
    for attempt in 1..=MAX_CHECKOUT_ATTEMPTS {
        let error = match install_once(&git, &request).await {
            Ok(target) => return Ok(target),
            Err(RepositoryInstallError::Deadline) => {
                return Err(previous_failure.unwrap_or(RepositoryInstallError::Deadline));
            }
            Err(error) => error,
        };
        if !error.permits_retry(
            attempt,
            deadline.saturating_duration_since(Instant::now()),
            retry_delay,
        ) {
            return Err(error);
        }
        tokio::time::sleep(retry_delay).await;
        if Instant::now() >= deadline {
            return Err(error);
        }
        if let Err(cleanup) = reset_workspace(request.workspace, &identity, deadline) {
            return Err(RepositoryInstallError::Recovery {
                failure: Box::new(error),
                message: cleanup.to_string(),
            });
        }
        previous_failure = Some(error);
    }
    unreachable!("checkout returns on success or the final attempt")
}

async fn install_once(
    git: &GitProcess<'_>,
    request: &RepositoryInstall<'_>,
) -> Result<DeliveryTarget, RepositoryInstallError> {
    initialize_repository(git, request).await?;
    fetch_source(git, request.workspace, request.resolved).await?;
    checkout_revision(git, request.workspace, request.resolved.revision.as_str()).await?;
    let revision = git
        .capture(
            request.workspace,
            &[OsString::from("rev-parse"), OsString::from("HEAD")],
        )
        .await?;
    if revision.trim() != request.resolved.revision.as_str() {
        return Err(RepositoryInstallError::RevisionMismatch {
            expected: request.resolved.revision.as_str().to_owned(),
            observed: revision.trim().to_owned(),
        });
    }
    DeliveryTarget::new(
        request.resolved.repository.as_str(),
        request.resolved.branch.as_str(),
        revision.trim(),
    )
    .map_err(|_| RepositoryInstallError::Workspace("resolved delivery target is invalid"))
}

fn pristine_workspace(workspace: &Path) -> Result<WorkspaceIdentity, RepositoryInstallError> {
    let identity = WorkspaceIdentity::capture(workspace)
        .map_err(|_| RepositoryInstallError::Workspace("staging directory is unavailable"))?;
    if std::fs::read_dir(workspace)?.next().transpose()?.is_some() {
        return Err(RepositoryInstallError::Workspace(
            "staging directory is not empty",
        ));
    }
    Ok(identity)
}

fn reset_workspace(
    workspace: &Path,
    identity: &WorkspaceIdentity,
    deadline: Instant,
) -> Result<(), RepositoryInstallError> {
    check_deadline(deadline)?;
    if !identity.is_current(workspace) {
        return Err(RepositoryInstallError::Workspace(
            "staging directory was replaced",
        ));
    }
    // No agent has run yet. Keep the admitted directory inode and permissions, removing only
    // entries from this fresh checkout. Neither file_type nor remove_dir_all follows symlinks.
    for entry in std::fs::read_dir(workspace)? {
        check_deadline(deadline)?;
        remove_checkout_entry(entry?)?;
    }
    check_deadline(deadline)
}

fn remove_checkout_entry(entry: std::fs::DirEntry) -> Result<(), std::io::Error> {
    if entry.file_type()?.is_dir() {
        std::fs::remove_dir_all(entry.path())
    } else {
        std::fs::remove_file(entry.path())
    }
}

fn check_deadline(deadline: Instant) -> Result<(), RepositoryInstallError> {
    if Instant::now() >= deadline {
        Err(RepositoryInstallError::Deadline)
    } else {
        Ok(())
    }
}

async fn initialize_repository(
    git: &GitProcess<'_>,
    request: &RepositoryInstall<'_>,
) -> Result<(), RepositoryInstallError> {
    git.run(
        None,
        &[
            OsString::from("init"),
            OsString::from("--quiet"),
            request.workspace.as_os_str().to_owned(),
        ],
    )
    .await?;
    let arguments = [
        OsString::from("remote"),
        OsString::from("add"),
        OsString::from("origin"),
        request.source.to_owned(),
    ];
    git.run(Some(request.workspace), &arguments).await
}

async fn fetch_source(
    git: &GitProcess<'_>,
    workspace: &Path,
    source: &ResolvedSource,
) -> Result<(), RepositoryInstallError> {
    git.run(
        Some(workspace),
        &[
            OsString::from("fetch"),
            OsString::from("--no-tags"),
            OsString::from("origin"),
            OsString::from(format!("refs/heads/{}", source.branch.as_str())),
            OsString::from(source.revision.as_str()),
        ],
    )
    .await
}

async fn checkout_revision(
    git: &GitProcess<'_>,
    workspace: &Path,
    revision: &str,
) -> Result<(), RepositoryInstallError> {
    git.run(
        Some(workspace),
        &[
            OsString::from("checkout"),
            OsString::from("--detach"),
            OsString::from(revision),
        ],
    )
    .await
}

fn github_source(repository: &str) -> OsString {
    OsString::from(format!("https://github.com/{repository}.git"))
}

pub(super) fn production_source(repository: &str) -> OsString {
    github_source(repository)
}

struct GitProcess<'a> {
    program: &'a Path,
    token: Option<&'a str>,
    uid: u32,
    gid: u32,
    deadline: Instant,
}

impl GitProcess<'_> {
    async fn run(
        &self,
        workspace: Option<&Path>,
        arguments: &[OsString],
    ) -> Result<(), RepositoryInstallError> {
        self.capture_at(workspace, arguments).await.map(|_| ())
    }

    async fn capture(
        &self,
        workspace: &Path,
        arguments: &[OsString],
    ) -> Result<String, RepositoryInstallError> {
        self.capture_at(Some(workspace), arguments)
            .await
            .map(|output| output.stdout)
    }

    async fn capture_at(
        &self,
        workspace: Option<&Path>,
        arguments: &[OsString],
    ) -> Result<GitCommandFailure, RepositoryInstallError> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RepositoryInstallError::Deadline);
        }
        let mut command = self.command(workspace, arguments);
        capture(&mut command, remaining)
            .await
            .and_then(GitCommandFailure::require_success)
            .map_err(|error| RepositoryInstallError::Git(Box::new(error)))
    }

    fn command(&self, workspace: Option<&Path>, arguments: &[OsString]) -> Command {
        let mut command = local_git_command(self.program, workspace.unwrap_or(Path::new("/")));
        // Hosted identities cannot inherit a launcher cwd they cannot traverse.
        command.current_dir("/");
        if let Some(token) = self.token {
            // This environment exists only for trusted checkout and the shared redactor.
            command.env("GH_TOKEN", token).arg("-c").arg(format!(
                "http.https://github.com/.extraheader=AUTHORIZATION: basic {}",
                encode_basic_credential(token)
            ));
        }
        command.args(arguments);
        configure_identity(&mut command, self.uid, self.gid);
        command
    }
}

#[cfg(target_os = "linux")]
fn configure_identity(command: &mut Command, uid: u32, gid: u32) {
    use std::os::unix::process::CommandExt as _;

    command.as_std_mut().uid(uid).gid(gid);
}

#[cfg(not(target_os = "linux"))]
fn configure_identity(_command: &mut Command, _uid: u32, _gid: u32) {}

#[derive(Debug, thiserror::Error)]
pub(super) enum RepositoryInstallError {
    #[error("Git checkout command failed: {0}")]
    Git(Box<GitCommandFailure>),
    #[error("checkout workspace is unavailable: {0}")]
    Workspace(&'static str),
    #[error("checkout workspace could not be prepared: {0}")]
    Filesystem(#[from] std::io::Error),
    #[error("checkout revision mismatch: expected {expected}, observed {observed}")]
    RevisionMismatch { expected: String, observed: String },
    #[error("repository checkout exceeded its total deadline")]
    Deadline,
    #[error("{failure}\nCheckout recovery failed: {message}")]
    Recovery { failure: Box<Self>, message: String },
}

impl RepositoryInstallError {
    fn permits_retry(&self, attempt: u32, remaining: Duration, delay: Duration) -> bool {
        attempt < MAX_CHECKOUT_ATTEMPTS && matches!(self, Self::Git(_)) && remaining > delay
    }

    fn git_failure(&self) -> Option<&GitCommandFailure> {
        match self {
            Self::Git(error) => Some(error),
            Self::Recovery { failure, .. } => failure.git_failure(),
            _ => None,
        }
    }

    fn operator_stderr(&self) -> String {
        match self {
            Self::Git(error) => error.operator_stderr(),
            Self::Recovery { failure, message } => {
                format!(
                    "Checkout recovery failed: {message}\n{}",
                    failure.operator_stderr()
                )
            }
            _ => self.to_string(),
        }
    }

    pub(super) fn record_diagnostic(&self, run_id: &RunId, store: &OperatorDiagnosticStore) {
        let git = self.git_failure();
        store.record(NewOperatorDiagnostic {
            run_id: run_id.clone(),
            code: "git_checkout_failed",
            operation: "source.checkout",
            exit_status: git.and_then(|error| error.exit_status),
            stdout: git.map_or_else(String::new, |error| error.stdout.clone()),
            stderr: self.operator_stderr(),
            stdout_truncated: git.is_some_and(|error| error.stdout_truncated),
            stderr_truncated: git.is_some_and(|error| error.stderr_truncated),
        });
    }
}

#[cfg(test)]
pub(super) fn path_source(path: &Path) -> OsString {
    path.as_os_str().to_owned()
}
