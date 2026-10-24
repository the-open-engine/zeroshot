use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::Duration;

use openengine_cluster_protocol::RunId;

use tokio::time::Instant;

use crate::execution::process::{HostedProcessPool, HostedProcessScope};
use crate::native_v2_delivery::{DeliveryTarget, DeliveryMode, SourceDeliveryError, source_target};
use crate::source_code_provider::SourceMaterializationDestination;
use crate::native_v2_delivery::command::GitCommandFailure;
#[cfg(all(test, unix))]
use crate::native_v2_delivery::command::GitCommandOutput;
use crate::native_v2_delivery::transport::{
    GitTransport, GitTransportConfig, GitMaterializationTarget, source_credential,
};
use crate::native_v2_portable_controller::WorkspaceIdentity;
use crate::native_v2_target_authority::{NewOperatorDiagnostic, OperatorDiagnosticStore};
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
    pub checkout_references: &'a [String],
    pub workspace: &'a Path,
    pub process_pool: HostedProcessPool,
    pub github_token: Option<&'a str>,
    pub git_credentials: Option<&'a openengine_cluster_protocol::GitTransportCredentials>,
    pub private_directory: &'a Path,
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
        identity: Some(writer),
        git_credentials: request.git_credentials,
        private_directory: request.private_directory,
        remote: request.source,
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
    let transport = git
        .transport(request.workspace)?
        .with_checkout_references(request.checkout_references.to_vec())
        .map_err(RepositoryInstallError::Source)?;
    let remote = crate::source_code_provider::adapters::core::source_remote(request.resolved);
    let target = GitMaterializationTarget::new(
        &transport,
        (
            &remote,
            request.resolved.branch.as_str(),
            request.resolved.revision.as_str(),
        ),
        source_credential(git.token.unwrap_or(""), git.git_credentials),
        git.deadline,
    );
    let result = crate::source_code_provider::adapters::core::materialize_source(
        request.resolved,
        SourceMaterializationDestination::new(&target),
    )
    .await;
    if result.is_err() {
        return Err(match target.take_failure() {
            Some(SourceDeliveryError::Command(failure)) => RepositoryInstallError::Git(failure),
            _ if Instant::now() >= git.deadline => RepositoryInstallError::Deadline,
            Some(error) => RepositoryInstallError::Source(error),
            None => RepositoryInstallError::Workspace("source provider materialization failed"),
        });
    }
    source_target(request.resolved, DeliveryMode::Push)
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

pub(super) fn production_source(source: &ResolvedSource) -> OsString {
    crate::source_code_provider::adapters::core::source_remote(source).into()
}

struct GitProcess<'a> {
    program: &'a Path,
    token: Option<&'a str>,
    identity: Option<crate::execution::process::HostedProcessIdentity>,
    git_credentials: Option<&'a openengine_cluster_protocol::GitTransportCredentials>,
    private_directory: &'a Path,
    remote: &'a OsStr,
    deadline: Instant,
}

impl GitProcess<'_> {
    fn transport(&self, workspace: &Path) -> Result<GitTransport, RepositoryInstallError> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RepositoryInstallError::Deadline);
        }
        // Portable checkout fixtures run under the test runner's UID. Production retains the
        // allocated writer identity; root tests continue to exercise their distinct worker UID.
        #[cfg(all(test, unix))]
        let identity = self
            .identity
            .filter(|writer| writer.uid() != unsafe { libc::geteuid() });
        #[cfg(not(all(test, unix)))]
        let identity = self.identity;
        Ok(GitTransport::new(
            GitTransportConfig {
                git_program: self.program.to_owned(),
                workspace: workspace.to_owned(),
                home_directory: self.private_directory.to_owned(),
                private_directory: self.private_directory.to_owned(),
                identity,
                local_environment: None,
                deadline: remaining,
            },
            self.remote,
        ))
    }

    #[cfg(all(test, unix))]
    async fn run(
        &self,
        workspace: Option<&Path>,
        arguments: &[OsString],
    ) -> Result<(), RepositoryInstallError> {
        self.capture_at(workspace, arguments).await.map(|_| ())
    }

    #[cfg(all(test, unix))]
    async fn capture(
        &self,
        workspace: &Path,
        arguments: &[OsString],
    ) -> Result<String, RepositoryInstallError> {
        self.capture_at(Some(workspace), arguments)
            .await
            .map(|output| output.stdout().to_owned())
    }

    #[cfg(all(test, unix))]
    async fn capture_at(
        &self,
        workspace: Option<&Path>,
        arguments: &[OsString],
    ) -> Result<GitCommandOutput, RepositoryInstallError> {
        let transport = self.transport(workspace.unwrap_or(Path::new("/")))?;
        transport
            .execute(
                arguments,
                source_credential(self.token.unwrap_or(""), self.git_credentials),
                transport.config().deadline,
            )
            .await
            .and_then(GitCommandOutput::require_success)
            .map_err(|error| RepositoryInstallError::Git(Box::new(error)))
    }
}

#[derive(Debug, thiserror::Error)]
pub(super) enum RepositoryInstallError {
    #[error("Git checkout command failed: {0}")]
    Git(Box<GitCommandFailure>),
    #[error("checkout workspace is unavailable: {0}")]
    Workspace(&'static str),
    #[error("checkout workspace could not be prepared: {0}")]
    Filesystem(#[from] std::io::Error),
    #[error(transparent)]
    Source(SourceDeliveryError),
    #[error("repository checkout exceeded its total deadline")]
    Deadline,
    #[error("{failure}\nCheckout recovery failed: {message}")]
    Recovery { failure: Box<Self>, message: String },
}

impl RepositoryInstallError {
    fn permits_retry(&self, attempt: u32, remaining: Duration, delay: Duration) -> bool {
        attempt < MAX_CHECKOUT_ATTEMPTS
            && matches!(self, Self::Git(failure) if !failure.cleanup_unconfirmed)
            && remaining > delay
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
