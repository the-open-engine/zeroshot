//! Discovery acceptance uses the production registration hook and scoped shared transport.
use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::GitRemoteUrl;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::SourceAdapterContext;
use crate::source_code_provider::*;
use crate::source_code_provider::adapters::core::GitDeliveryAuthority;

pub(crate) const PROVIDER: &str = "source.contribution-discovery";
pub(crate) const REPOSITORY: &str = "tenant:fixture/projects/team/nested/project";
pub(crate) const HTTPS_REMOTE: &str =
    "https://contribution-discovery.invalid/team/nested/project.git";
pub(crate) const SSH_REMOTE: &str = "git@contribution-discovery.invalid:team/nested/project.git";
pub(crate) const AMBIGUOUS_REMOTE: &str = "https://contribution-ambiguity.invalid/team/project.git";
const DISCOVERY_REMOTES: &[&str] = &[HTTPS_REMOTE, SSH_REMOTE, AMBIGUOUS_REMOTE];
const AMBIGUITY_REMOTES: &[&str] = &[AMBIGUOUS_REMOTE];

pub(crate) fn assert_source(source: &crate::native_v2_contract::ResolvedSource, remote: &str) {
    assert_eq!(source.repository.as_str(), REPOSITORY);
    assert_eq!(source.branch.as_str(), "main");
    let metadata = source.remote.as_ref().assert_value();
    assert_eq!(metadata.url.as_str(), remote);
    assert!(metadata.connection.is_none());
    let forge = metadata.forge.as_ref().assert_value();
    assert_eq!(forge.provider.as_str(), PROVIDER);
    assert_eq!(forge.repository.as_str(), REPOSITORY);
}

struct DiscoveryContribution {
    descriptor: SourceProviderDescriptor,
    remote_urls: &'static [&'static str],
    authority: Option<Arc<dyn SourceDeliveryAuthority>>,
}

pub(super) fn register(
    registry: &mut SourceCodeProviderRegistry,
    context: SourceAdapterContext<'_>,
) -> Result<(), SourceRegistryError> {
    let capabilities = [
        SourceCapability::Read,
        SourceCapability::Push,
        SourceCapability::PullRequest,
        SourceCapability::Checks,
        SourceCapability::Merge,
    ]
    .into_iter()
    .collect();
    let profile = SourceProfileDescriptor::new(capabilities, Default::default()).assert_value();
    for (provider, remote_urls) in [
        (PROVIDER, DISCOVERY_REMOTES),
        ("source.contribution-ambiguity", AMBIGUITY_REMOTES),
    ] {
        let descriptor = SourceProviderDescriptor::new(
            SourceProviderRef::new(SourceProviderId::new(provider).assert_value(), 1)
                .assert_value(),
            BTreeMap::from([(
                SourceProfileId::new("default").assert_value(),
                profile.clone(),
            )]),
        )
        .assert_value();
        let configured = context
            .source
            .and_then(|source| source.remote.as_ref())
            .and_then(|remote| remote.forge.as_ref())
            .is_some_and(|forge| {
                forge.provider.as_str() == provider && forge.repository.as_str() == REPOSITORY
            });
        let authority = context.transport.filter(|_| configured).map(|transport| {
            Arc::new(GitDeliveryAuthority::new(transport.clone()))
                as Arc<dyn SourceDeliveryAuthority>
        });
        registry.register(Arc::new(DiscoveryContribution {
            descriptor,
            remote_urls,
            authority,
        }))?;
    }
    Ok(())
}

fn discovery_unavailable<T>() -> Result<T, SourceProviderFailure> {
    Err(SourceProviderFailure::new(
        SourceProviderFailureCode::InvalidRequest,
        SourceFailureMessage::new("discovery fixture does not expose atomic operations")
            .assert_value(),
    )
    .assert_value())
}

#[async_trait]
impl SourceCodeProvider for DiscoveryContribution {
    fn descriptor(&self) -> &SourceProviderDescriptor {
        &self.descriptor
    }

    fn repository_from_remote(&self, remote: &str) -> Option<SourceRepositoryId> {
        self.remote_urls
            .contains(&remote)
            .then(|| SourceRepositoryId::new(REPOSITORY).assert_value())
    }

    fn delivery_authority(&self) -> Option<Arc<dyn SourceDeliveryAuthority>> {
        self.authority.clone()
    }

    async fn identify_repository(
        &self,
        _request: &SourceIdentifyRepositoryRequest,
    ) -> Result<CanonicalRepository, SourceProviderFailure> {
        discovery_unavailable()
    }

    async fn inspect_repository(
        &self,
        _request: &SourceInspectRepositoryRequest,
    ) -> Result<SourceRepositoryInspection, SourceProviderFailure> {
        discovery_unavailable()
    }

    async fn materialize(
        &self,
        _request: &SourceMaterializeRequest,
        _destination: SourceMaterializationDestination<'_>,
    ) -> Result<SourceMaterializationReceipt, SourceProviderFailure> {
        discovery_unavailable()
    }

    async fn inspect_operation(
        &self,
        _request: &SourceOperationRequest,
    ) -> Result<SourceOperationInspection, SourceProviderFailure> {
        discovery_unavailable()
    }

    async fn operate(
        &self,
        _request: &SourceOperationRequest,
        _workspace: SourceWorkspaceCapability<'_>,
    ) -> Result<SourceOperationReceipt, SourceProviderFailure> {
        discovery_unavailable()
    }
}

#[test]
fn ordinary_registration_discovers_contributions_without_source_or_transport() {
    let mut registry = SourceCodeProviderRegistry::new();
    super::register(
        &mut registry,
        SourceAdapterContext {
            source: None,
            transport: None,
        },
    )
    .assert_value();
    for url in [HTTPS_REMOTE, SSH_REMOTE] {
        let remote = GitRemoteUrl::new(url).assert_value();
        let (provider, repository) = registry
            .repository_from_remote(remote.as_str())
            .assert_value()
            .assert_value();
        assert_eq!(provider.id().as_str(), PROVIDER);
        assert_eq!(provider.version(), 1);
        assert_eq!(repository.as_str(), REPOSITORY);
        assert!(
            registry
                .lookup(&provider)
                .assert_value()
                .delivery_authority()
                .is_none()
        );
        let forge = crate::source_code_provider::adapters::core::forge_identity(&remote)
            .assert_value()
            .assert_value();
        assert_eq!(forge.provider.as_str(), PROVIDER);
        assert_eq!(forge.repository.as_str(), REPOSITORY);
    }
    assert!(
        registry
            .repository_from_remote("https://forge.invalid/team/nested/project.git")
            .assert_value()
            .is_none()
    );
    assert_eq!(
        registry
            .repository_from_remote(AMBIGUOUS_REMOTE)
            .assert_error(),
        SourceRegistryError::AmbiguousRemote
    );
}

#[cfg(unix)]
#[tokio::test]
async fn ordinary_runtime_factory_binds_the_contributed_context_transport() {
    use std::path::Path;
    use crate::native_v2_contract::ResolvedSource;
    use crate::source_code_provider::adapters::core::{
        SourceDeliveryFactoryRequest, delivery_authority,
    };
    let root = tempfile::tempdir().assert_value();
    let program = root.path().join("bound-git");
    let revision = "b".repeat(40);
    let script = format!(
        concat!(
            "#!/bin/sh\nfor argument in \"$@\"; do\n",
            "  if test \"$argument\" = '{remote}'; then\n",
            "    printf '{revision}\\trefs/heads/main\\n'\n",
            "    exit 0\n  fi\ndone\nexit 93\n"
        ),
        remote = HTTPS_REMOTE,
        revision = revision,
    );
    openengine_cluster_testkit::fixture::write_executable(&program, &script, 0o700).assert_value();
    let remote = GitRemoteUrl::new(HTTPS_REMOTE).assert_value();
    let source = ResolvedSource {
        repository: openengine_cluster_protocol::SourceRepositoryId::new(REPOSITORY).assert_value(),
        branch: openengine_cluster_protocol::SourceBranchId::new("main").assert_value(),
        revision: openengine_cluster_protocol::SourceRevisionId::new("a".repeat(40)).assert_value(),
        remote: Some(openengine_cluster_protocol::ResolvedGitRemote {
            forge: crate::source_code_provider::adapters::core::forge_identity(&remote)
                .assert_value(),
            url: remote,
            connection: None,
        }),
    };
    let authority = delivery_authority(SourceDeliveryFactoryRequest {
        source: &source,
        workspace: root.path(),
        git_program: &program,
        gh_program: Path::new("/must-not-start-gh"),
        home_directory: root.path(),
        private_directory: root.path(),
        git_identity: None,
        local_environment: None,
        required_capabilities: &[SourceCapability::PullRequest, SourceCapability::Checks],
        operator_diagnostics: None,
    })
    .assert_value();
    let target = DeliveryTarget::new(REPOSITORY, "main", source.revision.as_str()).assert_value();
    let observed = authority
        .observe_delivery(
            SourceDeliveryRead {
                target: &target,
                head_branch: "main",
                known_review: None,
                include_review: false,
            },
            SourceDeliveryCredential::token(""),
        )
        .await
        .assert_value();
    assert_eq!(observed.head_revision.as_deref(), Some(revision.as_str()));
    assert!(observed.review.is_none());
}
