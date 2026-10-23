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

pub(super) fn select_remote(
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

/// Observes a new local PR/merge run's base using the same context and authority as delivery.
pub(crate) async fn validate_remote_base_branch(
    prepared: &super::PreparedLocalRun,
    git_program: &Path,
    state_root: &Path,
) -> Result<(), LocalCompositionError> {
    use crate::native_v2_delivery::{github_git_token, resolved_git_credentials, transport};
    use crate::native_v2_contract::NodeRuntimeBinding;

    let source = prepared
        .submission
        .source
        .as_ref()
        .ok_or(LocalCompositionError::DeliverySourceRequired)?;
    let binding = prepared
        .submission
        .runtime
        .nodes()
        .values()
        .find(|binding| matches!(binding, NodeRuntimeBinding::GitDelivery { .. }))
        .ok_or(LocalCompositionError::DeliverySourceRequired)?;
    let environment = prepared.environment.resolve(binding).await?;
    let token = github_git_token(&environment, prepared.github_token.as_deref())
        .map_err(|_| LocalCompositionError::DeliveryCredentials)?;
    let credentials = resolved_git_credentials(&environment, prepared.git_credentials.as_ref())
        .map_err(|_| LocalCompositionError::DeliveryCredentials)?;
    let token_from_file = if token.is_none()
        && credentials.is_none()
        && crate::source_code_provider::adapters::core::delivery_uses_github_cli(
            source,
            &[crate::source_code_provider::SourceCapability::PullRequest],
        ) {
        prepared
            .github_token_file
            .as_ref()
            .map(|file| file.read())
            .transpose()
            .map_err(|_| LocalCompositionError::GitHubTokenFile)?
    } else {
        None
    };
    let private_directory = state_root.join("git-preflight");
    super::prepare_private_directory(&private_directory)?;
    let transport = transport::GitTransport::new(
        transport::GitTransportConfig {
            git_program: git_program.to_owned(),
            workspace: prepared.workspace.clone(),
            home_directory: private_directory.clone(),
            private_directory,
            identity: None,
            local_environment: Some(prepared.native_environment.clone()),
            deadline: std::time::Duration::from_secs(10 * 60),
        },
        crate::source_code_provider::adapters::core::source_remote(source),
    );
    let credential = transport::source_credential(
        token.or(token_from_file.as_deref()).unwrap_or(""),
        credentials.as_ref(),
    );
    match transport.read_ref(source.branch.as_str(), credential).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(LocalCompositionError::MissingRemoteBase {
            branch: source.branch.to_string(),
        }),
        Err(_) => Err(LocalCompositionError::RemoteBaseCheck {
            branch: source.branch.to_string(),
        }),
    }
}
