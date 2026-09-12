use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;

use super::{command, valid_revision, GitCommandFailure};

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(super) enum GitError {
    #[error("workspace has no deliverable mutation relative to the requested base revision")]
    NoMutation,
    #[error("Git returned an invalid HEAD revision")]
    InvalidRevision,
    #[error("{0}")]
    Command(Box<GitCommandFailure>),
}

impl From<GitCommandFailure> for GitError {
    fn from(failure: GitCommandFailure) -> Self {
        Self::Command(Box::new(failure))
    }
}

#[derive(Clone)]
pub(super) struct SystemGit {
    program: PathBuf,
}

impl SystemGit {
    pub(super) fn new(program: PathBuf) -> Self {
        Self { program }
    }

    pub(super) async fn prepare_revision(
        &self,
        workspace: &Path,
        base_revision: &str,
        commit_message: &str,
    ) -> Result<String, GitError> {
        self.require_success(workspace, &["add", "--all"]).await?;
        let staged = self
            .execute(workspace, &["diff", "--cached", "--quiet", "--exit-code"])
            .await?;
        match staged.exit_status {
            Some(0) => {}
            Some(1) => {
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
            _ => return Err(staged.into()),
        }
        self.deliverable_revision(workspace, base_revision).await
    }

    async fn deliverable_revision(
        &self,
        workspace: &Path,
        base_revision: &str,
    ) -> Result<String, GitError> {
        let output = self
            .require_success(workspace, &["rev-parse", "HEAD"])
            .await?;
        let revision = output.stdout.trim().to_owned();
        if !valid_revision(&revision) {
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
    ) -> Result<GitCommandFailure, GitError> {
        self.execute(workspace, arguments)
            .await?
            .require_success()
            .map_err(Into::into)
    }

    async fn execute(
        &self,
        workspace: &Path,
        arguments: &[&str],
    ) -> Result<GitCommandFailure, GitError> {
        command::capture(
            &mut self.command(workspace, arguments),
            Duration::from_secs(10 * 60),
        )
        .await
        .map_err(Into::into)
    }

    fn command(&self, workspace: &Path, arguments: &[&str]) -> Command {
        let mut command = command::local_git_command(&self.program, workspace);
        command.args(arguments);
        command
    }
}

#[cfg(test)]
mod tests {
    use openengine_cluster_testkit::assertions::AssertValue;

    use super::*;
    use crate::native_v2_candidate::test_support::{TestGitRepository, git_output};

    #[tokio::test]
    async fn uses_manifest_title_as_the_commit_message() {
        let repository = TestGitRepository::delivery();
        let git = SystemGit::new(PathBuf::from("/usr/bin/git"));

        let revision = git
            .prepare_revision(
                &repository.workspace,
                &repository.base,
                "fix: repair checkout",
            )
            .await
            .assert_value();

        assert_eq!(
            git_output(
                &repository.workspace,
                &["show", "--no-patch", "--format=%s", &revision]
            ),
            "fix: repair checkout"
        );
    }
}
