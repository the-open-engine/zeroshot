//! Authenticated, bounded issue/PR hydration before graph admission.
use std::time::Duration;

use openengine_cluster_protocol::{
    ResolvedSource, SourceBranchId, SourceRepositoryId, SourceRevisionId,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::Instant;

use super::http::{Failure, FailureKind, Http, Reply, Request};
use crate::native_v2_review::{
    PreparedReview, ReviewResolutionRequest, ReviewResolveError, ReviewTarget, ReviewTargetKind,
    ReviewMetadata,
};
use crate::source_code_provider::SourceDeliveryCredential;

const API_VERSION: &str = "2022-11-28";
const API_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_API_BYTES: usize = 2 * 1024 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
#[path = "review_task/pull.rs"]
mod pull;
use pull::resolve_pull;

pub(crate) fn parse_url(value: &str) -> Result<ReviewTarget, ReviewResolveError> {
    let url = validated_url(value)?;
    let parts = url
        .path_segments()
        .ok_or(ReviewResolveError::InvalidUrl)?
        .collect::<Vec<_>>();
    let [owner, repository, kind, number] = parts.as_slice() else {
        return Err(ReviewResolveError::InvalidUrl);
    };
    let (kind, segment) = review_kind(kind)?;
    let number = canonical_number(number)?;
    let repository = canonical_repository(&format!("{owner}/{repository}"))?;
    Ok(ReviewTarget {
        url: format!("https://github.com/{repository}/{segment}/{number}"),
        repository,
        number,
        kind,
    })
}

fn matches_discussion_url(expected: &ReviewTarget, observed: &str) -> bool {
    parse_url(observed).is_ok_and(|target| {
        target.url == observed
            && target.kind == expected.kind
            && target.number == expected.number
            && target.repository.eq_ignore_ascii_case(&expected.repository)
    })
}

fn review_kind(kind: &str) -> Result<(ReviewTargetKind, &'static str), ReviewResolveError> {
    match kind {
        "issues" => Ok((ReviewTargetKind::Issue, "issues")),
        "pull" => Ok((ReviewTargetKind::PullRequest, "pull")),
        _ => Err(ReviewResolveError::InvalidUrl),
    }
}

fn validated_url(value: &str) -> Result<url::Url, ReviewResolveError> {
    if value.len() > 2048 {
        return Err(ReviewResolveError::InvalidUrl);
    }
    let url = url::Url::parse(value).map_err(|_| ReviewResolveError::InvalidUrl)?;
    if !clean_url(&url) {
        return Err(ReviewResolveError::InvalidUrl);
    }
    Ok(url)
}

fn clean_url(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn canonical_number(value: &str) -> Result<u64, ReviewResolveError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == value)
        .ok_or(ReviewResolveError::InvalidUrl)
}

fn canonical_repository(repository: &str) -> Result<String, ReviewResolveError> {
    let remote = format!("https://github.com/{repository}.git");
    if super::super::github_repository_reference(&remote).as_deref() != Some(repository) {
        return Err(ReviewResolveError::InvalidUrl);
    }
    Ok(repository.to_owned())
}

pub(crate) async fn resolve(
    request: ReviewResolutionRequest,
    token: &str,
) -> Result<PreparedReview, ReviewResolveError> {
    if token.trim().is_empty() || token.len() > 4096 || token.chars().any(char::is_control) {
        return Err(ReviewResolveError::Credential);
    }
    let http = Http::for_review(reqwest::Client::builder(), API_VERSION)
        .map_err(|_| ReviewResolveError::Transport)?;
    resolve_with(request, &HttpMetadata { http, token }).await
}

pub(crate) async fn resolve_with(
    request: ReviewResolutionRequest,
    metadata: &dyn ReviewMetadata,
) -> Result<PreparedReview, ReviewResolveError> {
    let target = parse_url(&request.url)?;
    let (url, context, source) = resolve_parts(&request, &target, metadata).await?;
    let initial_input = json!({"url":url,"context":context});
    if serde_json::to_vec(&initial_input)
        .map_err(|_| ReviewResolveError::Identity)?
        .len()
        > MAX_API_BYTES
    {
        return Err(ReviewResolveError::TooLarge);
    }
    Ok(PreparedReview {
        initial_input,
        source,
    })
}

async fn resolve_parts(
    request: &ReviewResolutionRequest,
    target: &ReviewTarget,
    metadata: &dyn ReviewMetadata,
) -> Result<ResolvedParts, ReviewResolveError> {
    match target.kind {
        ReviewTargetKind::Issue => resolve_issue(metadata, target, request).await,
        ReviewTargetKind::PullRequest => {
            if request.branch.is_some() || request.revision.is_some() {
                return Err(ReviewResolveError::PullRequestOverrides);
            }
            resolve_pull(metadata, target).await
        }
    }
}

type ResolvedParts = (String, Value, ResolvedSource);

fn validate_text(title: &str, body: &str) -> Result<(), ReviewResolveError> {
    if title.is_empty() || title.len() > 4096 || body.len() > MAX_BODY_BYTES {
        return Err(ReviewResolveError::TooLarge);
    }
    Ok(())
}

async fn resolve_issue(
    metadata: &dyn ReviewMetadata,
    target: &ReviewTarget,
    request: &ReviewResolutionRequest,
) -> Result<ResolvedParts, ReviewResolveError> {
    let issue: Issue = get(
        metadata,
        &format!("repos/{}/issues/{}", target.repository, target.number),
    )
    .await?;
    if issue.number != target.number
        || !matches_discussion_url(target, &issue.html_url)
        || issue.pull_request.is_some()
    {
        return Err(ReviewResolveError::Identity);
    }
    let source = issue_source(metadata, target, request).await?;
    let body = issue.body.unwrap_or_default();
    validate_text(&issue.title, &body)?;
    Ok((
        issue.html_url,
        json!({"title":issue.title,"body":body,"reviewScope":{}}),
        source,
    ))
}

async fn issue_source(
    metadata: &dyn ReviewMetadata,
    target: &ReviewTarget,
    request: &ReviewResolutionRequest,
) -> Result<ResolvedSource, ReviewResolveError> {
    let repository: Repository = get(metadata, &format!("repos/{}", target.repository)).await?;
    if !repository
        .full_name
        .eq_ignore_ascii_case(&target.repository)
    {
        return Err(ReviewResolveError::Identity);
    }
    let branch = match request.branch.as_ref() {
        Some(branch) => branch.clone(),
        None => SourceBranchId::new(repository.default_branch)
            .map_err(|_| ReviewResolveError::Identity)?,
    };
    let revision = issue_revision(metadata, target, request.revision.as_ref(), &branch).await?;
    Ok(ResolvedSource {
        repository: SourceRepositoryId::new(repository.full_name)
            .map_err(|_| ReviewResolveError::Identity)?,
        branch,
        revision,
        remote: None,
    })
}

async fn issue_revision(
    metadata: &dyn ReviewMetadata,
    target: &ReviewTarget,
    selected: Option<&SourceRevisionId>,
    branch: &SourceBranchId,
) -> Result<SourceRevisionId, ReviewResolveError> {
    let reference = selected.map_or(branch.as_str(), |revision| revision.as_str());
    let mut url = url::Url::parse(&format!(
        "https://api.github.com/repos/{}/commits/",
        target.repository
    ))
    .map_err(|_| ReviewResolveError::Transport)?;
    url.path_segments_mut()
        .map_err(|_| ReviewResolveError::Transport)?
        .pop_if_empty()
        .push(reference);
    let commit: Commit = get(metadata, url.path().trim_start_matches('/')).await?;
    let revision = SourceRevisionId::new(commit.sha).map_err(|_| ReviewResolveError::Identity)?;
    if selected.is_some_and(|expected| expected != &revision) {
        return Err(ReviewResolveError::Identity);
    }
    Ok(revision)
}

async fn get<T: serde::de::DeserializeOwned>(
    metadata: &dyn ReviewMetadata,
    path: &str,
) -> Result<T, ReviewResolveError> {
    let value = metadata.get(path).await?;
    if serde_json::to_vec(&value)
        .map_err(|_| ReviewResolveError::Identity)?
        .len()
        > MAX_API_BYTES
    {
        return Err(ReviewResolveError::TooLarge);
    }
    serde_json::from_value(value).map_err(|_| ReviewResolveError::Identity)
}

struct HttpMetadata<'a> {
    http: Http,
    token: &'a str,
}

#[async_trait::async_trait]
impl ReviewMetadata for HttpMetadata<'_> {
    async fn get(&self, path: &str) -> Result<Value, ReviewResolveError> {
        decode_reply(
            self.http
                .encoded_get(
                    path,
                    SourceDeliveryCredential::token(self.token),
                    (Instant::now() + API_TIMEOUT, MAX_API_BYTES),
                )
                .await,
        )
    }

    async fn graphql(&self, query: &str, variables: &Value) -> Result<Value, ReviewResolveError> {
        decode_reply(
            self.http
                .send(
                    Request::Graphql {
                        document: query,
                        variables,
                    },
                    SourceDeliveryCredential::token(self.token),
                    (Instant::now() + API_TIMEOUT, MAX_API_BYTES),
                )
                .await,
        )
    }
}

fn decode_reply(result: Result<Reply, Failure>) -> Result<Value, ReviewResolveError> {
    let reply = result.map_err(|failure| {
        // Review preparation exposes a refusal's status even if its error body was interrupted.
        if let Some(head) = failure
            .head
            .as_ref()
            .filter(|head| !head.status.is_success())
        {
            return ReviewResolveError::Http(head.status.as_u16());
        }
        match failure.kind {
            FailureKind::Oversize => ReviewResolveError::TooLarge,
            _ => ReviewResolveError::Transport,
        }
    })?;
    if !reply.head.status.is_success() {
        return Err(ReviewResolveError::Http(reply.head.status.as_u16()));
    }
    serde_json::from_slice(&reply.body).map_err(|_| ReviewResolveError::Identity)
}

#[derive(Deserialize)]
struct Issue {
    number: u64,
    html_url: String,
    title: String,
    body: Option<String>,
    pull_request: Option<Value>,
}
#[derive(Deserialize)]
struct Repository {
    full_name: String,
    default_branch: String,
}
#[derive(Deserialize)]
struct Commit {
    sha: String,
}
#[cfg(test)]
#[path = "review_task/tests.rs"]
mod tests;
