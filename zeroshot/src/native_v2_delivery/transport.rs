//! One supervised Git transport for source admission, checkout, and exact-SHA publication.
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
#[cfg(test)]
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

use crate::execution::process::HostedProcessIdentity;
use crate::native_v2_target_authority::{NewOperatorDiagnostic, OperatorDiagnosticStore};
use crate::source_code_provider::{SourceDeliveryCredential, SourceDeliveryError, valid_git_revision};
use super::command::{
    GitCommandFailure, GitCommandOutput, capture_sensitive, local_git_command_with_identity,
};

mod authentication;
mod materialization;
mod reconcile;
pub(crate) use materialization::GitMaterializationTarget;
pub(crate) use reconcile::reconcile as reconcile_head;
use authentication::GitAuthentication;

/// Only trusted Git subprocesses receive this captured native context. It never enters a source
/// record, checkpoint, ledger, or agent environment.
#[derive(Clone)]
pub struct GitTransportConfig {
    pub git_program: PathBuf,
    pub workspace: PathBuf,
    pub home_directory: PathBuf,
    pub private_directory: PathBuf,
    pub identity: Option<HostedProcessIdentity>,
    pub local_environment: Option<BTreeMap<String, String>>,
    pub deadline: Duration,
}

impl std::fmt::Debug for GitTransportConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GitTransportConfig")
            .field("git_program", &self.git_program)
            .field("workspace", &self.workspace)
            .field("home_directory", &self.home_directory)
            .field("identity", &self.identity)
            .field(
                "local_environment",
                &self.local_environment.as_ref().map(|e| e.keys()),
            )
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct GitTransport {
    config: Arc<GitTransportConfig>,
    remote: OsString,
    checkout_references: Vec<String>,
    diagnostics: Option<(
        openengine_cluster_protocol::RunId,
        Arc<OperatorDiagnosticStore>,
    )>,
}

struct CheckoutRequest<'a> {
    branch: &'a str,
    revision: &'a str,
}

impl GitTransport {
    #[must_use]
    pub fn new(config: GitTransportConfig, remote: impl AsRef<OsStr>) -> Self {
        Self {
            config: Arc::new(config),
            remote: remote.as_ref().to_owned(),
            checkout_references: Vec::new(),
            diagnostics: None,
        }
    }

    #[must_use]
    pub(crate) fn with_operator_diagnostics(
        mut self,
        run_id: openengine_cluster_protocol::RunId,
        store: Arc<OperatorDiagnosticStore>,
    ) -> Self {
        self.diagnostics = Some((run_id, store));
        self
    }

    pub(crate) fn with_checkout_references(
        mut self,
        references: Vec<String>,
    ) -> Result<Self, SourceDeliveryError> {
        if references.len() > 2
            || references
                .iter()
                .any(|reference| !valid_checkout_reference(reference))
        {
            return Err(SourceDeliveryError::Rejected);
        }
        self.checkout_references = references;
        Ok(self)
    }

    #[must_use]
    pub fn config(&self) -> &GitTransportConfig {
        &self.config
    }

    #[must_use]
    pub fn remote(&self) -> &OsStr {
        &self.remote
    }

    pub async fn read_ref(
        &self,
        branch: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Option<String>, SourceDeliveryError> {
        if !crate::source_code_provider::valid_git_branch(branch) {
            return Err(SourceDeliveryError::Rejected);
        }
        let reference = format!("refs/heads/{branch}");
        let output = self
            .execute(
                &[
                    "ls-remote".into(),
                    "--refs".into(),
                    "--".into(),
                    self.remote.clone(),
                    reference.clone().into(),
                ],
                credential,
                self.config.deadline,
            )
            .await?
            .require_success()?;
        if output.stdout_truncated() || output.stdout().len() > 4096 {
            return Err(SourceDeliveryError::identity(
                "Git remote ref observation exceeded its bound",
            ));
        }
        parse_ref(output.stdout(), &reference)
    }

    pub async fn confirm_ref(
        &self,
        branch: &str,
        revision: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        match self.read_ref(branch, credential).await? {
            Some(observed) if observed == revision => Ok(()),
            observed => Err(SourceDeliveryError::identity(format!(
                "Git remote branch {branch} expected {revision}; observed {}",
                observed.as_deref().unwrap_or("absent")
            ))),
        }
    }

    pub async fn fetch_exact(
        &self,
        revision: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        if !valid_git_revision(revision) {
            return Err(SourceDeliveryError::Rejected);
        }
        self.success(
            &[
                "fetch",
                "--no-tags",
                "--quiet",
                "--no-write-fetch-head",
                "--",
                self.remote.to_str().ok_or(SourceDeliveryError::Rejected)?,
                revision,
            ],
            credential,
        )
        .await?;
        self.success(
            &["cat-file", "-e", &format!("{revision}^{{commit}}")],
            credential,
        )
        .await?;
        Ok(())
    }

    /// Initializes only an engine-owned fresh staging directory, then selects the admitted commit.
    pub async fn checkout(
        &self,
        branch: &str,
        revision: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        self.checkout_before(
            CheckoutRequest { branch, revision },
            credential,
            tokio::time::Instant::now() + self.config.deadline,
        )
        .await
    }

    async fn checkout_before(
        &self,
        source: CheckoutRequest<'_>,
        credential: SourceDeliveryCredential<'_>,
        deadline: tokio::time::Instant,
    ) -> Result<(), SourceDeliveryError> {
        let CheckoutRequest { branch, revision } = source;
        require_identity(branch, revision)?;
        self.initialize_checkout(revision, credential, deadline)
            .await?;
        let branch_ref = format!("refs/heads/{branch}");
        let mut references = vec!["fetch", "--no-tags", "--", "origin", &branch_ref, revision];
        references.extend(self.checkout_references.iter().map(String::as_str));
        self.success_before(&references, credential, deadline)
            .await?;
        self.retain_review_base(&self.checkout_references, credential, deadline)
            .await?;
        self.success_before(&["checkout", "--detach", revision], credential, deadline)
            .await?;
        let head = self
            .success_before(&["rev-parse", "HEAD"], credential, deadline)
            .await?;
        if head.stdout_truncated() || head.stdout().trim() != revision {
            return Err(SourceDeliveryError::identity(format!(
                "checkout revision mismatch: expected {revision}, observed {}",
                head.diagnostic().stdout.trim()
            )));
        }
        Ok(())
    }

    async fn initialize_checkout(
        &self,
        revision: &str,
        credential: SourceDeliveryCredential<'_>,
        deadline: tokio::time::Instant,
    ) -> Result<(), SourceDeliveryError> {
        self.success_before(
            &["init", "--quiet", object_format(revision)],
            credential,
            deadline,
        )
        .await?;
        self.success_before(
            &[
                "remote",
                "add",
                "origin",
                self.remote.to_str().ok_or(SourceDeliveryError::Rejected)?,
            ],
            credential,
            deadline,
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn fetch_review_references(
        &self,
        references: &[String],
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        if references.len() > 2
            || references
                .iter()
                .any(|reference| !valid_checkout_reference(reference))
        {
            return Err(SourceDeliveryError::Rejected);
        }
        let deadline = tokio::time::Instant::now() + self.config.deadline;
        let remote = self.remote.to_str().ok_or(SourceDeliveryError::Rejected)?;
        let mut args = vec!["fetch", "--no-tags", "--", remote];
        args.extend(references.iter().map(String::as_str));
        self.success_before(&args, credential, deadline).await?;
        self.retain_review_base(references, credential, deadline)
            .await
    }

    async fn retain_review_base(
        &self,
        references: &[String],
        credential: SourceDeliveryCredential<'_>,
        deadline: tokio::time::Instant,
    ) -> Result<(), SourceDeliveryError> {
        if let Some(revision) = references
            .iter()
            .find(|reference| super::valid_revision(reference))
        {
            self.success_before(
                &["update-ref", "refs/zeroshot/review/base", revision],
                credential,
                deadline,
            )
            .await?;
        }
        Ok(())
    }

    /// Publishes the captured candidate, then confirms its exact ref through Git. A lost response
    /// is reconciled before any caller can retry the effect or route a repair.
    pub async fn push(
        &self,
        branch: &str,
        revision: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        require_identity(branch, revision)?;
        let result = self
            .execute(
                &[
                    "push".into(),
                    "--porcelain".into(),
                    "--no-verify".into(),
                    "--".into(),
                    self.remote.clone(),
                    format!("{revision}:refs/heads/{branch}").into(),
                ],
                credential,
                self.config.deadline,
            )
            .await
            .and_then(GitCommandOutput::require_success);
        match result {
            Ok(_) => self.confirm_ref(branch, revision, credential).await,
            Err(failure) => {
                self.record_push_failure(&failure);
                if failure.cleanup_unconfirmed {
                    return Err(failure.into());
                }
                match self.confirm_ref(branch, revision, credential).await {
                    Ok(()) => Ok(()),
                    Err(error) if error.authentication_failed() || error.retryable_operation() => {
                        Err(error.with_context(failure))
                    }
                    Err(error) => Err(failure
                        .with_context(format!("push confirmation failed: {error}"))
                        .into()),
                }
            }
        }
    }

    pub(crate) async fn success(
        &self,
        arguments: &[&str],
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<GitCommandOutput, SourceDeliveryError> {
        let args = arguments.iter().map(OsString::from).collect::<Vec<_>>();
        self.execute(&args, credential, self.config.deadline)
            .await?
            .require_success()
            .map_err(Into::into)
    }

    async fn success_before(
        &self,
        arguments: &[&str],
        credential: SourceDeliveryCredential<'_>,
        deadline: tokio::time::Instant,
    ) -> Result<GitCommandOutput, SourceDeliveryError> {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(SourceDeliveryError::api(
                None,
                "repository checkout exceeded its total deadline",
            ));
        }
        let args = arguments.iter().map(OsString::from).collect::<Vec<_>>();
        self.execute(&args, credential, remaining)
            .await?
            .require_success()
            .map_err(Into::into)
    }

    pub(crate) async fn execute(
        &self,
        arguments: &[OsString],
        credential: SourceDeliveryCredential<'_>,
        deadline: Duration,
    ) -> Result<GitCommandOutput, GitCommandFailure> {
        let mut command = self.command();
        let authentication = GitAuthentication::prepare(&self.config, &self.remote, credential)
            .map_err(|error| super::command::preparation_failure(&command, error))?;
        if arguments.first().is_some_and(|operation| {
            matches!(operation.to_str(), Some("ls-remote" | "fetch" | "push"))
        }) {
            authentication.configure(&mut command);
        }
        command.args(arguments);
        capture_sensitive(&mut command, deadline, authentication.secrets()).await
    }

    pub(crate) fn workspace_git(&self) -> super::git::SystemGit {
        super::git::SystemGit::new(self.config.git_program.clone())
            .with_identity(self.config.identity)
    }

    fn command(&self) -> Command {
        let mut command = local_git_command_with_identity(
            &self.config.git_program,
            &self.config.workspace,
            self.config.identity,
        );
        if let Some(environment) = &self.config.local_environment {
            command.envs(environment);
            if self.config.identity.is_none() {
                // Resolve native relative PATH entries in the selected workspace.
                command.current_dir(&self.config.workspace);
            }
            for variable in ["GIT_CONFIG_NOSYSTEM", "GIT_CONFIG_GLOBAL"] {
                if !environment.contains_key(variable) {
                    command.env_remove(variable);
                }
            }
        } else {
            command
                .env("HOME", &self.config.home_directory)
                .env("PATH", "/usr/bin:/bin");
        }
        remove_repository_environment(command.as_std_mut());
        command
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            .env("SSH_ASKPASS_REQUIRE", "never");
        command
    }

    fn record_push_failure(&self, failure: &GitCommandFailure) {
        let Some((run_id, store)) = &self.diagnostics else {
            return;
        };
        store.record(NewOperatorDiagnostic {
            run_id: run_id.clone(),
            code: "git_push_failed",
            operation: "delivery.git_push",
            exit_status: failure.exit_status,
            stdout: failure.stdout.clone(),
            stderr: failure.stderr.clone(),
            stdout_truncated: failure.stdout_truncated,
            stderr_truncated: failure.stderr_truncated,
        });
    }
}

#[cfg(test)]
#[path = "transport/tests.rs"]
mod tests;

fn require_identity(branch: &str, revision: &str) -> Result<(), SourceDeliveryError> {
    if crate::source_code_provider::valid_git_branch(branch) && valid_git_revision(revision) {
        Ok(())
    } else {
        Err(SourceDeliveryError::Rejected)
    }
}

fn parse_ref(output: &str, reference: &str) -> Result<Option<String>, SourceDeliveryError> {
    let mut lines = output.lines();
    let Some(line) = lines.next() else {
        return Ok(None);
    };
    let Some((revision, name)) = line.split_once('\t') else {
        return Err(SourceDeliveryError::identity(
            "Git returned a malformed remote ref",
        ));
    };
    if name != reference || !valid_git_revision(revision) || lines.next().is_some() {
        return Err(SourceDeliveryError::identity(
            "Git remote ref did not match the requested identity",
        ));
    }
    Ok(Some(revision.to_owned()))
}

#[cfg(test)]
#[path = "transport/chaos_tests.rs"]
mod chaos_tests;

pub fn source_credential<'a>(
    token: &'a str,
    credentials: Option<&'a openengine_cluster_protocol::GitTransportCredentials>,
) -> SourceDeliveryCredential<'a> {
    use openengine_cluster_protocol::GitTransportCredentials;
    use crate::source_code_provider::SourceGitCredential;
    let credential = SourceDeliveryCredential::token(token).with_github_git_token(token);
    match credentials {
        Some(GitTransportCredentials::Https { username, password }) => {
            credential.with_git(SourceGitCredential::Https { username, password })
        }
        Some(GitTransportCredentials::Ssh {
            private_key,
            known_hosts,
        }) => credential.with_git(SourceGitCredential::Ssh {
            private_key,
            known_hosts,
        }),
        None => credential,
    }
}

pub(crate) const fn object_format(revision: &str) -> &'static str {
    if revision.len() == 64 {
        "--object-format=sha256"
    } else {
        "--object-format=sha1"
    }
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

fn valid_checkout_reference(value: &str) -> bool {
    if super::valid_revision(value) {
        return true;
    }
    value
        .strip_prefix("refs/pull/")
        .and_then(|value| value.strip_suffix("/head"))
        .is_some_and(|number| {
            !number.starts_with('0')
                && !number.is_empty()
                && number.len() <= 20
                && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}
