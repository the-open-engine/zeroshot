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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceCommentDestination {
    pub provider: SourceProviderRef,
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
