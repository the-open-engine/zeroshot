use std::sync::Arc;
use async_trait::async_trait;
use crate::native_v2_contract::ResolvedSource;
use crate::source_code_provider::provider::*;
use super::{GIT_PROVIDER_ID, GITHUB_PROVIDER_ID, GITLAB_PROVIDER_ID};

#[derive(Clone, Copy)]
pub(super) enum ProviderKind {
    Git,
    GitHub,
    GitLab,
}
impl ProviderKind {
    pub(super) const fn id(self) -> &'static str {
        match self {
            Self::Git => GIT_PROVIDER_ID,
            Self::GitHub => GITHUB_PROVIDER_ID,
            Self::GitLab => GITLAB_PROVIDER_ID,
        }
    }
    fn capabilities(self) -> Vec<SourceCapability> {
        use SourceCapability::*;
        match self {
            Self::Git => vec![Read, Push],
            Self::GitLab => vec![Read, Comment, Push, PullRequest, Checks, Merge],
            Self::GitHub => vec![
                Read,
                Comment,
                Push,
                PullRequest,
                Checks,
                AutoMerge,
                MergeQueue,
                Merge,
            ],
        }
    }
}

pub(super) struct CoreSourceProvider {
    descriptor: SourceProviderDescriptor,
    source: Option<ResolvedSource>,
    authority: Option<Arc<dyn SourceDeliveryAuthority>>,
    comment_authority: Option<Arc<dyn SourceCommentAuthority>>,
    kind: ProviderKind,
}
impl CoreSourceProvider {
    pub(super) fn new(
        kind: ProviderKind,
        source: Option<ResolvedSource>,
        authority: Option<Arc<dyn SourceDeliveryAuthority>>,
    ) -> Result<Self, SourceContractError> {
        let descriptor = SourceProviderDescriptor::new(
            SourceProviderRef::new(SourceProviderId::new(kind.id())?, 1)?,
            BTreeMap::from([(
                SourceProfileId::new("default")?,
                SourceProfileDescriptor::new(
                    kind.capabilities().into_iter().collect(),
                    Default::default(),
                )?,
            )]),
        )?;
        Ok(Self {
            descriptor,
            source,
            authority,
            comment_authority: None,
            kind,
        })
    }

    pub(super) fn with_comment_authority(
        mut self,
        authority: Option<Arc<dyn SourceCommentAuthority>>,
    ) -> Self {
        self.comment_authority = authority;
        self
    }
}
use std::collections::BTreeMap;
fn unavailable() -> SourceProviderFailure {
    SourceProviderFailure::new(
        SourceProviderFailureCode::InvalidRequest,
        SourceFailureMessage::new("operation requires a configured scoped source capability")
            .expect("static bounded message"),
    )
    .expect("static bounded failure")
}

#[async_trait]
impl SourceCodeProvider for CoreSourceProvider {
    fn descriptor(&self) -> &SourceProviderDescriptor {
        &self.descriptor
    }
    fn comment_destination_from_url(&self, url: &str) -> Option<SourceCommentDestination> {
        match self.kind {
            ProviderKind::Git => None,
            ProviderKind::GitHub => super::github::comment_destination(url),
            ProviderKind::GitLab => super::gitlab::comment_destination(url),
        }
    }
    fn comment_destination_at(&self, url: &str, base: &str) -> Option<SourceCommentDestination> {
        let base = openengine_cluster_protocol::SourceForgeBaseUrl::new(base).ok()?;
        match self.kind {
            ProviderKind::Git => None,
            ProviderKind::GitHub => super::github::comment_destination_at(url, &base),
            ProviderKind::GitLab => super::gitlab::comment_destination_at(url, &base),
        }
    }
    fn valid_comment_permalink(
        &self,
        destination: &SourceCommentDestination,
        id: &str,
        url: &str,
    ) -> bool {
        match self.kind {
            ProviderKind::Git => false,
            ProviderKind::GitHub => super::github::valid_comment_permalink(destination, id, url),
            ProviderKind::GitLab => super::gitlab::valid_comment_permalink(destination, id, url),
        }
    }
    fn comment_authority(&self) -> Option<Arc<dyn SourceCommentAuthority>> {
        self.comment_authority.clone()
    }
    fn default_forge_connection(&self) -> Option<(&'static str, &'static str)> {
        match self.kind {
            ProviderKind::Git => None,
            ProviderKind::GitHub => Some(("github", "GH_TOKEN")),
            ProviderKind::GitLab => Some(("gitlab", "GITLAB_TOKEN")),
        }
    }
    fn repository_from_remote(&self, remote: &str) -> Option<SourceRepositoryId> {
        let base = match self.kind {
            ProviderKind::Git => return None,
            ProviderKind::GitHub => {
                return super::github_repository_reference(remote)
                    .and_then(|repository| SourceRepositoryId::new(repository).ok());
            }
            ProviderKind::GitLab => "https://gitlab.com",
        };
        self.repository_from_remote_at(remote, base)
    }
    fn repository_from_remote_at(&self, remote: &str, base: &str) -> Option<SourceRepositoryId> {
        let base = openengine_cluster_protocol::SourceForgeBaseUrl::new(base).ok()?;
        let repository = match self.kind {
            ProviderKind::Git => None,
            ProviderKind::GitHub => super::github_repository_reference_at(remote, &base),
            ProviderKind::GitLab => super::gitlab::repository_reference(remote, &base),
        }?;
        SourceRepositoryId::new(repository).ok()
    }
    fn delivery_authority(&self) -> Option<Arc<dyn SourceDeliveryAuthority>> {
        self.authority.clone()
    }
    async fn identify_repository(
        &self,
        request: &SourceIdentifyRepositoryRequest,
    ) -> Result<CanonicalRepository, SourceProviderFailure> {
        let source = self.source.as_ref().ok_or_else(unavailable)?;
        let repository = match self.kind {
            ProviderKind::Git => source.repository.as_str(),
            ProviderKind::GitHub | ProviderKind::GitLab => source
                .remote
                .as_ref()
                .and_then(|r| r.forge.as_ref())
                .map_or(source.repository.as_str(), |forge| {
                    forge.repository.as_str()
                }),
        };
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
        let remote = super::source_remote(source);
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
