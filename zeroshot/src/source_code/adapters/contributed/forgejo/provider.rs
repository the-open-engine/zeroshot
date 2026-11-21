//! Metadata-only Forgejo adapter: instance-scoped repository identification and no effects.
use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::native_v2_contract::ResolvedSource;
use crate::source_code_provider::{
    CanonicalRepository, SourceCapability, SourceCodeProvider, SourceContractError,
    SourceDeliveryAuthority, SourceFailureMessage, SourceGitCheckout,
    SourceIdentifyRepositoryRequest, SourceInspectRepositoryRequest,
    SourceMaterializationDestination, SourceMaterializationReceipt, SourceMaterializeRequest,
    SourceOperationInspection, SourceOperationReceipt, SourceOperationRequest,
    SourceProfileDescriptor, SourceProfileId, SourceProviderDescriptor, SourceProviderFailure,
    SourceProviderFailureCode, SourceProviderId, SourceProviderRef, SourceRepositoryId,
    SourceRepositoryInspection, SourceRevisionId, SourceWorkspaceCapability,
};

use crate::source_code_provider::adapters::core::{instance, source_remote};

use super::{FORGEJO_CONNECTION, FORGEJO_PROVIDER_ID, FORGEJO_TOKEN_FIELD};

pub(super) struct ForgejoSourceProvider {
    descriptor: SourceProviderDescriptor,
    source: Option<ResolvedSource>,
    authority: Option<Arc<dyn SourceDeliveryAuthority>>,
}

impl ForgejoSourceProvider {
    pub(super) fn new(source: Option<&ResolvedSource>) -> Result<Self, SourceContractError> {
        let capabilities = [
            SourceCapability::Read,
            SourceCapability::Push,
            SourceCapability::PullRequest,
            SourceCapability::Checks,
            SourceCapability::Merge,
        ]
        .into_iter()
        .collect();
        let descriptor = SourceProviderDescriptor::new(
            SourceProviderRef::new(SourceProviderId::new(FORGEJO_PROVIDER_ID)?, 1)?,
            BTreeMap::from([(
                SourceProfileId::new("default")?,
                SourceProfileDescriptor::new(capabilities, Default::default())?,
            )]),
        )?;
        let stored = source
            .filter(|source| {
                source
                    .remote
                    .as_ref()
                    .and_then(|remote| remote.forge.as_ref())
                    .is_some_and(|forge| forge.provider.as_str() == FORGEJO_PROVIDER_ID)
            })
            .cloned();
        Ok(Self {
            descriptor,
            source: stored,
            authority: None,
        })
    }

    /// The delivery authority exists only for an admitted instance with an admitted transport.
    /// Registration itself performs no effect: it stores what the engine supplied.
    pub(super) fn with_authority(
        mut self,
        authority: Option<Arc<dyn SourceDeliveryAuthority>>,
    ) -> Self {
        self.authority = authority;
        self
    }
}

fn unavailable() -> SourceProviderFailure {
    SourceProviderFailure::new(
        SourceProviderFailureCode::InvalidRequest,
        SourceFailureMessage::new("operation requires a configured scoped source capability")
            .expect("static bounded message"),
    )
    .expect("static bounded failure")
}

#[async_trait]
impl SourceCodeProvider for ForgejoSourceProvider {
    fn descriptor(&self) -> &SourceProviderDescriptor {
        &self.descriptor
    }

    fn repository_from_remote_at(&self, remote: &str, base: &str) -> Option<SourceRepositoryId> {
        let base = openengine_cluster_protocol::SourceForgeBaseUrl::new(base).ok()?;
        let repository = instance::repository_at_remote(remote, &base)?;
        if repository.split('/').count() != 2 {
            return None;
        }
        SourceRepositoryId::new(repository).ok()
    }

    fn default_forge_connection(&self) -> Option<(&'static str, &'static str)> {
        Some((FORGEJO_CONNECTION, FORGEJO_TOKEN_FIELD))
    }

    fn delivery_authority(&self) -> Option<Arc<dyn SourceDeliveryAuthority>> {
        self.authority.clone()
    }

    async fn identify_repository(
        &self,
        request: &SourceIdentifyRepositoryRequest,
    ) -> Result<CanonicalRepository, SourceProviderFailure> {
        let source = self.source.as_ref().ok_or_else(unavailable)?;
        let repository = source
            .remote
            .as_ref()
            .and_then(|remote| remote.forge.as_ref())
            .map_or(source.repository.as_str(), |forge| {
                forge.repository.as_str()
            });
        CanonicalRepository::new(
            request.provider().clone(),
            request.profile().clone(),
            request.account().clone(),
            SourceRepositoryId::new(repository).map_err(|_| unavailable())?,
        )
        .map_err(|_| unavailable())
    }

    async fn inspect_repository(
        &self,
        request: &SourceInspectRepositoryRequest,
    ) -> Result<SourceRepositoryInspection, SourceProviderFailure> {
        let source = self.source.as_ref().ok_or_else(unavailable)?;
        SourceRepositoryInspection::new(
            request.repository().clone(),
            SourceRevisionId::new(source.revision.as_str()).map_err(|_| unavailable())?,
            Vec::new(),
        )
        .map_err(|_| unavailable())
    }

    async fn materialize(
        &self,
        request: &SourceMaterializeRequest,
        destination: SourceMaterializationDestination<'_>,
    ) -> Result<SourceMaterializationReceipt, SourceProviderFailure> {
        let source = self.source.as_ref().ok_or_else(unavailable)?;
        let remote = source_remote(source);
        let digest = destination
            .checkout_git(SourceGitCheckout {
                remote: &remote,
                branch: source.branch.as_str(),
                revision: request.revision(),
            })
            .await
            .map_err(|_| unavailable())?;
        SourceMaterializationReceipt::new(
            request.repository().clone(),
            request.revision().clone(),
            digest,
        )
        .map_err(|_| unavailable())
    }

    async fn inspect_operation(
        &self,
        _request: &SourceOperationRequest,
    ) -> Result<SourceOperationInspection, SourceProviderFailure> {
        Err(unavailable())
    }

    async fn operate(
        &self,
        _request: &SourceOperationRequest,
        _workspace: SourceWorkspaceCapability<'_>,
    ) -> Result<SourceOperationReceipt, SourceProviderFailure> {
        Err(unavailable())
    }
}
