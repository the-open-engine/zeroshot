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
