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
    commit_identity: Option<openengine_cluster_protocol::CommitIdentity>,
    commit_co_author: Option<openengine_cluster_protocol::CommitIdentity>,
}

impl SystemGit {
    pub(crate) async fn attached_branch(&self, workspace: &Path) -> Result<String, GitError> {
        let output = self
            .require_success(workspace, &["symbolic-ref", "--quiet", "HEAD"])
            .await?;
        let branch = output
            .stdout()
            .trim()
            .strip_prefix("refs/heads/")
            .ok_or(GitError::InvalidRevision)?;
        if output.stdout_truncated() || !crate::source_code_provider::valid_git_branch(branch) {
            return Err(GitError::InvalidRevision);
        }
        Ok(branch.to_owned())
    }

    pub(crate) fn new(program: PathBuf) -> Self {
        Self {
            program,
            identity: None,
            local_environment: None,
            commit_identity: None,
            commit_co_author: None,
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

    pub(super) fn diagnostic_secrets(&self) -> Vec<String> {
        self.local_environment
            .iter()
            .flat_map(|environment| environment.iter())
            .filter(|(name, value)| command::native_credential_name(name) && !value.is_empty())
            .flat_map(|(_, value)| {
                let mut secrets =
                    crate::source_code_provider::adapters::core::network::proxy_secrets(value);
                secrets.push(super::git_auth::encode_basic_credential(value));
                secrets
            })
            .collect()
    }

    pub(crate) fn with_commit_identity(
        mut self,
        identity: Option<openengine_cluster_protocol::CommitIdentity>,
    ) -> Self {
        self.commit_identity = identity;
        self
    }

    pub(crate) fn with_commit_co_author(
        mut self,
        co_author: Option<openengine_cluster_protocol::CommitIdentity>,
    ) -> Self {
        self.commit_co_author = co_author;
        self
    }

    pub(crate) async fn prepare_revision(
        &self,
        workspace: &Path,
        base_revision: &str,
        commit_message: &str,
    ) -> Result<String, GitError> {
        self.stage_workspace(workspace).await?;
        if self.commit_pending(workspace).await? {
            if let Some(co_author) = &self.commit_co_author {
                self.commit_crediting(workspace, commit_message, co_author)
                    .await?;
            } else {
                let [name, email] = command::identity_configuration(self.commit_identity.as_ref());
                self.require_success(
                    workspace,
                    &[
                        "-c",
                        &name,
                        "-c",
                        &email,
                        "commit",
                        "--no-verify",
                        "--message",
                        commit_message,
                    ],
                )
                .await?;
            }
        }
        self.deliverable_revision(workspace, base_revision).await
    }

    /// Commits with the co-author credit in one Git command, so a failed or interrupted commit
    /// never leaves the run's changes committed without it. The message gets Git's default
    /// whitespace cleanup whatever the workspace configures. One that cleanup empties stays
    /// uncredited, so Git refuses it rather than committing the credit as the whole message.
    /// Git's trailer handling appends the credit with pinned placement, duplicate handling and
    /// separator, so workspace trailer defaults cannot move, repeat or drop it. No pin sets a
    /// `key`, which would make Git rename existing trailers whose names prefix it.
    async fn commit_crediting(
        &self,
        workspace: &Path,
        commit_message: &str,
        co_author: &openengine_cluster_protocol::CommitIdentity,
    ) -> Result<(), GitError> {
        let [name, email] = command::identity_configuration(self.commit_identity.as_ref());
        let trailer = format!(
            "Co-authored-by: {} <{}>",
            co_author.name(),
            co_author.email()
        );
        let mut arguments = vec![
            "-c",
            name.as_str(),
            "-c",
            email.as_str(),
            "-c",
            "trailer.separators=:",
            "-c",
            "trailer.Co-authored-by.where=end",
            "-c",
            "trailer.Co-authored-by.ifexists=addIfDifferent",
            "-c",
            "trailer.Co-authored-by.ifmissing=add",
            "commit",
            "--no-verify",
            "--cleanup=whitespace",
            "--message",
            commit_message,
        ];
        if !empty_after_whitespace_cleanup(commit_message) {
            arguments.extend(["--trailer", trailer.as_str()]);
        }
        self.require_success(workspace, &arguments).await?;
        Ok(())
    }

    pub(crate) async fn stage_workspace(&self, workspace: &Path) -> Result<String, GitError> {
        self.require_supported_operation(workspace).await?;
        let unresolved = self
            .require_success(workspace, &["diff", "--name-only", "--diff-filter=U"])
            .await?;
        if !unresolved.stdout().is_empty() || unresolved.stdout_truncated() {
            return Err(GitError::Unmerged(Box::new(unresolved.into_diagnostic())));
        }
        Ok(self
            .require_success(workspace, &["add", "--all"])
            .await?
            .diagnostic()
            .stderr
            .clone())
    }

    pub(crate) async fn uncommitted_changes(
        &self,
        workspace: &Path,
    ) -> Result<Option<GitCommandFailure>, GitError> {
        self.require_supported_operation(workspace).await?;
        let status = self
            .require_success(
                workspace,
                &[
                    "--no-optional-locks",
                    "status",
                    "--porcelain=v1",
                    "--untracked-files=all",
                    "--ignore-submodules=dirty",
                ],
            )
            .await?;
        let pending_merge = self.merge_pending(workspace).await?;
        if status.stdout().is_empty() && !status.stdout_truncated() && !pending_merge {
            return Ok(None);
        }
        let diagnostic = status.into_diagnostic();
        Ok(Some(if pending_merge {
            diagnostic.with_context("workspace has a pending MERGE_HEAD; finish or abort the merge before starting a delivery run")
        } else {
            diagnostic
        }))
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
                &[
                    "status",
                    "--porcelain=v1",
                    "--untracked-files=all",
                    "--ignore-submodules=dirty",
                ],
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

    pub(crate) async fn merge_pending(&self, workspace: &Path) -> Result<bool, GitError> {
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
        let revision = self.descendant_revision(workspace, base_revision).await?;
        if revision == base_revision {
            return Err(GitError::NoMutation);
        }
        Ok(revision)
    }

    /// Confirms source ancestry, including an already committed local starting revision.
    pub(crate) async fn descendant_revision(
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
            command::GitCommandContext {
                identity: self.identity,
                local_environment: self.local_environment.as_ref(),
                commit_identity: self.commit_identity.as_ref(),
            },
        );
        command.args(arguments);
        command
    }
}

/// Whether Git's whitespace cleanup leaves only blank and `Signed-off-by: ` lines, which Git
/// refuses as an empty commit message.
fn empty_after_whitespace_cleanup(message: &str) -> bool {
    message.split('\n').all(|line| {
        let line = line.trim_end_matches([' ', '\t', '\r']);
        line.is_empty() || line.starts_with("Signed-off-by: ")
    })
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
