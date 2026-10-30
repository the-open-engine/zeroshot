use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;

use super::{command, valid_revision, GitCommandFailure};

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum GitError {
    #[error("workspace has no deliverable mutation relative to the requested base revision")]
    NoMutation,
    #[error("Git returned an invalid HEAD revision")]
    InvalidRevision,
    #[error(
        "workspace has unresolved merge entries; resolve and stage these paths before delivery\n{0}"
    )]
    Unmerged(Box<GitCommandFailure>),
    #[error("{0}")]
    Command(Box<GitCommandFailure>),
}

impl From<GitCommandFailure> for GitError {
    fn from(failure: GitCommandFailure) -> Self {
        Self::Command(Box::new(failure))
    }
}

#[derive(Clone)]
pub(crate) struct SystemGit {
    program: PathBuf,
    identity: Option<crate::execution::process::HostedProcessIdentity>,
    local_environment: Option<std::collections::BTreeMap<String, String>>,
}

impl SystemGit {
    pub(crate) fn new(program: PathBuf) -> Self {
        Self {
            program,
            identity: None,
            local_environment: None,
        }
    }

    pub(crate) fn with_identity(
        mut self,
        identity: Option<crate::execution::process::HostedProcessIdentity>,
    ) -> Self {
        self.identity = identity;
        self
    }

    pub(crate) fn with_local_environment(
        mut self,
        environment: Option<std::collections::BTreeMap<String, String>>,
    ) -> Self {
        self.local_environment = environment;
        self
    }

    pub(crate) async fn prepare_revision(
        &self,
        workspace: &Path,
        base_revision: &str,
        commit_message: &str,
    ) -> Result<String, GitError> {
        self.require_supported_operation(workspace).await?;
        let unresolved = self
            .require_success(workspace, &["diff", "--name-only", "--diff-filter=U"])
            .await?;
        if !unresolved.stdout().is_empty() || unresolved.stdout_truncated() {
            return Err(GitError::Unmerged(Box::new(unresolved.into_diagnostic())));
        }
        self.require_success(workspace, &["add", "--all"]).await?;
        if self.commit_pending(workspace).await? {
            self.require_success(
                workspace,
                &[
                    "-c",
                    "user.name=Zeroshot",
                    "-c",
                    "user.email=delivery@zeroshot.invalid",
                    "commit",
                    "--no-verify",
                    "--message",
                    commit_message,
                ],
            )
            .await?;
        }
        self.deliverable_revision(workspace, base_revision).await
    }

    async fn require_supported_operation(&self, workspace: &Path) -> Result<(), GitError> {
        let directory = self
            .require_success(workspace, &["rev-parse", "--absolute-git-dir"])
            .await?;
        if directory.stdout_truncated() || directory.stdout().is_empty() {
            return Err(directory
                .into_diagnostic()
                .with_context("Git directory path is unavailable")
                .into());
        }
        let path = Path::new(
            directory
                .stdout()
                .strip_suffix('\n')
                .unwrap_or(directory.stdout()),
        );
        for marker in [
            "rebase-merge",
            "rebase-apply",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "sequencer",
        ] {
            let pending = path.join(marker).try_exists().map_err(|error| {
                directory
                    .diagnostic()
                    .clone()
                    .with_context(format!("cannot inspect Git state {marker}: {error}"))
            })?;
            if pending {
                let status = self
                    .require_success(
                        workspace,
                        &["--no-optional-locks", "status", "--untracked-files=all"],
                    )
                    .await?;
                return Err(status.into_diagnostic().with_context(format!(
                    "workspace has an unfinished Git operation ({marker}); finish or abort it before delivery; \
                     no files were staged or committed"
                )).into());
            }
        }
        Ok(())
    }

    pub(crate) async fn workspace_state(
        &self,
        workspace: &Path,
    ) -> Result<(String, bool), GitError> {
        self.require_supported_operation(workspace).await?;
        let head = self
            .require_success(workspace, &["rev-parse", "HEAD"])
            .await?;
        let revision = head.stdout().trim().to_owned();
        if !valid_revision(&revision) || head.stdout_truncated() {
            return Err(GitError::InvalidRevision);
        }
        let status = self
            .require_success(
                workspace,
                &["status", "--porcelain=v1", "--untracked-files=all"],
            )
            .await?;
        Ok((
            revision,
            !status.stdout().is_empty()
                || status.stdout_truncated()
                || self.merge_pending(workspace).await?,
        ))
    }

    pub(crate) async fn revision_tree(
        &self,
        workspace: &Path,
        revision: &str,
    ) -> Result<String, GitError> {
        let tree = self
            .require_success(workspace, &["rev-parse", &format!("{revision}^{{tree}}")])
            .await?;
        let value = tree.stdout().trim().to_owned();
        if tree.stdout_truncated() || !valid_revision(&value) {
            return Err(GitError::InvalidRevision);
        }
        Ok(value)
    }

    async fn commit_pending(&self, workspace: &Path) -> Result<bool, GitError> {
        let staged = self
            .execute(workspace, &["diff", "--cached", "--quiet", "--exit-code"])
            .await?;
        match staged.diagnostic().exit_status {
            Some(0) => self.merge_pending(workspace).await,
            Some(1) => Ok(true),
            _ => Err(staged.into_diagnostic().into()),
        }
    }

    async fn merge_pending(&self, workspace: &Path) -> Result<bool, GitError> {
        let merge = self
            .execute(workspace, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])
            .await?;
        match merge.diagnostic().exit_status {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(merge.into_diagnostic().into()),
        }
    }

    pub(crate) async fn deliverable_revision(
        &self,
        workspace: &Path,
        base_revision: &str,
    ) -> Result<String, GitError> {
        let output = self
            .require_success(workspace, &["rev-parse", "HEAD"])
            .await?;
        let revision = output.stdout().trim().to_owned();
        if output.stdout_truncated() || !valid_revision(&revision) {
            return Err(GitError::InvalidRevision);
        }
        if revision == base_revision {
            return Err(GitError::NoMutation);
        }
        self.require_success(
            workspace,
            &["merge-base", "--is-ancestor", base_revision, &revision],
        )
        .await?;
        Ok(revision)
    }

    async fn require_success(
        &self,
        workspace: &Path,
        arguments: &[&str],
    ) -> Result<command::GitCommandOutput, GitError> {
        self.execute(workspace, arguments)
            .await?
            .require_success()
            .map_err(Into::into)
    }

    async fn execute(
        &self,
        workspace: &Path,
        arguments: &[&str],
    ) -> Result<command::GitCommandOutput, GitError> {
        command::capture_sensitive(
            &mut self.command(workspace, arguments),
            Duration::from_secs(10 * 60),
            &[],
        )
        .await
        .map_err(Into::into)
    }

    fn command(&self, workspace: &Path, arguments: &[&str]) -> Command {
        let mut command = command::git_command(
            &self.program,
            workspace,
            self.identity,
            self.local_environment.as_ref(),
        );
        command.args(arguments);
        command
    }
}

#[cfg(test)]
#[path = "git/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "git/tests/operations.rs"]
mod operations;

#[cfg(test)]
#[path = "git/test_support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "git/tests/environment.rs"]
mod environment_tests;
