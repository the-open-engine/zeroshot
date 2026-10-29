//! GitLab's trusted, instance-scoped delivery and conversation-comment adapter.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use async_trait::async_trait;
use openengine_cluster_protocol::SourceForgeBaseUrl;
use crate::native_v2_delivery::transport::{self, GitTransport};
use crate::source_code_provider::*;

mod api;
mod comment;
mod review;
mod wire;

#[cfg(test)]
mod tests;

use api::{Api, ApiRequest};
use wire::{MergeRequest, Project};

const TOKEN_FIELD: &str = "GITLAB_TOKEN";
const MINIMUM_VERSION: (u64, u64, u64) = (19, 5, 1);

#[derive(Clone)]
pub struct GitLabDeliveryAuthority {
    base_url: SourceForgeBaseUrl,
    api: Api,
    transport: Option<GitTransport>,
    // Retain validated identity for links; every API operation reads fresh project policy.
    projects: Arc<Mutex<BTreeMap<String, Project>>>,
    effects: Arc<Mutex<Effects>>,
}

#[derive(Default)]
struct Effects {
    creates: BTreeSet<String>,
    merges: BTreeSet<String>,
}

impl std::fmt::Debug for GitLabDeliveryAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitLabDeliveryAuthority")
            .field("base_url", &self.base_url)
            .field("transport", &self.transport)
            .finish_non_exhaustive()
    }
}

impl GitLabDeliveryAuthority {
    pub fn new(
        base_url: SourceForgeBaseUrl,
        transport: Option<GitTransport>,
    ) -> Result<Self, SourceDeliveryError> {
        let endpoint = format!("{}/api/v4/", base_url.as_str().trim_end_matches('/'));
        Ok(Self {
            base_url,
            api: Api::new(&endpoint)?,
            transport,
            projects: Arc::new(Mutex::new(BTreeMap::new())),
            effects: Arc::new(Mutex::new(Effects::default())),
        })
    }

    fn git(&self) -> Result<&GitTransport, SourceDeliveryError> {
        self.transport.as_ref().ok_or_else(|| {
            SourceDeliveryError::identity("GitLab Git delivery requires the admitted Git transport")
        })
    }

    async fn project(
        &self,
        repository: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Project, SourceDeliveryError> {
        self.api.ensure_version(credential).await?;
        let project: Project = self
            .api
            .json(ApiRequest::get(&["projects", repository]), credential)
            .await?;
        project.validate(repository, &self.base_url)?;
        let mut projects = self
            .projects
            .lock()
            .map_err(|_| SourceDeliveryError::Unavailable)?;
        if projects.len() >= 8 && !projects.contains_key(repository) {
            return Err(SourceDeliveryError::identity(
                "GitLab project scope exceeded its bound",
            ));
        }
        projects.insert(repository.to_owned(), project.clone());
        Ok(project)
    }

    async fn branch_revision(
        &self,
        repository: &str,
        branch: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Option<String>, SourceDeliveryError> {
        self.project(repository, credential).await?;
        let segments = ["projects", repository, "repository", "branches", branch];
        let request = ApiRequest::get(&segments);
        match self.api.json::<wire::Branch>(request, credential).await {
            Ok(value) => {
                if value.name != branch || !valid_git_revision(&value.commit.id) {
                    return Err(SourceDeliveryError::identity(
                        "GitLab branch observation changed identity",
                    ));
                }
                Ok(Some(value.commit.id))
            }
            Err(error) if error.api_status() == Some(404) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn merge_request(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<MergeRequest, SourceDeliveryError> {
        let project = self.project(&review.repository, credential).await?;
        let value: MergeRequest = self
            .api
            .json(
                ApiRequest::get(&[
                    "projects",
                    &review.repository,
                    "merge_requests",
                    &review.review_id,
                ]),
                credential,
            )
            .await?;
        value.require_review(review, project.id)?;
        Ok(value)
    }
}

#[async_trait]
impl SourceDeliveryAuthority for GitLabDeliveryAuthority {
    fn forge_credential_field(&self) -> Option<&'static str> {
        Some(TOKEN_FIELD)
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
        let revision = self
            .branch_revision(
                &request.target.repository,
                &request.target.target_branch,
                credential,
            )
            .await?
            .ok_or_else(|| SourceDeliveryError::identity("GitLab target branch is absent"))?;
        transport::reconcile_target(self.git()?, request, &revision, credential).await
    }

    async fn reconcile_delivery_head(
        &self,
        request: SourceHeadReconciliation<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
        transport::reconcile_head(self.git()?, request, credential).await
    }

    async fn push_branch(
        &self,
        request: &SourcePushRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        self.project(&request.target.repository, credential).await?;
        self.git()?
            .push(&request.head_branch, &request.head_revision, credential)
            .await
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
        receipt: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewObservation, SourceDeliveryError> {
        review::inspect(self, receipt, credential).await
    }

    async fn inspect_review_feedback(
        &self,
        receipt: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewFeedback, SourceDeliveryError> {
        review::feedback(self, receipt, credential).await
    }

    async fn request_merge(
        &self,
        receipt: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
        review::merge(self, receipt, credential).await
    }

    async fn withdraw_merge_request(
        &self,
        receipt: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceMergeWithdrawal, SourceDeliveryError> {
        review::withdraw(self, receipt, credential).await
    }

    async fn materialize_merge_conflict(
        &self,
        request: &SourceConflictRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceConflictOutcome, SourceDeliveryError> {
        let value = self.merge_request(&request.review, credential).await?;
        let revision = self
            .branch_revision(
                &request.review.repository,
                &request.review.target_branch,
                credential,
            )
            .await?
            .ok_or_else(|| SourceDeliveryError::identity("GitLab target branch is absent"))?;
        if value.detailed_merge_status == "need_rebase" {
            return review::integrate_rebase(self, request, &revision, credential).await;
        }
        transport::materialize_conflict(self.git()?, request, &revision, credential).await
    }

    fn review_links(
        &self,
        receipt: &SourceReviewReceipt,
    ) -> Result<Option<SourceReviewLinks>, SourceDeliveryError> {
        let projects = self
            .projects
            .lock()
            .map_err(|_| SourceDeliveryError::Unavailable)?;
        let project = projects
            .get(&receipt.repository)
            .ok_or(SourceDeliveryError::Unavailable)?;
        if !wire::valid_number(&receipt.review_id) || !valid_git_revision(&receipt.head_revision) {
            return Err(SourceDeliveryError::Rejected);
        }
        Ok(Some(SourceReviewLinks {
            url: format!("{}/-/merge_requests/{}", project.web_url, receipt.review_id),
            commit_url: format!("{}/-/commit/{}", project.web_url, receipt.head_revision),
        }))
    }
}

pub(super) fn repository_reference(remote: &str, base_url: &SourceForgeBaseUrl) -> Option<String> {
    let repository = super::instance::repository_at_remote(remote, base_url)?;
    wire::valid_repository_path(&repository).then_some(repository)
}

pub(super) fn comment_destination(url: &str) -> Option<SourceCommentDestination> {
    let parsed = super::instance::public_discussion_url(url)?;
    let base = SourceForgeBaseUrl::new(parsed.origin().ascii_serialization()).ok()?;
    comment_destination_at(url, &base)
}

pub(super) fn comment_destination_at(
    value: &str,
    base_url: &SourceForgeBaseUrl,
) -> Option<SourceCommentDestination> {
    let parsed = comment_url(value)?;
    let prefix = format!("{}/", base_url.as_str().trim_end_matches('/'));
    let relative = parsed.as_str().strip_prefix(&prefix)?;
    let (repository, discussion) = relative.split_once("/-/")?;
    if !wire::valid_repository_path(repository) {
        return None;
    }
    let (segment, id) = discussion.split_once('/')?;
    if !wire::valid_number(id) {
        return None;
    }
    let kind = discussion_kind(segment)?;
    Some(SourceCommentDestination {
        provider: provider_reference()?,
        base_url: Some(base_url.to_string()),
        repository: repository.to_owned(),
        discussion_id: id.to_owned(),
        kind,
        url: parsed.to_string(),
    })
}

fn provider_reference() -> Option<SourceProviderRef> {
    SourceProviderRef::new(SourceProviderId::new(super::GITLAB_PROVIDER_ID).ok()?, 1).ok()
}

fn discussion_kind(segment: &str) -> Option<SourceDiscussionKind> {
    match segment {
        "issues" | "work_items" => Some(SourceDiscussionKind::Issue),
        "merge_requests" => Some(SourceDiscussionKind::PullRequest),
        _ => None,
    }
}

fn comment_url(value: &str) -> Option<url::Url> {
    super::instance::public_discussion_url(value)
}

/// Pure GitLab resource grammar shared by native publication and final-output validation.
pub(super) fn valid_comment_permalink(
    destination: &SourceCommentDestination,
    id: &str,
    url: &str,
) -> bool {
    let Some(base) = destination
        .base_url
        .as_deref()
        .and_then(|value| SourceForgeBaseUrl::new(value).ok())
    else {
        return false;
    };
    comment_destination_at(&destination.url, &base).as_ref() == Some(destination)
        && wire::valid_number(id)
        && url == format!("{}#note_{id}", destination.url)
}
