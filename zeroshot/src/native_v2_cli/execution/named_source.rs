use std::path::{Path, PathBuf};
use std::ffi::OsString;
use std::process::Stdio;

use openengine_cluster_protocol::{
    GitRemoteUrl, GitTransportCredentials, ResolvedGitRemote, RunConnectionValues, ResolvedSource,
    SourceBranchId, SourceRepositoryId, SourceRevisionId,
};
use tokio::process::Command;

use crate::native_v2_cli::{NamedRunSource, NativeV2CliError, RunCommand, RunRepository};

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
    git_credentials: Option<&'a GitTransportCredentials>,
}

pub(super) struct NamedSourceResolution {
    pub(super) source: NamedRunSource,
    pub(super) git_credentials: Option<GitTransportCredentials>,
}

pub(super) struct NamedSourceRequest<'a> {
    pub(super) run: &'a RunCommand,
    pub(super) token: Option<&'a str>,
    pub(super) provided: &'a RunConnectionValues,
    pub(super) required_capabilities: &'a [crate::source_code_provider::SourceCapability],
}

pub(super) async fn resolve<F>(
    request: NamedSourceRequest<'_>,
    available: &F,
) -> Result<Option<NamedSourceResolution>, NativeV2CliError>
where
    F: Fn(&str) -> Option<OsString>,
{
    let NamedSourceRequest {
        run,
        token,
        provided,
        ..
    } = request;
    if run.target.is_none() || run.validate_only {
        return Ok(None);
    }
    let worktree = inspect_worktree(run).await?;
    let (repository, remote, metadata) = selected_repository(&request, &worktree).await?;
    let branch = select_run_branch(run, &worktree)?;
    let git_credentials = source_credentials(metadata.as_ref(), provided, available)?;
    let revision = select_run_revision(
        run,
        RevisionSelection {
            root: &worktree.root,
            repository: &repository,
            branch: &branch,
            remote: remote.as_deref(),
            token,
            git_credentials: git_credentials.as_ref(),
        },
    )
    .await?;
    let dirty = worktree_is_dirty(&worktree.root).await?;
    Ok(Some(NamedSourceResolution {
        git_credentials,
        source: NamedRunSource {
            resolved: ResolvedSource {
                repository,
                branch,
                revision,
                remote: metadata,
            },
            dirty,
        },
    }))
}

async fn selected_repository(
    request: &NamedSourceRequest<'_>,
    worktree: &WorktreeSourceContext,
) -> Result<
    (
        SourceRepositoryId,
        Option<String>,
        Option<ResolvedGitRemote>,
    ),
    NativeV2CliError,
> {
    let (repository, remote) = select_run_repository(request.run, worktree).await?;
    let metadata = resolved_remote(request.run, &worktree.root, remote.as_deref()).await?;
    crate::source_code_provider::adapters::core::validate_delivery_repository(
        repository.as_str(),
        metadata.as_ref(),
        request.required_capabilities,
    )
    .map_err(|error| source_error(&error.to_string()))?;
    Ok((repository, remote, metadata))
}

fn source_credentials<F>(
    remote: Option<&ResolvedGitRemote>,
    provided: &RunConnectionValues,
    available: &F,
) -> Result<Option<GitTransportCredentials>, NativeV2CliError>
where
    F: Fn(&str) -> Option<OsString>,
{
    let Some(remote) = remote else {
        return Ok(None);
    };
    let requirements = remote
        .connection_requirements()
        .into_iter()
        .map(|(key, fields)| (key, fields.into_iter().collect()))
        .collect();
    let selected = super::select_connection_requirements(requirements, |name| {
        remote
            .connection
            .as_ref()
            .and_then(|key| provided.get(key))
            .and_then(|values| {
                values
                    .as_map()
                    .iter()
                    .find(|(field, _)| field.as_str() == name)
            })
            .map(|(_, value)| OsString::from(value))
            .or_else(|| available(name))
    })?;
    let Some(values) = selected.values().next() else {
        return Ok(None);
    };
    GitTransportCredentials::from_connection_values(values)
        .map_err(|error| source_error(&error.to_string()))
}

async fn inspect_worktree(run: &RunCommand) -> Result<WorktreeSourceContext, NativeV2CliError> {
    let directory = run.cwd.as_deref().unwrap_or(Path::new("."));
    let root = git_at(directory, &["rev-parse", "--show-toplevel"])
        .await
        .map_err(|_| {
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
            "detached worktrees require both --repository REPOSITORY and --branch BRANCH",
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
        Some(RunRepository::Git(remote)) => (
            remote_repository(remote.as_str())?,
            Some(remote.to_string()),
        ),
        Some(RunRepository::GitHub(repository)) => (
            repository.clone(),
            matching_github_remote(&worktree.root, repository, upstream_remote).await,
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
        if let Some(repository) = eligible_repository(root, remote).await? {
            return Ok((repository, Some(remote.to_owned())));
        }
    }
    let remotes = git_at(root, &["remote"])
        .await
        .map_err(|_| source_error("could not list Git remotes; supply --repository OWNER/NAME"))?;
    let mut eligible = Vec::new();
    for remote in remotes.lines() {
        if let Some(repository) = eligible_repository(root, remote).await? {
            eligible.push((repository, Some(remote.to_owned())));
        }
    }
    unique_repository(&eligible)
}

fn unique_repository(
    eligible: &[(SourceRepositoryId, Option<String>)],
) -> Result<(SourceRepositoryId, Option<String>), NativeV2CliError> {
    match eligible {
        [repository] => Ok(repository.clone()),
        [] => Err(source_error(
            "no eligible HTTPS or SSH Git remote was found; supply --repository REPOSITORY",
        )),
        _ => Err(source_error(
            "multiple eligible Git remotes were found; configure an upstream or supply --repository REPOSITORY",
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

// The owner/name shorthand explicitly selects GitHub, independently of other forge IDs.
async fn matching_github_remote(
    root: &Path,
    repository: &SourceRepositoryId,
    preferred: Option<&str>,
) -> Option<String> {
    let remotes = git_at(root, &["remote"]).await.unwrap_or_default();
    for remote in preferred
        .into_iter()
        .chain(remotes.lines())
        .filter(|value| !value.is_empty())
    {
        let Ok(url) = remote_location(root, remote).await else {
            continue;
        };
        if crate::source_code_provider::adapters::core::github_repository_reference(url.as_str())
            .as_deref()
            == Some(repository.as_str())
        {
            return Some(remote.to_owned());
        }
    }
    None
}

async fn eligible_repository(
    root: &Path,
    remote: &str,
) -> Result<Option<SourceRepositoryId>, NativeV2CliError> {
    let Ok(url) = remote_location(root, remote).await else {
        return Ok(None);
    };
    remote_repository(url.as_str()).map(Some)
}

async fn remote_location(root: &Path, remote: &str) -> Result<GitRemoteUrl, NativeV2CliError> {
    if let Ok(url) = GitRemoteUrl::new(remote) {
        return Ok(url);
    }
    let url = git_at(root, &["remote", "get-url", remote])
        .await
        .map_err(|_| source_error("could not read the selected Git remote"))?;
    GitRemoteUrl::new(url.trim())
        .map_err(|_| source_error("the selected remote must be credential-free HTTPS or SSH"))
}

fn remote_repository(url: &str) -> Result<SourceRepositoryId, NativeV2CliError> {
    let remote = GitRemoteUrl::new(url)
        .map_err(|_| source_error("the selected remote must be credential-free HTTPS or SSH"))?;
    if let Some(forge) = crate::source_code_provider::adapters::core::forge_identity(&remote)
        .map_err(|error| source_error(&error.to_string()))?
    {
        return Ok(forge.repository);
    }
    use sha2::{Digest, Sha256};
    SourceRepositoryId::new(url)
        .or_else(|_| SourceRepositoryId::new(format!("git:{:x}", Sha256::digest(url.as_bytes()))))
        .map_err(|_| source_error("invalid Git source identity"))
}

async fn selected_remote_url(
    run: &RunCommand,
    root: &Path,
    selected: Option<&str>,
) -> Result<Option<GitRemoteUrl>, NativeV2CliError> {
    match selected {
        Some(selected) => remote_location(root, selected).await.map(Some),
        None => match (&run.repository, &run.source_connection) {
            (Some(RunRepository::GitHub(repository)), Some(_)) => {
                GitRemoteUrl::new(format!("https://github.com/{repository}.git"))
                    .map(Some)
                    .map_err(|_| source_error("invalid GitHub transport locator"))
            }
            _ => Ok(None),
        },
    }
}

async fn resolved_remote(
    run: &RunCommand,
    root: &Path,
    selected: Option<&str>,
) -> Result<Option<ResolvedGitRemote>, NativeV2CliError> {
    let Some(url) = selected_remote_url(run, root, selected).await? else {
        return Ok(None);
    };
    let forge = crate::source_code_provider::adapters::core::forge_identity(&url)
        .map_err(|error| source_error(&error.to_string()))?;
    let explicit_url = matches!(run.repository.as_ref(), Some(RunRepository::Git(_)));
    if forge
        .as_ref()
        .is_some_and(|forge| forge.provider.as_str() == "source.github")
        && !explicit_url
        && run.source_connection.is_none()
    {
        return Ok(None);
    }
    Ok(Some(ResolvedGitRemote {
        url,
        forge,
        connection: run.source_connection.clone(),
    }))
}

async fn remote_tip(
    selection: RevisionSelection<'_>,
) -> Result<SourceRevisionId, NativeV2CliError> {
    let fallback_url;
    let location = match selection.remote {
        Some(remote) => remote,
        None => {
            fallback_url = format!("https://github.com/{}.git", selection.repository.as_str());
            &fallback_url
        }
    };
    let remote = if GitRemoteUrl::new(location).is_ok() {
        location.to_owned()
    } else {
        remote_location(selection.root, location).await?.to_string()
    };
    let native = crate::native_v2_local::capture_local_native_environment(selection.root)
        .map_err(|_| source_error("could not capture native Git context"))?;
    let private =
        tempfile::tempdir().map_err(|_| source_error("could not prepare private Git context"))?;
    let transport = crate::native_v2_delivery::transport::GitTransport::new(
        crate::native_v2_delivery::transport::GitTransportConfig {
            git_program: std::env::var_os("ZEROSHOT_TARGET_GIT_PROGRAM")
                .unwrap_or_else(|| "git".into())
                .into(),
            workspace: selection.root.to_owned(),
            home_directory: private.path().to_owned(),
            private_directory: private.path().to_owned(),
            identity: None,
            local_environment: Some(native),
            deadline: GIT_TIMEOUT,
        },
        remote,
    );
    let credential = crate::native_v2_delivery::transport::source_credential(
        selection.token.unwrap_or(""),
        selection.git_credentials,
    );
    let revision = transport
        .read_ref(selection.branch.as_str(), credential)
        .await
        .map_err(|_| {
            source_error(
                "could not resolve the remote branch tip; check repository access and --branch",
            )
        })?
        .ok_or_else(|| {
            source_error("the selected remote branch does not exist or is inaccessible")
        })?;
    SourceRevisionId::new(revision)
        .map_err(|_| source_error("remote branch returned an invalid Git revision"))
}

async fn git_at(root: &Path, args: &[&str]) -> Result<String, ()> {
    run(git_command(Some(root)).args(args)).await
}
fn git_command(root: Option<&Path>) -> Command {
    let mut command = Command::new(
        std::env::var_os("ZEROSHOT_TARGET_GIT_PROGRAM").unwrap_or_else(|| "git".into()),
    );
    crate::native_v2_delivery::transport::remove_repository_environment(command.as_std_mut());
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
#[path = "named_source/tests.rs"]
mod tests;
