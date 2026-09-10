use std::path::{Path, PathBuf};
use std::process::Stdio;

use openengine_cluster_protocol::{
    ResolvedSource, SourceBranchId, SourceRepositoryId, SourceRevisionId,
};
use tokio::process::Command;

use crate::native_v2_cli::{NamedRunSource, NativeV2CliError, RunCommand};
use crate::native_v2_delivery::git_auth::encode_basic_credential;

const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10 * 60);
const MAX_OUTPUT: usize = 1024 * 1024;

struct WorktreeSourceContext {
    root: PathBuf,
    local_branch: Option<String>,
    upstream: Option<(String, SourceBranchId)>,
}

struct RevisionSelection<'a> {
    root: &'a Path,
    repository: &'a SourceRepositoryId,
    branch: &'a SourceBranchId,
    remote: Option<&'a str>,
    token: Option<&'a str>,
}

pub(super) async fn resolve(
    run: &RunCommand,
    token: Option<&str>,
) -> Result<Option<NamedRunSource>, NativeV2CliError> {
    if run.target.is_none() || run.validate_only {
        return Ok(None);
    }
    let worktree = inspect_worktree(run).await?;
    let (repository, remote) = select_run_repository(run, &worktree).await?;
    let branch = select_run_branch(run, &worktree)?;
    let revision = select_run_revision(
        run,
        RevisionSelection {
            root: &worktree.root,
            repository: &repository,
            branch: &branch,
            remote: remote.as_deref(),
            token,
        },
    )
    .await?;
    let dirty = worktree_is_dirty(&worktree.root).await?;
    Ok(Some(NamedRunSource {
        resolved: ResolvedSource {
            repository,
            branch,
            revision,
        },
        dirty,
    }))
}

async fn inspect_worktree(run: &RunCommand) -> Result<WorktreeSourceContext, NativeV2CliError> {
    let root = git(&["rev-parse", "--show-toplevel"]).await.map_err(|_| {
        source_error(
            "run from a Git worktree, or supply --repository and --branch from a Git worktree",
        )
    })?;
    let root = PathBuf::from(root.trim());
    let local_branch = git_at(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .await
        .ok()
        .map(|value| value.trim().to_owned());
    if local_branch.is_none() && (run.repository.is_none() || run.branch.is_none()) {
        return Err(source_error(
            "detached worktrees require both --repository OWNER/NAME and --branch BRANCH",
        ));
    }
    let upstream = if run.repository.is_none() || run.branch.is_none() {
        match &local_branch {
            Some(branch) => upstream(&root, branch).await?,
            None => None,
        }
    } else {
        None
    };
    Ok(WorktreeSourceContext {
        root,
        local_branch,
        upstream,
    })
}

async fn select_run_repository(
    run: &RunCommand,
    worktree: &WorktreeSourceContext,
) -> Result<(SourceRepositoryId, Option<String>), NativeV2CliError> {
    let upstream_remote = worktree.upstream.as_ref().map(|value| value.0.as_str());
    let (repository, remote) = match &run.repository {
        Some(repository) => (
            repository.clone(),
            matching_remote(&worktree.root, repository, upstream_remote).await,
        ),
        None => select_repository(&worktree.root, upstream_remote).await?,
    };
    Ok((repository, remote))
}

fn select_run_branch(
    run: &RunCommand,
    worktree: &WorktreeSourceContext,
) -> Result<SourceBranchId, NativeV2CliError> {
    run.branch
        .clone()
        .or_else(|| worktree.upstream.as_ref().map(|value| value.1.clone()))
        .or_else(|| {
            worktree
                .local_branch
                .as_ref()
                .and_then(|value| SourceBranchId::new(value.clone()).ok())
        })
        .ok_or_else(|| source_error("could not select a branch; supply --branch BRANCH"))
}

async fn select_run_revision(
    run: &RunCommand,
    selection: RevisionSelection<'_>,
) -> Result<SourceRevisionId, NativeV2CliError> {
    match &run.revision {
        Some(revision) => Ok(revision.clone()),
        None => remote_tip(selection).await,
    }
}

async fn upstream(
    root: &Path,
    branch: &str,
) -> Result<Option<(String, SourceBranchId)>, NativeV2CliError> {
    let reference = format!("refs/heads/{branch}");
    let value = git_at(
        root,
        &[
            "for-each-ref",
            "--count=1",
            "--format=%(upstream:remotename)%00%(upstream:lstrip=3)",
            &reference,
        ],
    )
    .await
    .map_err(|_| {
        source_error(
            "could not inspect the attached branch upstream; supply --repository and --branch",
        )
    })?;
    let Some((remote, branch)) = value.trim().split_once('\0') else {
        return Ok(None);
    };
    if remote.is_empty() || branch.is_empty() || remote == "." {
        return Ok(None);
    }
    Ok(Some((
        remote.to_owned(),
        SourceBranchId::new(branch)
            .map_err(|error| source_error(&format!("invalid upstream branch: {error}")))?,
    )))
}

async fn select_repository(
    root: &Path,
    upstream_remote: Option<&str>,
) -> Result<(SourceRepositoryId, Option<String>), NativeV2CliError> {
    if let Some(remote) = upstream_remote {
        if let Ok(repository) = repository_for_remote(root, remote).await {
            return Ok((repository, Some(remote.to_owned())));
        }
    }
    let remotes = git_at(root, &["remote"])
        .await
        .map_err(|_| source_error("could not list Git remotes; supply --repository OWNER/NAME"))?;
    let mut github = Vec::new();
    for remote in remotes.lines().filter(|value| !value.is_empty()) {
        if let Ok(repository) = repository_for_remote(root, remote).await {
            github.push((repository, Some(remote.to_owned())));
        }
    }
    match github.as_slice() {
        [repository] => Ok(repository.clone()),
        [] => Err(source_error(
            "no GitHub remote was found; supply --repository OWNER/NAME",
        )),
        _ => Err(source_error(
            "multiple GitHub remotes were found; supply --repository OWNER/NAME",
        )),
    }
}

async fn worktree_is_dirty(root: &Path) -> Result<bool, NativeV2CliError> {
    Ok(
        !git_at(root, &["status", "--porcelain", "--untracked-files=normal"])
            .await
            .map_err(|_| source_error("could not inspect worktree status"))?
            .is_empty(),
    )
}

async fn matching_remote(
    root: &Path,
    repository: &SourceRepositoryId,
    preferred: Option<&str>,
) -> Option<String> {
    if let Some(remote) = preferred {
        if repository_for_remote(root, remote).await.ok().as_ref() == Some(repository) {
            return Some(remote.to_owned());
        }
    }
    let remotes = git_at(root, &["remote"]).await.ok()?;
    for remote in remotes.lines().filter(|value| !value.is_empty()) {
        if repository_for_remote(root, remote).await.ok().as_ref() == Some(repository) {
            return Some(remote.to_owned());
        }
    }
    None
}

async fn repository_for_remote(
    root: &Path,
    remote: &str,
) -> Result<SourceRepositoryId, NativeV2CliError> {
    let url = git_at(root, &["remote", "get-url", remote])
        .await
        .map_err(|_| {
            source_error(
                "could not read the selected upstream remote; supply --repository OWNER/NAME",
            )
        })?;
    parse_github_repository(url.trim()).ok_or_else(|| {
        source_error("the selected remote is not GitHub; supply --repository OWNER/NAME")
    })
}

fn parse_github_repository(url: &str) -> Option<SourceRepositoryId> {
    let path = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    SourceRepositoryId::new(path.strip_suffix(".git").unwrap_or(path)).ok()
}

async fn remote_tip(
    selection: RevisionSelection<'_>,
) -> Result<SourceRevisionId, NativeV2CliError> {
    let reference = format!("refs/heads/{}", selection.branch.as_str());
    let fallback_url;
    let location = match selection.remote {
        Some(remote) => remote,
        None => {
            fallback_url = format!("https://github.com/{}.git", selection.repository.as_str());
            &fallback_url
        }
    };
    let mut command = git_command(Some(selection.root));
    command.arg("ls-remote").arg(location).arg(&reference);
    add_token(&mut command, selection.token);
    let output = run(&mut command).await.map_err(|_| {
        source_error(
            "could not resolve the selected remote branch tip; check repository access and --branch",
        )
    })?;
    let mut matches = output
        .lines()
        .filter_map(|line| line.strip_suffix(&format!("\t{reference}")));
    let revision = matches.next().ok_or_else(|| {
        source_error("the selected remote branch does not exist or is inaccessible")
    })?;
    if matches.next().is_some() {
        return Err(source_error(
            "the selected remote branch resolved ambiguously",
        ));
    }
    SourceRevisionId::new(revision).map_err(|error| {
        source_error(&format!(
            "remote branch returned an invalid revision: {error}"
        ))
    })
}

async fn git(args: &[&str]) -> Result<String, ()> {
    run(git_command(None).args(args)).await
}
async fn git_at(root: &Path, args: &[&str]) -> Result<String, ()> {
    run(git_command(Some(root)).args(args)).await
}
fn git_command(root: Option<&Path>) -> Command {
    let mut command = Command::new(
        std::env::var_os("ZEROSHOT_TARGET_GIT_PROGRAM").unwrap_or_else(|| "git".into()),
    );
    command
        .kill_on_drop(true)
        .env("LANG", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .stdin(Stdio::null());
    if let Some(root) = root {
        command.current_dir(root);
    }
    command
}
fn add_token(command: &mut Command, token: Option<&str>) {
    if let Some(token) = token {
        command
            .env("GIT_CONFIG_COUNT", "2")
            .env("GIT_CONFIG_KEY_0", "credential.helper")
            .env("GIT_CONFIG_VALUE_0", "")
            .env("GIT_CONFIG_KEY_1", "http.https://github.com/.extraheader")
            .env(
                "GIT_CONFIG_VALUE_1",
                format!("AUTHORIZATION: basic {}", encode_basic_credential(token)),
            );
    }
}
async fn run(command: &mut Command) -> Result<String, ()> {
    let output = tokio::time::timeout(GIT_TIMEOUT, command.output())
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
    if !output.status.success() || output.stdout.len() > MAX_OUTPUT {
        return Err(());
    }
    String::from_utf8(output.stdout).map_err(|_| ())
}
fn source_error(message: &str) -> NativeV2CliError {
    NativeV2CliError::Usage(format!("named target source selection failed: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(std::path::PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "zeroshot-named-source-{label}-{}",
                uuid::Uuid::now_v7()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parses_supported_github_remote_urls() {
        for url in [
            "git@github.com:owner/repo.git",
            "ssh://git@github.com/owner/repo.git",
            "https://github.com/owner/repo.git",
        ] {
            assert_eq!(
                parse_github_repository(url)
                    .map(|value| value.to_string())
                    .as_deref(),
                Some("owner/repo")
            );
        }
        assert!(parse_github_repository("https://example.com/owner/repo.git").is_none());
    }

    #[tokio::test]
    async fn explicit_repository_reuses_authenticated_ssh_remote() {
        let root = TestDirectory::new("ssh");
        run(std::process::Command::new("git").arg("init").arg(&root.0)).unwrap();
        run(std::process::Command::new("git")
            .current_dir(&root.0)
            .args([
                "remote",
                "add",
                "private",
                "git@github.com:owner/private.git",
            ]))
        .unwrap();

        let repository = SourceRepositoryId::new("owner/private").unwrap();
        assert_eq!(
            matching_remote(&root.0, &repository, None).await.as_deref(),
            Some("private")
        );
    }

    #[tokio::test]
    async fn github_upstream_takes_precedence_over_other_github_remotes() {
        let root = initialized_repository("github-upstream");
        add_remote(&root.0, "upstream", "git@github.com:owner/upstream.git");
        add_remote(&root.0, "fork", "https://github.com/owner/fork.git");

        let (repository, remote) = select_repository(&root.0, Some("upstream")).await.unwrap();

        assert_eq!(repository.as_str(), "owner/upstream");
        assert_eq!(remote.as_deref(), Some("upstream"));
    }

    #[tokio::test]
    async fn non_github_or_stale_upstream_falls_back_to_the_only_github_remote() {
        let root = initialized_repository("non-github-upstream");
        add_remote(&root.0, "mirror", "https://gitlab.com/owner/project.git");
        add_remote(&root.0, "github", "https://github.com/owner/project.git");

        for preferred in ["mirror", "stale"] {
            let (repository, remote) = select_repository(&root.0, Some(preferred)).await.unwrap();

            assert_eq!(repository.as_str(), "owner/project");
            assert_eq!(remote.as_deref(), Some("github"));
        }
    }

    #[tokio::test]
    async fn dirty_status_includes_untracked_unstaged_and_staged_changes() {
        let root = initialized_repository("dirty");
        let tracked = root.0.join("tracked.txt");
        std::fs::write(&tracked, "clean\n").unwrap();
        run(std::process::Command::new("git")
            .current_dir(&root.0)
            .args(["add", "tracked.txt"]))
        .unwrap();
        run(std::process::Command::new("git")
            .current_dir(&root.0)
            .args([
                "-c",
                "user.name=Zeroshot Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ]))
        .unwrap();
        assert!(!worktree_is_dirty(&root.0).await.unwrap());

        std::fs::write(root.0.join("untracked.txt"), "new\n").unwrap();
        assert!(worktree_is_dirty(&root.0).await.unwrap());
        std::fs::remove_file(root.0.join("untracked.txt")).unwrap();

        std::fs::write(&tracked, "changed\n").unwrap();
        assert!(worktree_is_dirty(&root.0).await.unwrap());

        run(std::process::Command::new("git")
            .current_dir(&root.0)
            .args(["add", "tracked.txt"]))
        .unwrap();
        assert!(worktree_is_dirty(&root.0).await.unwrap());
    }

    #[tokio::test]
    async fn git_commands_preserve_https_credential_helper_configuration() {
        let home = TestDirectory::new("credential-helper");
        std::fs::write(
            home.0.join(".gitconfig"),
            "[credential]\n\thelper = private-helper\n",
        )
        .unwrap();
        let mut command = git_command(None);
        command
            .env("HOME", &home.0)
            .args(["config", "--global", "--get", "credential.helper"]);

        assert_eq!(
            super::run(&mut command).await.unwrap().trim(),
            "private-helper"
        );
    }

    fn initialized_repository(label: &str) -> TestDirectory {
        let root = TestDirectory::new(label);
        run(std::process::Command::new("git").arg("init").arg(&root.0)).unwrap();
        root
    }

    fn add_remote(root: &Path, name: &str, url: &str) {
        run(std::process::Command::new("git")
            .current_dir(root)
            .args(["remote", "add", name, url]))
        .unwrap();
    }

    fn run(command: &mut std::process::Command) -> std::io::Result<()> {
        let status = command.status()?;
        assert!(status.success());
        Ok(())
    }
}
