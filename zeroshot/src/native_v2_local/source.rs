//! Local Git metadata is optional until the graph asks for delivery.

use openengine_cluster_protocol::{
    ConnectionKey, GitRemoteUrl, ResolvedGitRemote, ResolvedSource, SourceBranchId,
    SourceRepositoryId, SourceRevisionId,
};
use sha2::{Digest, Sha256};
use std::path::Path;

use super::{LocalCompositionError, git_line, git_output};

pub(super) fn resolved_git_source(
    workspace: &Path,
    git_program: &Path,
    connection: Option<ConnectionKey>,
) -> Result<ResolvedSource, LocalCompositionError> {
    if !git_line(
        git_program,
        workspace,
        &["rev-parse", "--is-inside-work-tree"],
    )
    .is_ok_and(|inside| inside == "true")
    {
        return Err(LocalCompositionError::DeliverySourceRequired);
    }
    let branch = git_line(
        git_program,
        workspace,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    )
    .map_err(|_| LocalCompositionError::DetachedHead)?;
    let revision = git_line(
        git_program,
        workspace,
        &["rev-parse", "--verify", "HEAD^{commit}"],
    )
    .map_err(|_| LocalCompositionError::ResolvedSource)?;
    let upstream = branch_upstream(workspace, git_program, &branch);
    let remote = select_remote(
        workspace,
        git_program,
        upstream.as_ref().map(|value| value.0.as_str()),
    )?;
    let branch = upstream.map_or(branch, |value| value.1);
    let forge = crate::source_code_provider::adapters::core::forge_identity(&remote)
        .map_err(|_| LocalCompositionError::RepositoryIdentity)?;
    let repository = forge.as_ref().map_or_else(
        || transport_repository(&remote),
        |forge| Ok(forge.repository.clone()),
    )?;
    Ok(ResolvedSource {
        repository,
        branch: SourceBranchId::new(branch).map_err(|_| LocalCompositionError::ResolvedSource)?,
        revision: SourceRevisionId::new(revision)
            .map_err(|_| LocalCompositionError::ResolvedSource)?,
        remote: Some(ResolvedGitRemote {
            url: remote,
            forge,
            connection,
        }),
    })
}

fn branch_upstream(workspace: &Path, git_program: &Path, branch: &str) -> Option<(String, String)> {
    let remote = git_line(
        git_program,
        workspace,
        &["config", "--get", &format!("branch.{branch}.remote")],
    )
    .ok()?;
    if remote == "." {
        return None;
    }
    let branch = git_line(
        git_program,
        workspace,
        &["config", "--get", &format!("branch.{branch}.merge")],
    )
    .ok()?;
    Some((remote, branch.strip_prefix("refs/heads/")?.to_owned()))
}

fn select_remote(
    workspace: &Path,
    git_program: &Path,
    preferred: Option<&str>,
) -> Result<GitRemoteUrl, LocalCompositionError> {
    if let Some(remote) = preferred {
        return remote_url(workspace, git_program, remote);
    }
    let remotes = git_output(git_program, workspace, &["remote"])?;
    let eligible: Vec<_> = remotes
        .lines()
        .filter_map(|name| remote_url(workspace, git_program, name).ok())
        .collect();
    match eligible.as_slice() {
        [remote] => Ok(remote.clone()),
        [] => Err(LocalCompositionError::RepositoryIdentity),
        _ => Err(LocalCompositionError::AmbiguousRemote),
    }
}

fn remote_url(
    workspace: &Path,
    git_program: &Path,
    name: &str,
) -> Result<GitRemoteUrl, LocalCompositionError> {
    let remote = git_line(git_program, workspace, &["remote", "get-url", name])
        .map_err(|_| LocalCompositionError::RepositoryIdentity)?;
    GitRemoteUrl::new(remote).map_err(|_| LocalCompositionError::RepositoryIdentity)
}

fn transport_repository(
    remote: &GitRemoteUrl,
) -> Result<SourceRepositoryId, LocalCompositionError> {
    SourceRepositoryId::new(remote.as_str())
        .or_else(|_| {
            SourceRepositoryId::new(format!(
                "git:{:x}",
                Sha256::digest(remote.as_str().as_bytes())
            ))
        })
        .map_err(|_| LocalCompositionError::ResolvedSource)
}
