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
pub(crate) mod gitlab;
pub(crate) mod instance;
mod provider;
pub use github::{GhCliAuthorityConfig, GhCliDeliveryAuthority};
pub use instance::forge_base_url;
pub use git::GitDeliveryAuthority;
use provider::{CoreSourceProvider, ProviderKind};

pub const GIT_PROVIDER_ID: &str = "source.git";
pub const GITHUB_PROVIDER_ID: &str = "source.github";
pub const GITLAB_PROVIDER_ID: &str = "source.gitlab";

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
    #[error("{0}")]
    Instance(&'static str),
    #[error(transparent)]
    Delivery(#[from] SourceDeliveryError),
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
        base_url: None,
    }))
}

/// Identifies a remote through one registered provider and explicit instance, without effects.
pub fn forge_identity_for(
    remote: &openengine_cluster_protocol::GitRemoteUrl,
    provider: &str,
    base_url: &openengine_cluster_protocol::SourceForgeBaseUrl,
) -> Result<openengine_cluster_protocol::SourceForgeIdentity, SourceDeliveryFactoryError> {
    let selected = SourceProviderRef::new(SourceProviderId::new(provider)?, 1)?;
    let registry = maintained_registry(None, SourceAuthorities::default(), &selected, None)?;
    let repository = registry
        .lookup(&selected)?
        .repository_from_remote_at(remote.as_str(), base_url.as_str())
        .ok_or(SourceDeliveryFactoryError::Instance(
            "forge identity does not match its Git remote",
        ))?;
    Ok(openengine_cluster_protocol::SourceForgeIdentity {
        provider: openengine_cluster_protocol::SourceForgeProviderId::new(provider)
            .map_err(|_| SourceDeliveryFactoryError::RemoteIdentity)?,
        repository: openengine_cluster_protocol::SourceRepositoryId::new(repository.as_str())
            .map_err(|_| SourceDeliveryFactoryError::RemoteIdentity)?,
        base_url: Some(base_url.clone()),
    })
}

/// Pure credential metadata from the same registry used by admission and effects.
pub fn forge_credential_field(
    provider: &str,
) -> Result<Option<&'static str>, SourceDeliveryFactoryError> {
    let selected = SourceProviderRef::new(SourceProviderId::new(provider)?, 1)?;
    let registry = maintained_registry(None, SourceAuthorities::default(), &selected, None)?;
    Ok(registry
        .lookup(&selected)?
        .default_forge_connection()
        .map(|(_, field)| field))
}

/// Canonical missing connection requirement, declared by the selected adapter.
pub fn forge_connection_requirement(
    provider: &str,
) -> Result<Option<(&'static str, &'static str)>, SourceDeliveryFactoryError> {
    let selected = SourceProviderRef::new(SourceProviderId::new(provider)?, 1)?;
    let registry = maintained_registry(None, SourceAuthorities::default(), &selected, None)?;
    Ok(registry.lookup(&selected)?.default_forge_connection())
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
        validate_github_identity(repository, remote)?;
    }
    if id == GITLAB_PROVIDER_ID {
        validate_forge_remote(remote)?;
    }
    Ok(SourceProviderRef::new(SourceProviderId::new(id)?, 1)?)
}

fn validate_github_identity(
    repository: &str,
    remote: Option<&crate::native_v2_contract::ResolvedGitRemote>,
) -> Result<(), SourceDeliveryFactoryError> {
    let repository = remote
        .and_then(|r| r.forge.as_ref())
        .map_or(repository, |forge| forge.repository.as_str());
    if !valid_github_repository(repository) {
        return Err(SourceDeliveryFactoryError::GitHubRepository);
    }
    if let Some(remote) = remote {
        let forge = remote
            .forge
            .as_ref()
            .ok_or(SourceDeliveryFactoryError::ForgeRequired)?;
        let base = forge_base_url(forge)?;
        if !github_repository_reference_at(remote.url.as_str(), &base)
            .is_some_and(|identified| identified.eq_ignore_ascii_case(repository))
        {
            return Err(SourceDeliveryFactoryError::GitHubRemoteIdentity);
        }
    }
    Ok(())
}

fn validate_forge_remote(
    remote: Option<&crate::native_v2_contract::ResolvedGitRemote>,
) -> Result<(), SourceDeliveryFactoryError> {
    let remote = remote.ok_or(SourceDeliveryFactoryError::ForgeRequired)?;
    let forge = remote
        .forge
        .as_ref()
        .ok_or(SourceDeliveryFactoryError::ForgeRequired)?;
    let identity = forge_identity_for(
        &remote.url,
        forge.provider.as_str(),
        &forge_base_url(forge)?,
    )?;
    if identity.repository != forge.repository {
        return Err(SourceDeliveryFactoryError::Instance(
            "forge repository does not match its Git remote",
        ));
    }
    Ok(())
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
    for kind in [
        ProviderKind::Git,
        ProviderKind::GitHub,
        ProviderKind::GitLab,
    ] {
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
    let authority = selected_delivery_authority(&request, &selected, &transport)?;
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

fn selected_delivery_authority(
    request: &SourceDeliveryFactoryRequest<'_>,
    selected: &SourceProviderRef,
    transport: &GitTransport,
) -> Result<Option<Arc<dyn SourceDeliveryAuthority>>, SourceDeliveryFactoryError> {
    match selected.id().as_str() {
        GIT_PROVIDER_ID => Ok(Some(Arc::new(GitDeliveryAuthority::new(transport.clone())))),
        GITHUB_PROVIDER_ID => Ok(Some(Arc::new(github_authority(request, transport)?))),
        GITLAB_PROVIDER_ID => {
            let forge = request
                .source
                .remote
                .as_ref()
                .and_then(|r| r.forge.as_ref())
                .ok_or(SourceDeliveryFactoryError::ForgeRequired)?;
            Ok(Some(Arc::new(gitlab::GitLabDeliveryAuthority::new(
                forge_base_url(forge)?,
                Some(transport.clone()),
            )?)))
        }
        _ => Ok(None),
    }
}

fn github_authority(
    request: &SourceDeliveryFactoryRequest<'_>,
    transport: &GitTransport,
) -> Result<GhCliDeliveryAuthority, SourceDeliveryFactoryError> {
    let mut github = GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
        git_identity: request.git_identity,
        git_program: request.git_program.to_owned(),
        gh_program: request.gh_program.to_owned(),
        ..GhCliAuthorityConfig::hosted(request.home_directory.to_owned())
    })
    .with_git_transport(transport.clone());
    if let Some(forge) = request
        .source
        .remote
        .as_ref()
        .and_then(|r| r.forge.as_ref())
    {
        github = github.with_base_url(forge_base_url(forge)?)?;
    }
    if let Some((run_id, store)) = &request.operator_diagnostics {
        github = github.with_operator_diagnostics(run_id.clone(), store.clone());
    }
    Ok(github)
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
    let base = openengine_cluster_protocol::SourceForgeBaseUrl::new("https://github.com").ok()?;
    github_repository_reference_at(value, &base)
}

pub(crate) fn github_repository_reference_at(
    value: &str,
    base: &openengine_cluster_protocol::SourceForgeBaseUrl,
) -> Option<String> {
    if url::Url::parse(base.as_str()).ok()?.path() != "/" {
        return None;
    }
    let normalized = github_ssh_alias(value, base);
    let value = normalized.as_deref().unwrap_or(value);
    let repository = instance::repository_at_remote(value, base)?;
    valid_github_repository(&repository).then_some(repository)
}

fn github_ssh_alias(
    value: &str,
    base: &openengine_cluster_protocol::SourceForgeBaseUrl,
) -> Option<String> {
    if base.as_str() != "https://github.com" {
        return None;
    }
    let mut parsed = url::Url::parse(value).ok()?;
    if parsed.scheme() != "ssh"
        || !parsed.host_str()?.eq_ignore_ascii_case("ssh.github.com")
        || parsed.port() != Some(443)
    {
        return None;
    }
    parsed.set_host(Some("github.com")).ok()?;
    parsed.set_port(None).ok()?;
    Some(parsed.to_string())
}

pub(crate) fn valid_github_repository(value: &str) -> bool {
    let Some((owner, name)) = value.split_once('/') else {
        return false;
    };
    !owner.is_empty()
        && !name.is_empty()
        && !matches!(owner, "." | "..")
        && !matches!(name, "." | "..")
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

/// Captured comparison syntax and repository identity policy belong to the selected adapter.
pub fn validate_comment_review_scope(
    destination: &SourceCommentDestination,
    scope: &serde_json::Value,
) -> Result<(), SourceDeliveryError> {
    if destination.provider.id().as_str() == GITHUB_PROVIDER_ID {
        github::validate_comment_review_scope(destination, scope)
    } else {
        instance::validate_comment_review_scope(destination, scope)
    }
}

/// Identifies a destination against a captured instance rather than a global host default.
pub fn comment_destination_for(
    url: &str,
    forge: &openengine_cluster_protocol::SourceForgeIdentity,
) -> Result<SourceCommentDestination, SourceDeliveryFactoryError> {
    let selected = SourceProviderRef::new(SourceProviderId::new(forge.provider.as_str())?, 1)?;
    let registry = maintained_registry(None, SourceAuthorities::default(), &selected, None)?;
    registry
        .lookup(&selected)?
        .comment_destination_at(url, forge_base_url(forge)?.as_str())
        .ok_or(SourceDeliveryFactoryError::Instance(
            "comment URL does not belong to the admitted forge instance",
        ))
}

/// Preserves a selected installation prefix when the URL belongs to that installation.
pub fn comment_destination_with_context(
    url: &str,
    forge: Option<&openengine_cluster_protocol::SourceForgeIdentity>,
) -> Result<SourceCommentDestination, SourceDeliveryFactoryError> {
    if let Some(forge) = forge
        && let Ok(base) = forge_base_url(forge)
        && url.starts_with(&format!("{base}/"))
    {
        return comment_destination_for(url, forge);
    }
    comment_destination(url)
}

/// Rechecks a complete secret-free destination against its registered adapter's grammar.
pub fn valid_comment_destination(destination: &SourceCommentDestination) -> bool {
    let Some(base) = destination.base_url.as_deref() else {
        return false;
    };
    let Ok(registry) = maintained_registry(
        None,
        SourceAuthorities::default(),
        &destination.provider,
        None,
    ) else {
        return false;
    };
    registry
        .lookup(&destination.provider)
        .is_ok_and(|provider| {
            provider
                .comment_destination_at(&destination.url, base)
                .as_ref()
                == Some(destination)
        })
}

/// Pure adapter-owned resource URL validation, using the existing provider registry.
pub fn valid_comment_permalink(
    destination: &SourceCommentDestination,
    id: &str,
    url: &str,
) -> bool {
    let Ok(registry) = maintained_registry(
        None,
        SourceAuthorities::default(),
        &destination.provider,
        None,
    ) else {
        return false;
    };
    registry
        .lookup(&destination.provider)
        .is_ok_and(|provider| provider.valid_comment_permalink(destination, id, url))
}

/// Constructs only forge authority; source-less comments allocate no Git transport.
pub fn comment_authority(
    config: GhCliAuthorityConfig,
    url: &str,
) -> Result<Arc<dyn SourceCommentAuthority>, SourceDeliveryFactoryError> {
    let destination = comment_destination(url)?;
    comment_authority_for(config, &destination)
}

/// Constructs authority from the frozen admission identity rather than reparsing its web URL.
pub fn comment_authority_for(
    config: GhCliAuthorityConfig,
    destination: &SourceCommentDestination,
) -> Result<Arc<dyn SourceCommentAuthority>, SourceDeliveryFactoryError> {
    if !valid_comment_destination(destination) {
        return Err(SourceDeliveryFactoryError::RemoteIdentity);
    }
    let base = openengine_cluster_protocol::SourceForgeBaseUrl::new(
        destination
            .base_url
            .clone()
            .ok_or(SourceDeliveryFactoryError::RemoteIdentity)?,
    )
    .map_err(|_| SourceDeliveryFactoryError::RemoteIdentity)?;
    let authority: Arc<dyn SourceCommentAuthority> =
        if destination.provider.id().as_str() == GITHUB_PROVIDER_ID {
            Arc::new(GhCliDeliveryAuthority::new(config).with_base_url(base)?)
        } else if destination.provider.id().as_str() == GITLAB_PROVIDER_ID {
            Arc::new(gitlab::GitLabDeliveryAuthority::new(base, None)?)
        } else {
            return Err(SourceDeliveryFactoryError::ForgeRequired);
        };
    let registry = maintained_registry(
        None,
        SourceAuthorities {
            delivery: None,
            comment: Some(authority),
        },
        &destination.provider,
        None,
    )?;
    Ok(registry.comment_authority(destination)?)
}

#[cfg(test)]
mod tests;
