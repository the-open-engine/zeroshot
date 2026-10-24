use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};

use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

struct ScriptedSourceProvider {
    descriptor: SourceProviderDescriptor,
    substitute_evidence: AtomicBool,
}

impl ScriptedSourceProvider {
    fn new() -> Self {
        let provider = provider_ref(1);
        let profile = profile_id("default");
        let read = SourceProfileDescriptor::new(
            BTreeSet::from([SourceCapability::Read, SourceCapability::Branch]),
            BTreeSet::new(),
        )
        .assert_value();
        Self {
            descriptor: SourceProviderDescriptor::new(provider, BTreeMap::from([(profile, read)]))
                .assert_value(),
            substitute_evidence: AtomicBool::new(false),
        }
    }

    fn substitute_evidence(&self, value: bool) {
        self.substitute_evidence.store(value, Ordering::SeqCst);
    }

    fn repository(&self, repository: &CanonicalRepository) -> CanonicalRepository {
        CanonicalRepository::new(
            repository.provider().clone(),
            repository.profile().clone(),
            repository.account().clone(),
            SourceRepositoryId::new(if self.substitute_evidence.load(Ordering::SeqCst) {
                "substituted/repository"
            } else {
                repository.repository().as_str()
            })
            .assert_value(),
        )
        .assert_value()
    }
}

#[async_trait]
impl SourceCodeProvider for ScriptedSourceProvider {
    fn descriptor(&self) -> &SourceProviderDescriptor {
        &self.descriptor
    }

    fn comment_destination_from_url(&self, url: &str) -> Option<SourceCommentDestination> {
        (url == "https://comments.fixture.invalid/project/issues/17").then(|| {
            SourceCommentDestination {
                provider: self.descriptor.provider().clone(),
                repository: "team/project".into(),
                discussion_id: "17".into(),
                kind: SourceDiscussionKind::Issue,
                url: url.to_owned(),
            }
        })
    }

    fn repository_from_remote(&self, remote: &str) -> Option<SourceRepositoryId> {
        (remote == "https://registry-matcher.invalid/nested/team/repo.git").then(|| {
            SourceRepositoryId::new(if self.substitute_evidence.load(Ordering::SeqCst) {
                "r".repeat(256)
            } else {
                "tenant:17/nested/team/repository".to_owned()
            })
            .assert_value()
        })
    }

    async fn identify_repository(
        &self,
        request: &SourceIdentifyRepositoryRequest,
    ) -> Result<CanonicalRepository, SourceProviderFailure> {
        CanonicalRepository::new(
            request.provider().clone(),
            request.profile().clone(),
            if self.substitute_evidence.load(Ordering::SeqCst) {
                SourceAccountId::new("substituted-account").assert_value()
            } else {
                request.account().clone()
            },
            SourceRepositoryId::new(request.reference().as_str()).assert_value(),
        )
        .map_err(|error| provider_failure(error.to_string()))
    }

    async fn inspect_repository(
        &self,
        request: &SourceInspectRepositoryRequest,
    ) -> Result<SourceRepositoryInspection, SourceProviderFailure> {
        SourceRepositoryInspection::new(
            self.repository(request.repository()),
            SourceRevisionId::new("main-head").assert_value(),
            vec![SourcePublicUrl::new("https://github.com/acme/project").assert_value()],
        )
        .map_err(|error| provider_failure(error.to_string()))
    }

    async fn materialize(
        &self,
        request: &SourceMaterializeRequest,
        destination: SourceMaterializationDestination<'_>,
    ) -> Result<SourceMaterializationReceipt, SourceProviderFailure> {
        destination
            .write_file("README.md", b"materialized")
            .map_err(|error| provider_failure(error.to_string()))?;
        SourceMaterializationReceipt::new(
            self.repository(request.repository()),
            if self.substitute_evidence.load(Ordering::SeqCst) {
                SourceRevisionId::new("substituted-revision").assert_value()
            } else {
                request.revision().clone()
            },
            SourceContentDigest::new("a".repeat(64)).assert_value(),
        )
        .map_err(|error| provider_failure(error.to_string()))
    }

    async fn inspect_operation(
        &self,
        _request: &SourceOperationRequest,
    ) -> Result<SourceOperationInspection, SourceProviderFailure> {
        Ok(SourceOperationInspection::Unobserved)
    }

    async fn operate(
        &self,
        _request: &SourceOperationRequest,
        _workspace: SourceWorkspaceCapability<'_>,
    ) -> Result<SourceOperationReceipt, SourceProviderFailure> {
        Err(provider_failure("operate is not used by this fixture"))
    }
}

#[test]
fn comment_routing_rejects_unsupported_missing_authority_and_ambiguous_providers() {
    let url = "https://comments.fixture.invalid/project/issues/17";
    let mut registry = SourceCodeProviderRegistry::new();
    assert!(matches!(
        registry.comment_destination(url),
        Err(SourceRegistryError::UnsupportedCommentUrl)
    ));
    registry
        .register(Arc::new(ScriptedSourceProvider::new()))
        .assert_value();
    assert!(matches!(
        registry.comment_destination(url),
        Err(SourceRegistryError::UnsupportedCapability {
            capability: SourceCapability::Comment,
            ..
        })
    ));
    let mut registry = SourceCodeProviderRegistry::new();
    for version in [1, 2] {
        let mut provider = ScriptedSourceProvider::new();
        provider.descriptor = SourceProviderDescriptor::new(
            provider_ref(version),
            BTreeMap::from([(
                profile_id("default"),
                SourceProfileDescriptor::new(
                    BTreeSet::from([SourceCapability::Comment]),
                    BTreeSet::new(),
                )
                .assert_value(),
            )]),
        )
        .assert_value();
        registry.register(Arc::new(provider)).assert_value();
        if version == 1 {
            let destination = registry.comment_destination(url).assert_value();
            assert_eq!(destination.provider, provider_ref(1));
            assert!(matches!(
                registry.comment_authority(&destination),
                Err(SourceRegistryError::UnavailableDelivery { .. })
            ));
        }
    }
    assert!(matches!(
        registry.comment_destination(url),
        Err(SourceRegistryError::AmbiguousCommentUrl)
    ));
}

fn provider_failure(message: impl Into<String>) -> SourceProviderFailure {
    SourceProviderFailure::new(
        SourceProviderFailureCode::InvalidRequest,
        SourceFailureMessage::new(message).assert_value(),
    )
    .assert_value()
}

fn provider_ref(version: u32) -> SourceProviderRef {
    SourceProviderRef::new(
        SourceProviderId::new("source.fixture").assert_value(),
        version,
    )
    .assert_value()
}

fn profile_id(value: &str) -> SourceProfileId {
    SourceProfileId::new(value).assert_value()
}

fn canonical_repository() -> CanonicalRepository {
    CanonicalRepository::new(
        provider_ref(1),
        profile_id("default"),
        SourceAccountId::new("account").assert_value(),
        SourceRepositoryId::new("acme/project").assert_value(),
    )
    .assert_value()
}

fn identify_request() -> SourceIdentifyRepositoryRequest {
    SourceIdentifyRepositoryRequest::new(
        provider_ref(1),
        profile_id("default"),
        (
            SourceAccountId::new("account").assert_value(),
            SourceCredentialHandleId::new("credential").assert_value(),
        ),
        SourceRepositoryReference::new("acme/project").assert_value(),
    )
    .assert_value()
}

#[test]
fn hosting_source_contract_registry_selects_exact_version_profile_and_capability() {
    let mut registry = SourceCodeProviderRegistry::new();
    assert!(matches!(
        registry.lookup(&provider_ref(1)),
        Err(SourceRegistryError::UnknownProvider { .. })
    ));
    assert!(matches!(
        registry.descriptor(&provider_ref(1)),
        Err(SourceRegistryError::UnknownProvider { .. })
    ));

    let provider = Arc::new(ScriptedSourceProvider::new());
    registry.register(provider.clone()).assert_value();
    assert_eq!(
        registry
            .lookup(&provider_ref(1))
            .assert_value()
            .descriptor(),
        provider.descriptor()
    );
    assert!(matches!(
        registry.register(provider),
        Err(SourceRegistryError::DuplicateRegistration { .. })
    ));
    assert!(matches!(
        registry.descriptor(&provider_ref(2)),
        Err(SourceRegistryError::UnavailableVersion { .. })
    ));
    assert!(matches!(
        registry.capability(
            &provider_ref(1),
            &profile_id("missing"),
            SourceCapability::Read
        ),
        Err(SourceRegistryError::UnavailableProfile { .. })
    ));
    assert!(matches!(
        registry.capability(
            &provider_ref(1),
            &profile_id("default"),
            SourceCapability::Merge,
        ),
        Err(SourceRegistryError::UnsupportedCapability { .. })
    ));
    assert!(
        registry
            .capability(
                &provider_ref(1),
                &profile_id("default"),
                SourceCapability::Read,
            )
            .assert_value()
            .supports(SourceCapability::Branch)
    );
}

#[tokio::test]
async fn hosting_source_contract_registry_rejects_substituted_repository_evidence() {
    let provider = Arc::new(ScriptedSourceProvider::new());
    let mut registry = SourceCodeProviderRegistry::new();
    registry.register(provider.clone()).assert_value();
    let identify = identify_request();
    assert_eq!(identify.credential_handle().as_str(), "credential");
    let repository = registry.identify_repository(&identify).await.assert_value();
    assert_eq!(repository, canonical_repository());

    let inspect = SourceInspectRepositoryRequest::new(
        repository.clone(),
        SourceCredentialHandleId::new("credential").assert_value(),
    )
    .assert_value();
    assert_eq!(inspect.credential_handle().as_str(), "credential");
    let inspection = registry.inspect_repository(&inspect).await.assert_value();
    assert_eq!(inspection.repository(), &repository);
    assert_eq!(inspection.default_revision().as_str(), "main-head");
    assert_eq!(
        inspection.public_urls()[0].as_str(),
        "https://github.com/acme/project"
    );

    let materialize = SourceMaterializeRequest::new(
        repository.clone(),
        SourceCredentialHandleId::new("credential").assert_value(),
        SourceRevisionId::new("revision").assert_value(),
    )
    .assert_value();
    assert_eq!(materialize.credential_handle().as_str(), "credential");
    let harness = unsafe { SourceMaterializationContractHarness::new() };
    assert!(harness.destination().is_available());
    let receipt = registry
        .materialize(&materialize, harness.destination())
        .await
        .assert_value();
    assert_eq!(harness.write_count(), 1);
    assert_eq!(receipt.repository(), &repository);
    assert_eq!(receipt.revision().as_str(), "revision");
    assert_eq!(receipt.content_digest().as_str(), "a".repeat(64));

    let operation = SourceOperationRequest::new(
        repository.clone(),
        SourceCredentialHandleId::new("credential").assert_value(),
        (
            SourceWorkspaceId::new("b".repeat(64)).assert_value(),
            SourceOperationId::new("branch-operation").assert_value(),
        ),
        SourceOperation::Branch {
            expected_parent: SourceRevisionId::new("revision").assert_value(),
            branch: SourceBranchId::new("feature").assert_value(),
            pre_effect: SourceStateDigest::new("c".repeat(64)).assert_value(),
        },
    )
    .assert_value();
    assert_eq!(
        registry.inspect_operation(&operation).await.assert_value(),
        SourceOperationInspection::Unobserved
    );
    let mut handle = String::from("verified workspace");
    let mut workspace = unsafe {
        SourceWorkspaceCapability::from_verified_contract_test(
            operation.workspace().clone(),
            &mut handle,
        )
    };
    assert_eq!(workspace.workspace(), operation.workspace());
    assert_eq!(
        workspace
            .downcast_mut::<String>()
            .map(|value| value.as_str()),
        Some("verified workspace")
    );
    assert!(workspace.downcast_mut::<u64>().is_none());

    provider.substitute_evidence(true);
    assert!(matches!(
        registry.identify_repository(&identify).await,
        Err(SourceCallError::InvalidEvidence { .. })
    ));
    assert!(matches!(
        registry.inspect_repository(&inspect).await,
        Err(SourceCallError::InvalidEvidence { .. })
    ));
    assert!(matches!(
        registry
            .materialize(&materialize, harness.destination())
            .await,
        Err(SourceCallError::InvalidEvidence { .. })
    ));
}

#[test]
fn remote_identification_is_optional_opaque_and_uses_the_registered_provider() {
    let remote = "https://registry-matcher.invalid/nested/team/repo.git";
    let mut registry = SourceCodeProviderRegistry::new();
    assert_eq!(registry.repository_from_remote(remote).assert_value(), None);
    registry
        .register(Arc::new(ScriptedSourceProvider::new()))
        .assert_value();
    let identity = registry
        .repository_from_remote(remote)
        .assert_value()
        .assert_value();
    assert_eq!(identity.0, provider_ref(1));
    assert_eq!(identity.1.as_str(), "tenant:17/nested/team/repository");
    let unrelated = "https://another-forge.invalid/team/repo.git";
    assert_eq!(
        registry.repository_from_remote(unrelated).assert_value(),
        None
    );
    let forge = crate::source_code_provider::adapters::core::forge_identity_with_registry(
        remote, &registry,
    )
    .assert_value()
    .assert_value();
    assert_eq!(forge.provider.as_str(), "source.fixture");
    assert_eq!(forge.repository.as_str(), identity.1.as_str());
}

#[test]
fn multiple_provider_versions_cannot_ambiguously_claim_a_remote() {
    let mut registry = SourceCodeProviderRegistry::new();
    registry
        .register(Arc::new(ScriptedSourceProvider::new()))
        .assert_value();
    let mut second = ScriptedSourceProvider::new();
    second.descriptor =
        SourceProviderDescriptor::new(provider_ref(2), second.descriptor.profiles().clone())
            .assert_value();
    registry.register(Arc::new(second)).assert_value();
    let remote = "https://registry-matcher.invalid/nested/team/repo.git";
    assert_eq!(
        registry.repository_from_remote(remote),
        Err(SourceRegistryError::AmbiguousRemote)
    );
}

#[test]
fn run_source_bridge_rejects_future_versions_and_oversized_repository_identity() {
    use crate::source_code_provider::adapters::core::{
        forge_identity_with_registry, SourceDeliveryFactoryError,
    };
    let remote = "https://registry-matcher.invalid/nested/team/repo.git";
    let mut registry = SourceCodeProviderRegistry::new();
    let mut future = ScriptedSourceProvider::new();
    future.descriptor =
        SourceProviderDescriptor::new(provider_ref(2), future.descriptor.profiles().clone())
            .assert_value();
    registry.register(Arc::new(future)).assert_value();
    assert!(matches!(
        forge_identity_with_registry(remote, &registry),
        Err(SourceDeliveryFactoryError::UnsupportedRemoteVersion { .. })
    ));
    let mut registry = SourceCodeProviderRegistry::new();
    let provider = Arc::new(ScriptedSourceProvider::new());
    provider.substitute_evidence(true);
    registry.register(provider).assert_value();
    assert!(matches!(
        forge_identity_with_registry(remote, &registry),
        Err(SourceDeliveryFactoryError::RemoteIdentity)
    ));
}
