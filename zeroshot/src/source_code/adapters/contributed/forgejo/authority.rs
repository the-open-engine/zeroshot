//! Delivery authority for one admitted Forgejo instance. It owns the bounded client, the instance
//! identity that links are built from, and the admitted Git transport that performs every effect.
//! Reads keep the provider's own identity rules; writes are delegated to the shared transport, so a
//! Forgejo adapter never runs Git itself. The hand-written `Debug` shows no credential.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use openengine_cluster_protocol::SourceForgeBaseUrl;

use crate::native_v2_delivery::transport::GitTransport;
use crate::source_code_provider::adapters::core::network::ForgeNetwork;
use crate::source_code_provider::{
    valid_git_revision, SourceConflictOutcome, SourceConflictRequest, SourceDeliveryAuthority,
    SourceDeliveryCredential, SourceDeliveryError, SourceDeliveryRead, SourceDeliverySnapshot,
    SourceHeadReconciliation, SourceMergeRequestOutcome, SourceMergeWithdrawal, SourcePushRequest,
    SourceReconciliationOutcome, SourceReviewCheckpoint, SourceReviewLinks,
    SourceReviewObservation, SourceReviewReceipt, SourceReviewRequest, SourceTargetIntegration,
    SourceTargetReconciliation,
};

use super::access;
use super::api::{Api, ApiRequest};
use super::merge;
use super::review;
use super::wire;
use super::FORGEJO_TOKEN_FIELD;

const TRANSPORT_ERROR: &str = "Forgejo Git delivery requires the admitted Git transport";

#[derive(Clone)]
pub(crate) struct ForgejoDeliveryAuthority {
    pub(super) base_url: SourceForgeBaseUrl,
    pub(super) api: Api,
    pub(super) transport: Option<GitTransport>,
    /// At-most-once review creation: a key records the first attempt and is removed only after
    /// that attempt is confirmed.
    pub(super) effects: Arc<Mutex<BTreeSet<String>>>,
}

impl std::fmt::Debug for ForgejoDeliveryAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ForgejoDeliveryAuthority")
            .field("base_url", &self.base_url)
            .field("api", &self.api)
            .field(
                "transport",
                &self.transport.as_ref().map(|_| "GitTransport"),
            )
            .finish()
    }
}

impl ForgejoDeliveryAuthority {
    pub(crate) fn new(
        base_url: SourceForgeBaseUrl,
        transport: Option<GitTransport>,
    ) -> Result<Self, SourceDeliveryError> {
        Ok(Self {
            api: Api::new(base_url.as_str())?,
            base_url,
            transport,
            effects: Arc::new(Mutex::new(BTreeSet::new())),
        })
    }

    pub(crate) fn with_network(self, network: ForgeNetwork) -> Result<Self, SourceDeliveryError> {
        Ok(Self {
            api: self.api.with_network(network)?,
            ..self
        })
    }

    #[cfg(test)]
    pub(super) fn for_test(base_url: SourceForgeBaseUrl, api: Api) -> Self {
        Self {
            base_url,
            api,
            transport: None,
            effects: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    pub(super) fn git(&self) -> Result<&GitTransport, SourceDeliveryError> {
        self.transport
            .as_ref()
            .ok_or_else(|| SourceDeliveryError::identity(TRANSPORT_ERROR))
    }

    pub(super) async fn repository(
        &self,
        repository: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<wire::Repository, SourceDeliveryError> {
        let (owner, name) = split_repository(repository)?;
        self.api.require_supported(credential).await?;
        let observed = self
            .api
            .optional_json::<wire::Repository>(ApiRequest::get(&["repos", owner, name]), credential)
            .await?
            .ok_or_else(|| {
                SourceDeliveryError::identity(format!(
                    "Forgejo repository read returned no repository for {}",
                    wire::repository_identity(repository)
                ))
            })?;
        if observed.full_name != repository {
            return Err(SourceDeliveryError::identity(format!(
                "Forgejo repository read returned {} instead of {}",
                wire::repository_identity(&observed.full_name),
                wire::repository_identity(repository)
            )));
        }
        let expected = wire::repository_link(&self.base_url, repository);
        if observed.html_url != expected {
            return Err(SourceDeliveryError::identity(format!(
                "Forgejo repository read returned {} instead of the admitted instance {expected}",
                wire::observed(&observed.html_url)
            )));
        }
        if observed.archived {
            return Err(SourceDeliveryError::identity(format!(
                "Forgejo repository {} is archived",
                wire::repository_identity(repository)
            )));
        }
        Ok(observed)
    }

    /// A branch that is not there is an answer, not a failure. A slash in its name stays path data.
    pub(super) async fn branch_revision(
        &self,
        repository: &str,
        branch: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Option<String>, SourceDeliveryError> {
        let (owner, name) = split_repository(repository)?;
        self.repository(repository, credential).await?;
        let mut path = vec!["repos", owner, name, "branches"];
        path.extend(branch.split('/'));
        let observed = self
            .api
            .optional_json::<wire::Branch>(ApiRequest::get(&path), credential)
            .await?;
        let Some(observed) = observed else {
            return Ok(None);
        };
        if observed.name != branch {
            return Err(SourceDeliveryError::identity(format!(
                "Forgejo branch read returned {} instead of {}",
                wire::branch_identity(&observed.name),
                wire::branch_identity(branch)
            )));
        }
        if !valid_git_revision(&observed.commit.id) {
            return Err(SourceDeliveryError::identity(format!(
                "Forgejo branch {} returned {} as its revision",
                wire::branch_identity(branch),
                wire::revision_identity(&observed.commit.id)
            )));
        }
        Ok(Some(observed.commit.id))
    }
}

pub(super) fn split_repository(value: &str) -> Result<(&str, &str), SourceDeliveryError> {
    let (owner, name) = value
        .split_once('/')
        .filter(|(owner, name)| !owner.is_empty() && !name.is_empty() && !name.contains('/'))
        .ok_or_else(|| SourceDeliveryError::identity(wire::REPOSITORY_ERROR))?;
    (owner.len() <= 100 && name.len() <= 100)
        .then_some((owner, name))
        .ok_or_else(|| SourceDeliveryError::identity(wire::REPOSITORY_ERROR))
}

#[async_trait]
impl SourceDeliveryAuthority for ForgejoDeliveryAuthority {
    async fn check_access(
        &self,
        repository: &str,
        credential: SourceDeliveryCredential<'_>,
        stopping: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> Result<(), SourceDeliveryError> {
        access::check(self, repository, credential, stopping).await
    }

    fn forge_credential_field(&self) -> Option<&'static str> {
        Some(FORGEJO_TOKEN_FIELD)
    }

    async fn observe_delivery(
        &self,
        request: SourceDeliveryRead<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceDeliverySnapshot, SourceDeliveryError> {
        review::observe(self, request, credential).await
    }

    async fn reconcile_delivery_target(
        &self,
        request: SourceTargetReconciliation<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceTargetIntegration, SourceDeliveryError> {
        let target = &request.target;
        let revision = self
            .branch_revision(&target.repository, &target.target_branch, credential)
            .await?
            .ok_or_else(|| {
                SourceDeliveryError::identity(format!(
                    "Forgejo target branch is absent: branch {} in repository {} (admitted source revision {}). Restore the target branch before resuming.",
                    wire::branch_identity(&target.target_branch),
                    wire::repository_identity(&target.repository),
                    wire::revision_identity(&target.base_revision),
                ))
            })?;
        crate::native_v2_delivery::transport::reconcile_target(
            self.git()?,
            request,
            &revision,
            credential,
        )
        .await
    }

    async fn reconcile_delivery_head(
        &self,
        request: SourceHeadReconciliation<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
        crate::native_v2_delivery::transport::reconcile_head(self.git()?, request, credential).await
    }

    async fn push_branch(
        &self,
        request: &SourcePushRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        self.repository(&request.target.repository, credential)
            .await?;
        self.git()?
            .push(&request.head_branch, &request.head_revision, credential)
            .await
    }

    fn prepare_commit_message(&self, title: &str) -> String {
        review::normalized_title(title)
    }

    fn review_links(
        &self,
        review: &SourceReviewReceipt,
    ) -> Result<Option<SourceReviewLinks>, SourceDeliveryError> {
        // The link is built from the admitted instance, never from a value the server returned.
        Ok(Some(review::links(self.base_url.as_str(), review)))
    }

    async fn open_or_update_review(
        &self,
        request: &SourceReviewRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewReceipt, SourceDeliveryError> {
        review::open_or_update(self, request, credential).await
    }

    async fn inspect_review(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewObservation, SourceDeliveryError> {
        review::inspect(self, review, credential).await
    }

    async fn request_merge(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
        _checkpoint: &dyn SourceReviewCheckpoint,
    ) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
        merge::request(self, review, credential).await
    }

    async fn withdraw_merge_request(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceMergeWithdrawal, SourceDeliveryError> {
        merge::withdraw(self, review, credential).await
    }

    async fn materialize_merge_conflict(
        &self,
        request: &SourceConflictRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceConflictOutcome, SourceDeliveryError> {
        merge::materialize_conflict(self, request, credential).await
    }
}
