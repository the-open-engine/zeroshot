//! Bounded, forge-owned conversation-comment effects, independent of Git transport.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{SourceDeliveryCredential, SourceDeliveryError, SourceProviderRef};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceDiscussionKind {
    Issue,
    PullRequest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceCommentDestination {
    pub provider: SourceProviderRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub repository: String,
    pub discussion_id: String,
    pub kind: SourceDiscussionKind,
    pub url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceComment {
    pub id: String,
    pub url: String,
    pub discussion_url: String,
    /// Provider-stable actor identity, not a display name.
    pub author: String,
    pub body: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceCommentPage {
    pub comments: Vec<SourceComment>,
    pub has_more: bool,
}

#[async_trait]
pub trait SourceCommentAuthority: Send + Sync {
    /// Pure canonical stored text, applied before publication hashing and durable intent.
    fn prepare_comment_body(&self, body: &str) -> Result<String, SourceDeliveryError> {
        Ok(body.to_owned())
    }

    /// Pure resolution within this authority's selected installation. None delegates to metadata.
    fn resolve_destination(
        &self,
        _url: &str,
    ) -> Result<Option<SourceCommentDestination>, SourceDeliveryError> {
        Ok(None)
    }

    /// Provider grammar belongs to the adapter; the default enforces common identity bounds.
    fn validate_comment(
        &self,
        destination: &SourceCommentDestination,
        comment: &SourceComment,
    ) -> Result<(), SourceDeliveryError> {
        validate_comment_identity(destination, comment)
    }

    fn comment_credential_field(&self) -> &'static str;

    async fn actor(
        &self,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<String, SourceDeliveryError>;

    async fn list_comments(
        &self,
        destination: &SourceCommentDestination,
        page: u32,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceCommentPage, SourceDeliveryError>;

    async fn create_comment(
        &self,
        destination: &SourceCommentDestination,
        body: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceComment, SourceDeliveryError>;

    async fn get_comment(
        &self,
        destination: &SourceCommentDestination,
        id: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceComment, SourceDeliveryError>;

    async fn verify_review_scope(
        &self,
        destination: &SourceCommentDestination,
        scope: &Value,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError>;
}

/// Common URL and identity checks, independent of provider resource-path grammar.
pub fn validate_comment_identity(
    destination: &SourceCommentDestination,
    comment: &SourceComment,
) -> Result<(), SourceDeliveryError> {
    if comment.id.is_empty()
        || comment.id.len() > 256
        || comment.author.is_empty()
        || comment.author.len() > 256
        || comment.body.len() > 128 * 1024
        || comment.discussion_url != destination.url
        || !same_public_origin(&destination.url, &comment.url)
    {
        return Err(SourceDeliveryError::Rejected);
    }
    Ok(())
}

/// Checks safe URLs while leaving provider permalink syntax to its adapter.
#[must_use]
pub fn same_public_origin(destination: &str, link: &str) -> bool {
    fn checked(value: &str) -> Option<url::Url> {
        if value.len() > 2_048 || value.chars().any(char::is_control) {
            return None;
        }
        let parsed = url::Url::parse(value).ok()?;
        (parsed.scheme() == "https"
            && parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none())
        .then_some(parsed)
    }
    match (checked(destination), checked(link)) {
        (Some(destination), Some(link)) => destination.origin() == link.origin(),
        _ => false,
    }
}
