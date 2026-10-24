//! Pre-start review input normalization shared by the CLI and authenticated hosts.
use openengine_cluster_protocol::{ResolvedSource, SourceBranchId, SourceRevisionId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
mod checkout;
pub(crate) use checkout::{checkout_references, prepare_local_checkout};
pub(crate) const LOCAL_CHECKOUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// A caller-owned issue or pull-request URL and optional issue checkout selection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReviewResolutionRequest {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<SourceBranchId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<SourceRevisionId>,
}

/// Ordinary graph input and the exact committed checkout it describes. Contains no credentials.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PreparedReview {
    pub initial_input: Value,
    pub source: ResolvedSource,
}

/// Read-only adapter boundary for hosts and deterministic metadata fixtures.
#[async_trait::async_trait]
pub trait ReviewMetadata: Send + Sync {
    async fn get(&self, path: &str) -> Result<Value, ReviewResolveError>;
    async fn graphql(&self, query: &str, variables: &Value) -> Result<Value, ReviewResolveError>;
}

pub async fn resolve_review_with(
    request: ReviewResolutionRequest,
    metadata: &dyn ReviewMetadata,
) -> Result<PreparedReview, ReviewResolveError> {
    crate::source_code_provider::adapters::core::github::review_task::resolve_with(
        request, metadata,
    )
    .await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewTargetKind {
    Issue,
    PullRequest,
}

/// Canonical GitHub discussion identity; URL parsing never performs network access.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewTarget {
    pub url: String,
    pub repository: String,
    pub number: u64,
    pub kind: ReviewTargetKind,
}

#[derive(Debug, thiserror::Error)]
pub enum ReviewResolveError {
    #[error("review input requires a canonical GitHub issue or pull-request HTTPS URL")]
    InvalidUrl,
    #[error("review metadata exceeds its supported size")]
    TooLarge,
    #[error("review metadata does not match the requested discussion or exact source")]
    Identity,
    #[error("review resolution requires a nonempty GitHub credential")]
    Credential,
    #[error("GitHub review metadata is unavailable (HTTP {0})")]
    Http(u16),
    #[error("GitHub review metadata could not be read")]
    Transport,
    #[error(
        "pull-request review source comes from the PR; branch and revision overrides are unsupported"
    )]
    PullRequestOverrides,
    #[error("linked issue requirements are unavailable or incomplete")]
    LinkedIssues,
}

/// Resolve an authenticated review without depending on a native `gh` login or subprocess.
pub async fn resolve_review(
    request: ReviewResolutionRequest,
    token: &str,
) -> Result<PreparedReview, ReviewResolveError> {
    crate::source_code_provider::adapters::core::github::review_task::resolve(request, token).await
}

/// Parse URLs with the maintained GitHub adapter's repository grammar.
pub fn parse_review_url(value: &str) -> Result<ReviewTarget, ReviewResolveError> {
    crate::source_code_provider::adapters::core::github::review_task::parse_url(value)
}
