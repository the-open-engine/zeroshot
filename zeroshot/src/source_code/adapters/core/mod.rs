//! Maintained Git and GitHub adapters and their shared source preflight/factory.
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use crate::native_v2_contract::ResolvedSource;
use crate::native_v2_delivery::transport::{GitTransport, GitTransportConfig};
use crate::source_code_provider::provider::*;
use crate::execution::process::HostedProcessIdentity;
use crate::native_v2_target_authority::OperatorDiagnosticStore;

mod git;
pub(crate) mod github;
mod provider;
pub use github::{GhCliAuthorityConfig, GhCliDeliveryAuthority};
pub use github::validate_comment_review_scope;
pub use git::GitDeliveryAuthority;
use provider::{CoreSourceProvider, ProviderKind};

pub const GIT_PROVIDER_ID: &str = "source.git";
pub const GITHUB_PROVIDER_ID: &str = "source.github";

pub(crate) struct SourceDeliveryFactoryRequest<'a> {
    pub source: &'a ResolvedSource,
    pub workspace: &'a Path,
    pub git_program: &'a Path,
    pub gh_program: &'a Path,
    pub home_directory: &'a Path,
    pub private_directory: &'a Path,
    pub git_identity: Option<HostedProcessIdentity>,
    pub local_environment: Option<BTreeMap<String, String>>,
    pub required_capabilities: &'a [SourceCapability],
    pub operator_diagnostics: Option<(
        openengine_cluster_protocol::RunId,
        Arc<OperatorDiagnosticStore>,
    )>,
}

#[derive(Debug, thiserror::Error)]
pub enum SourceDeliveryFactoryError {
    #[error("run source identities require provider version 1; received {provider}")]
    UnsupportedRemoteVersion { provider: SourceProviderRef },
    #[error("source provider returned an identity outside run source bounds")]
    RemoteIdentity,
    #[error("PR and merge delivery require a registered forge identity")]
    ForgeRequired,
    #[error("GitHub repository identity must have the form owner/name")]
    GitHubRepository,
    #[error("GitHub forge identity must match its Git remote")]
    GitHubRemoteIdentity,
    #[error(transparent)]
    Registry(#[from] SourceRegistryError),
    #[error(transparent)]
    Contract(#[from] SourceContractError),
    #[error(transparent)]
    Source(#[from] SourceCallError),
}

/// Resolves an optional forge through the same maintained/contributed metadata registry.
pub fn forge_identity(
    remote: &openengine_cluster_protocol::GitRemoteUrl,
) -> Result<Option<openengine_cluster_protocol::SourceForgeIdentity>, SourceDeliveryFactoryError> {
    let selected = SourceProviderRef::new(SourceProviderId::new(GIT_PROVIDER_ID)?, 1)?;
    let registry = maintained_registry(None, SourceAuthorities::default(), &selected, None)?;
    forge_identity_with_registry(remote.as_str(), &registry)
}

/// Pure remote identification for embedders using their configured registry.
pub fn forge_identity_with_registry(
    remote: &str,
    registry: &SourceCodeProviderRegistry,
) -> Result<Option<openengine_cluster_protocol::SourceForgeIdentity>, SourceDeliveryFactoryError> {
    let Some((provider, repository)) = registry.repository_from_remote(remote)? else {
        return Ok(None);
    };
    if provider.version() != 1 {
        return Err(SourceDeliveryFactoryError::UnsupportedRemoteVersion { provider });
    }
    Ok(Some(openengine_cluster_protocol::SourceForgeIdentity {
        provider: openengine_cluster_protocol::SourceForgeProviderId::new(provider.id().as_str())
            .map_err(|_| SourceDeliveryFactoryError::RemoteIdentity)?,
        repository: openengine_cluster_protocol::SourceRepositoryId::new(repository.as_str())
            .map_err(|_| SourceDeliveryFactoryError::RemoteIdentity)?,
    }))
}

fn provider_ref(
    source: &ResolvedSource,
    required: &[SourceCapability],
) -> Result<SourceProviderRef, SourceDeliveryFactoryError> {
    provider_ref_parts(source.repository.as_str(), source.remote.as_ref(), required)
}

fn provider_ref_parts(
    repository: &str,
    remote: Option<&crate::native_v2_contract::ResolvedGitRemote>,
    required: &[SourceCapability],
) -> Result<SourceProviderRef, SourceDeliveryFactoryError> {
    let forge = required
        .iter()
        .any(|c| !matches!(c, SourceCapability::Read | SourceCapability::Push));
    let id = if !forge {
        GIT_PROVIDER_ID
    } else if let Some(remote) = remote {
        remote
            .forge
            .as_ref()
            .ok_or(SourceDeliveryFactoryError::ForgeRequired)?
            .provider
            .as_str()
    } else {
        GITHUB_PROVIDER_ID
    };
    if id == GITHUB_PROVIDER_ID {
        let repository = remote
            .and_then(|r| r.forge.as_ref())
            .map_or(repository, |forge| forge.repository.as_str());
        if !valid_github_repository(repository) {
            return Err(SourceDeliveryFactoryError::GitHubRepository);
        }
        if let Some(remote) = remote {
            let matches = github_repository_reference(remote.url.as_str())
                .is_some_and(|identified| identified.eq_ignore_ascii_case(repository));
            if !matches {
                return Err(SourceDeliveryFactoryError::GitHubRemoteIdentity);
            }
        }
    }
    Ok(SourceProviderRef::new(SourceProviderId::new(id)?, 1)?)
}

#[derive(Default)]
struct SourceAuthorities {
    delivery: Option<Arc<dyn SourceDeliveryAuthority>>,
    comment: Option<Arc<dyn SourceCommentAuthority>>,
}

fn maintained_registry(
    source: Option<&ResolvedSource>,
    authorities: SourceAuthorities,
    selected: &SourceProviderRef,
    transport: Option<&GitTransport>,
) -> Result<SourceCodeProviderRegistry, SourceDeliveryFactoryError> {
    let mut registry = SourceCodeProviderRegistry::new();
    for kind in [ProviderKind::Git, ProviderKind::GitHub] {
        let adapter = if selected.id().as_str() == kind.id() {
            authorities.delivery.clone()
        } else {
            None
        };
        let comment = (selected.id().as_str() == kind.id())
            .then(|| authorities.comment.clone())
            .flatten();
        registry.register(Arc::new(
            CoreSourceProvider::new(kind, source.cloned(), adapter)?
                .with_comment_authority(comment),
        ))?;
    }
    super::contributed::register(
        &mut registry,
        super::SourceAdapterContext { source, transport },
    )?;
    Ok(registry)
}

/// Capability preflight is pure: it registers adapter descriptors and never reads credentials,
/// files, Git state, or a network endpoint.
pub fn validate_delivery_source(
    source: &ResolvedSource,
    required: &[SourceCapability],
) -> Result<(), SourceDeliveryFactoryError> {
    let selected = provider_ref(source, required)?;
    let registry =
        maintained_registry(Some(source), SourceAuthorities::default(), &selected, None)?;
    validate_delivery_source_with_registry(source, required, &registry)
}

/// Check selected repository capabilities before resolving transport credentials or remote refs.
pub fn validate_delivery_repository(
    repository: &str,
    remote: Option<&crate::native_v2_contract::ResolvedGitRemote>,
    required: &[SourceCapability],
) -> Result<(), SourceDeliveryFactoryError> {
    if required.is_empty() {
        return Ok(());
    }
    let selected = provider_ref_parts(repository, remote, required)?;
    let registry = maintained_registry(None, SourceAuthorities::default(), &selected, None)?;
    let profile = SourceProfileId::new("default")?;
    for capability in required {
        registry.capability(&selected, &profile, *capability)?;
    }
    Ok(())
}

/// The same pure capability check for a host that supplies a configured contribution registry.
pub fn validate_delivery_source_with_registry(
    source: &ResolvedSource,
    required: &[SourceCapability],
    registry: &SourceCodeProviderRegistry,
) -> Result<(), SourceDeliveryFactoryError> {
    let selected = provider_ref(source, required)?;
    let profile = SourceProfileId::new("default")?;
    for capability in required {
        registry.capability(&selected, &profile, *capability)?;
    }
    Ok(())
}

/// Whether [`delivery_authority`] selects the GitHub adapter, which runs the GitHub CLI.
pub(crate) fn delivery_uses_github_cli(
    source: &ResolvedSource,
    required: &[SourceCapability],
) -> bool {
    provider_ref(source, required)
        .is_ok_and(|selected| selected.id().as_str() == GITHUB_PROVIDER_ID)
}

pub(crate) fn delivery_authority(
    mut request: SourceDeliveryFactoryRequest<'_>,
) -> Result<Arc<dyn SourceDeliveryAuthority>, SourceDeliveryFactoryError> {
    validate_delivery_source(request.source, request.required_capabilities)?;
    let selected = provider_ref(request.source, request.required_capabilities)?;
    let remote = source_remote(request.source);
    let mut transport = GitTransport::new(
        GitTransportConfig {
            git_program: request.git_program.to_owned(),
            workspace: request.workspace.to_owned(),
            home_directory: request.home_directory.to_owned(),
            private_directory: request.private_directory.to_owned(),
            identity: request.git_identity,
            local_environment: request.local_environment.take(),
            deadline: std::time::Duration::from_secs(10 * 60),
        },
        remote,
    );
    if let Some((run_id, store)) = &request.operator_diagnostics {
        transport = transport.with_operator_diagnostics(run_id.clone(), store.clone());
    }
    let authority: Option<Arc<dyn SourceDeliveryAuthority>> =
        if selected.id().as_str() == GIT_PROVIDER_ID {
            Some(Arc::new(GitDeliveryAuthority::new(transport.clone())))
        } else if selected.id().as_str() == GITHUB_PROVIDER_ID {
            let mut github = GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
                git_identity: request.git_identity,
                git_program: request.git_program.to_owned(),
                gh_program: request.gh_program.to_owned(),
                ..GhCliAuthorityConfig::hosted(request.home_directory.to_owned())
            })
            .with_git_transport(transport.clone());
            if let Some((run_id, store)) = &request.operator_diagnostics {
                github = github.with_operator_diagnostics(run_id.clone(), store.clone());
            }
            Some(Arc::new(github))
        } else {
            None
        };
    let registry = maintained_registry(
        Some(request.source),
        SourceAuthorities {
            delivery: authority,
            comment: None,
        },
        &selected,
        Some(&transport),
    )?;
    delivery_authority_with_registry(request, &registry)
}

/// A host may supply registered runtime adapters without editing provider selection or the workflow.
pub(crate) fn delivery_authority_with_registry(
    request: SourceDeliveryFactoryRequest<'_>,
    registry: &SourceCodeProviderRegistry,
) -> Result<Arc<dyn SourceDeliveryAuthority>, SourceDeliveryFactoryError> {
    validate_delivery_source_with_registry(
        request.source,
        request.required_capabilities,
        registry,
    )?;
    let selected = provider_ref(request.source, request.required_capabilities)?;
    Ok(registry.delivery_authority(
        &selected,
        &SourceProfileId::new("default")?,
        request.required_capabilities,
    )?)
}

/// Canonical GitHub grammar belongs to this adapter. Authored Git paths and SSH usernames are
/// preserved by the source itself; this helper only derives an optional forge identity.
#[must_use]
pub fn github_repository_reference(value: &str) -> Option<String> {
    let remote = openengine_cluster_protocol::GitRemoteUrl::new(value).ok()?;
    let value = remote.as_str();
    let path = if value.contains("://") {
        let url = url::Url::parse(value).ok()?;
        if !matches!(
            (url.scheme(), url.host_str(), url.port()),
            (_, Some("github.com"), _) | ("ssh", Some("ssh.github.com"), Some(443))
        ) {
            return None;
        }
        url.path().trim_start_matches('/').to_owned()
    } else {
        let (host, path) = value.split_once(':')?;
        if host.rsplit('@').next()? != "github.com" {
            return None;
        }
        path.to_owned()
    };
    let repository = path.strip_suffix(".git").unwrap_or(&path);
    valid_github_repository(repository).then(|| repository.to_owned())
}

pub(crate) fn valid_github_repository(value: &str) -> bool {
    let Some((owner, name)) = value.split_once('/') else {
        return false;
    };
    !owner.is_empty()
        && !name.is_empty()
        && !name.contains('/')
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/'))
}

/// Legacy GitHub sources and generic transports share one immutable locator selection.
pub(crate) fn source_remote(source: &ResolvedSource) -> String {
    source.remote.as_ref().map_or_else(
        || format!("https://github.com/{}.git", source.repository.as_str()),
        |remote| remote.url.as_str().to_owned(),
    )
}

fn materialization_identity(
    source: &ResolvedSource,
) -> Result<SourceIdentifyRepositoryRequest, SourceDeliveryFactoryError> {
    Ok(SourceIdentifyRepositoryRequest::new(
        provider_ref(source, &[SourceCapability::Read])?,
        SourceProfileId::new("default")?,
        (
            SourceAccountId::new("workspace")?,
            SourceCredentialHandleId::new("git-transport")?,
        ),
        SourceRepositoryReference::new(source.repository.as_str())?,
    )?)
}

pub(crate) async fn materialize_source(
    source: &ResolvedSource,
    destination: SourceMaterializationDestination<'_>,
) -> Result<SourceMaterializationReceipt, SourceDeliveryFactoryError> {
    let identify = materialization_identity(source)?;
    let registry = maintained_registry(
        Some(source),
        SourceAuthorities::default(),
        identify.provider(),
        None,
    )?;
    let repository = registry.identify_repository(&identify).await?;
    let request = SourceMaterializeRequest::new(
        repository,
        identify.credential_handle().clone(),
        SourceRevisionId::new(source.revision.as_str())?,
    )?;
    Ok(registry.materialize(&request, destination).await?)
}

/// Pure destination/capability selection through the existing source registry.
pub fn comment_destination(
    url: &str,
) -> Result<SourceCommentDestination, SourceDeliveryFactoryError> {
    let selected = SourceProviderRef::new(SourceProviderId::new(GITHUB_PROVIDER_ID)?, 1)?;
    let registry = maintained_registry(None, SourceAuthorities::default(), &selected, None)?;
    Ok(registry.comment_destination(url)?)
}

/// Constructs only forge authority; source-less comments allocate no Git transport.
pub fn comment_authority(
    config: GhCliAuthorityConfig,
    url: &str,
) -> Result<Arc<dyn SourceCommentAuthority>, SourceDeliveryFactoryError> {
    let destination = comment_destination(url)?;
    let authority: Arc<dyn SourceCommentAuthority> = Arc::new(GhCliDeliveryAuthority::new(config));
    let registry = maintained_registry(
        None,
        SourceAuthorities {
            delivery: None,
            comment: Some(authority),
        },
        &destination.provider,
        None,
    )?;
    Ok(registry.comment_authority(&destination)?)
}

#[cfg(test)]
mod tests;
