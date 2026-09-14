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
        let unresolved = self
            .require_success(workspace, &["diff", "--name-only", "--diff-filter=U"])
            .await?;
        if !unresolved.stdout.is_empty() || unresolved.stdout_truncated {
            return Err(GitError::Unmerged(Box::new(unresolved)));
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

    pub(super) async fn workspace_state(
        &self,
        workspace: &Path,
    ) -> Result<(String, bool), GitError> {
        let head = self
            .require_success(workspace, &["rev-parse", "HEAD"])
            .await?;
        let revision = head.stdout.trim().to_owned();
        if !valid_revision(&revision) || head.stdout_truncated {
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
            !status.stdout.is_empty()
                || status.stdout_truncated
                || self.merge_pending(workspace).await?,
        ))
    }

    async fn commit_pending(&self, workspace: &Path) -> Result<bool, GitError> {
        let staged = self
            .execute(workspace, &["diff", "--cached", "--quiet", "--exit-code"])
            .await?;
        match staged.exit_status {
            Some(0) => self.merge_pending(workspace).await,
            Some(1) => Ok(true),
            _ => Err(staged.into()),
        }
    }

    async fn merge_pending(&self, workspace: &Path) -> Result<bool, GitError> {
        let merge = self
            .execute(workspace, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])
            .await?;
        match merge.exit_status {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(merge.into()),
        }
    }

    pub(super) async fn deliverable_revision(
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
    #[tokio::test]
    async fn unresolved_index_is_preserved_until_the_caller_resolves_it() {
        for resolution in ["combined change\n", "local change\n"] {
            assert_conflict_resolution(resolution).await;
        }
    }

    async fn assert_conflict_resolution(resolution: &str) {
        use crate::native_v2_candidate::test_support::git;
        let repository = TestGitRepository::candidate();
        let workspace = &repository.workspace;
        git(workspace, &["config", "user.name", "Test"]);
        git(workspace, &["config", "user.email", "test@example.invalid"]);
        git(workspace, &["checkout", "-b", "other"]);
        std::fs::write(workspace.join("README.md"), "remote change\n").assert_value();
        git(workspace, &["commit", "-am", "remote change"]);
        git(workspace, &["checkout", "main"]);
        std::fs::write(workspace.join("README.md"), "local change\n").assert_value();
        git(workspace, &["commit", "-am", "local change"]);
        let head = git_output(workspace, &["rev-parse", "HEAD"]);
        let merge = std::process::Command::new("git")
            .arg("-C")
            .arg(workspace)
            .args(["merge", "other"])
            .output()
            .assert_value();
        assert!(!merge.status.success());
        let index = git_output(workspace, &["ls-files", "--unmerged"]);
        assert!(!index.is_empty());
        let system = SystemGit::new(PathBuf::from("/usr/bin/git"));
        let error = system
            .prepare_revision(workspace, &repository.base, "delivery")
            .await
            .unwrap_err();
        assert!(matches!(&error, GitError::Unmerged(_)));
        assert!(error.to_string().contains("README.md"));
        assert_eq!(git_output(workspace, &["ls-files", "--unmerged"]), index);
        assert_eq!(git_output(workspace, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            system.workspace_state(workspace).await.assert_value(),
            (head, true)
        );
        std::fs::write(workspace.join("README.md"), resolution).assert_value();
        git(workspace, &["add", "README.md"]);
        assert!(system.workspace_state(workspace).await.assert_value().1);
        let delivered = system
            .prepare_revision(workspace, &repository.base, "resolved")
            .await
            .assert_value();
        assert_eq!(
            system.workspace_state(workspace).await.assert_value(),
            (delivered, false)
        );
        assert!(git_output(workspace, &["ls-files", "--unmerged"]).is_empty());
        assert_eq!(
            git_output(workspace, &["show", "HEAD:README.md"]),
            resolution.trim()
        );
        assert_eq!(
            git_output(workspace, &["rev-list", "--parents", "-n", "1", "HEAD"])
                .split_whitespace()
                .count(),
            3
        );
    }
}
