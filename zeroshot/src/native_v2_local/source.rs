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
    let branch = attached_branch(workspace, git_program)?;
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

pub(super) fn branch_upstream(
    workspace: &Path,
    git_program: &Path,
    branch: &str,
) -> Option<(String, String)> {
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

/// Resolve local transport with delivery's native context and declared Git authority.
pub(super) async fn local_transport(
    prepared: &super::PreparedLocalRun,
    git_program: &Path,
    state_root: &Path,
) -> Result<
    (
        crate::native_v2_delivery::transport::GitTransport,
        Option<String>,
        Option<openengine_cluster_protocol::GitTransportCredentials>,
    ),
    LocalCompositionError,
> {
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
        .find(|binding| matches!(binding, NodeRuntimeBinding::GitDelivery { .. }));
    let environment = match binding {
        Some(binding) => Some(prepared.environment.resolve(binding).await?),
        None => None,
    };
    let token = match &environment {
        Some(environment) => github_git_token(environment, prepared.github_token.as_deref())
            .map_err(|_| LocalCompositionError::DeliveryCredentials)?,
        None => prepared.github_token.as_deref(),
    };
    let credentials = match &environment {
        Some(environment) => {
            resolved_git_credentials(environment, prepared.git_credentials.as_ref())
                .map_err(|_| LocalCompositionError::DeliveryCredentials)?
        }
        None => prepared.git_credentials.clone(),
    };
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
    Ok((
        transport,
        token.or(token_from_file.as_deref()).map(str::to_owned),
        credentials,
    ))
}

/// Observes a new local PR/merge run's target with delivery's native transport and credentials.
pub(crate) async fn validate_remote_base_branch(
    prepared: &super::PreparedLocalRun,
    git_program: &Path,
    state_root: &Path,
) -> Result<(), LocalCompositionError> {
    let source = prepared
        .submission
        .source
        .as_ref()
        .ok_or(LocalCompositionError::DeliverySourceRequired)?;
    let (transport, token, credentials) =
        local_transport(prepared, git_program, state_root).await?;
    let credential = super::super::native_v2_delivery::transport::source_credential(
        token.as_deref().unwrap_or(""),
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

/// Delivery and worktree preparation require an attached branch. Reflog selectors identify the
/// branch to return to without switching refs or guessing from an ambiguous commit decoration.
/// The CLI can supplement this evidence from its retained workspace recovery metadata.
pub(super) fn attached_branch(
    workspace: &Path,
    git_program: &Path,
) -> Result<String, LocalCompositionError> {
    git_line(
        git_program,
        workspace,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    )
    .map_err(|_| {
        let branch = {
            git_output(
                git_program,
                workspace,
                &["reflog", "show", "-n", "32", "--format=%gs", "HEAD"],
            )
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.strip_prefix("checkout: moving from "))
            .filter_map(|line| line.split_once(" to "))
            // A restored HEAD has no checkout event: prefer the last attached destination.
            // An explicit detached checkout has a commit destination, so use its origin.
            .flat_map(|(from, to)| [to, from])
            .find(|name| verified_detached_branch(workspace, git_program, name))
            .map(str::to_owned)
        };
        branch.map_or(LocalCompositionError::DetachedHeadUnknown, |branch| {
            LocalCompositionError::DetachedHead { branch }
        })
    })
}

/// Recovery hints must still name an existing descendant branch available to this checkout.
/// Neither a stale recovery record nor a reflog entry authorizes switching a shared ref.
pub(crate) fn verified_detached_branch(workspace: &Path, git_program: &Path, branch: &str) -> bool {
    if !crate::source_code_provider::valid_git_branch(branch) {
        return false;
    }
    let reference = format!("refs/heads/{branch}");
    git_output(
        git_program,
        workspace,
        &["show-ref", "--verify", &reference],
    )
    .is_ok()
        && git_output(
            git_program,
            workspace,
            &["merge-base", "--is-ancestor", "HEAD", &reference],
        )
        .is_ok()
        && super::workspace::branch_owner_elsewhere(workspace, git_program, branch)
            .is_ok_and(|owner| owner.is_none())
}
