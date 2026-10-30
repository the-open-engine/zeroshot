use super::*;
use openengine_cluster_testkit::assertions::AssertValue;
use std::collections::{BTreeMap, BTreeSet};

struct ProviderWithoutForgeHooks {
    descriptor: SourceProviderDescriptor,
}

impl ProviderWithoutForgeHooks {
    fn new() -> Self {
        let provider = SourceProviderRef::new(
            SourceProviderId::new("source.no-forge-hooks").assert_value(),
            1,
        )
        .assert_value();
        let profile = SourceProfileDescriptor::new(
            BTreeSet::from([
                SourceCapability::Read,
                SourceCapability::Comment,
                SourceCapability::PullRequest,
            ]),
            BTreeSet::new(),
        )
        .assert_value();
        Self {
            descriptor: SourceProviderDescriptor::new(
                provider,
                BTreeMap::from([(SourceProfileId::new("default").assert_value(), profile)]),
            )
            .assert_value(),
        }
    }
}

#[async_trait]
impl SourceCodeProvider for ProviderWithoutForgeHooks {
    fn descriptor(&self) -> &SourceProviderDescriptor {
        &self.descriptor
    }
    async fn identify_repository(
        &self,
        _request: &SourceIdentifyRepositoryRequest,
    ) -> Result<CanonicalRepository, SourceProviderFailure> {
        panic!("metadata selection must not invoke provider effects")
    }
    async fn inspect_repository(
        &self,
        _request: &SourceInspectRepositoryRequest,
    ) -> Result<SourceRepositoryInspection, SourceProviderFailure> {
        panic!("metadata selection must not invoke provider effects")
    }
    async fn materialize(
        &self,
        _request: &SourceMaterializeRequest,
        _destination: SourceMaterializationDestination<'_>,
    ) -> Result<SourceMaterializationReceipt, SourceProviderFailure> {
        panic!("metadata selection must not invoke provider effects")
    }
    async fn inspect_operation(
        &self,
        _request: &SourceOperationRequest,
    ) -> Result<SourceOperationInspection, SourceProviderFailure> {
        panic!("metadata selection must not invoke provider effects")
    }
    async fn operate(
        &self,
        _request: &SourceOperationRequest,
        _workspace: SourceWorkspaceCapability<'_>,
    ) -> Result<SourceOperationReceipt, SourceProviderFailure> {
        panic!("metadata selection must not invoke provider effects")
    }
}

#[test]
fn advertised_capabilities_do_not_infer_optional_forge_authority_or_github_identity() {
    let provider = Arc::new(ProviderWithoutForgeHooks::new());
    let mut registry = SourceCodeProviderRegistry::new();
    registry.register(provider.clone()).assert_value();
    let base = "https://github.com";
    let url = "https://github.com/team/project/issues/7";
    let remote = "https://github.com/team/project.git";
    assert_eq!(
        registry.comment_destination(url),
        Err(SourceRegistryError::UnsupportedCommentUrl)
    );
    assert_eq!(registry.repository_from_remote(remote).assert_value(), None);
    assert_eq!(provider.comment_destination_at(url, base), None);
    assert_eq!(provider.repository_from_remote_at(remote, base), None);
    assert_eq!(provider.default_forge_connection(), None);
    let destination = SourceCommentDestination {
        base_url: Some(base.into()),
        provider: provider.descriptor().provider().clone(),
        repository: "team/project".into(),
        discussion_id: "7".into(),
        kind: SourceDiscussionKind::Issue,
        url: url.into(),
    };
    assert!(!provider.valid_comment_permalink(
        &destination,
        "123",
        &format!("{url}#issuecomment-123")
    ));
    assert!(matches!(
        registry.comment_authority(&destination),
        Err(SourceRegistryError::UnavailableDelivery { .. })
    ));
    assert!(matches!(
        registry.delivery_authority(
            provider.descriptor().provider(),
            &SourceProfileId::new("default").assert_value(),
            &[SourceCapability::PullRequest]
        ),
        Err(SourceRegistryError::UnavailableDelivery { .. })
    ));
}
