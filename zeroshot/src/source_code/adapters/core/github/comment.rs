use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::source_code_provider::*;

use super::{GhCliDeliveryAuthority, Method, Request, repository_path};

const MAX_COMMENT_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[cfg(test)]
mod tests;

pub(crate) fn comment_destination(value: &str) -> Option<SourceCommentDestination> {
    let url = super::super::instance::public_discussion_url(value)?;
    let base =
        openengine_cluster_protocol::SourceForgeBaseUrl::new(url.origin().ascii_serialization())
            .ok()?;
    comment_destination_at(value, &base)
}

pub(crate) fn comment_destination_at(
    value: &str,
    base: &openengine_cluster_protocol::SourceForgeBaseUrl,
) -> Option<SourceCommentDestination> {
    let url = super::super::instance::public_discussion_url(value)?;
    let base_url = url::Url::parse(base.as_str()).ok()?;
    if url.origin() != base_url.origin() || base_url.path() != "/" {
        return None;
    }
    let path = super::super::instance::strip_base_path(url.path(), base_url.path())?;
    let (repository, kind, discussion_id) = discussion_path(path)?;
    Some(SourceCommentDestination {
        provider: SourceProviderRef::new(
            SourceProviderId::new(super::super::GITHUB_PROVIDER_ID).ok()?,
            1,
        )
        .ok()?,
        base_url: Some(base.to_string()),
        repository,
        discussion_id,
        kind,
        url: value.to_owned(),
    })
}

fn discussion_path(path: &str) -> Option<(String, SourceDiscussionKind, String)> {
    let parts = path.split('/').collect::<Vec<_>>();
    let [owner, name, kind, number] = parts.as_slice() else {
        return None;
    };
    let repository = format!("{owner}/{name}");
    if !super::super::valid_github_repository(&repository) {
        return None;
    }
    let id = number.parse::<u64>().ok()?;
    if id == 0 || id.to_string() != *number {
        return None;
    }
    let kind = match *kind {
        "issues" => SourceDiscussionKind::Issue,
        "pull" => SourceDiscussionKind::PullRequest,
        _ => return None,
    };
    Some((repository, kind, id.to_string()))
}

fn api_base(destination: &SourceCommentDestination) -> Option<String> {
    let base = destination
        .base_url
        .as_deref()
        .unwrap_or("https://github.com");
    let url = url::Url::parse(base).ok()?;
    let host = url.host_str()?;
    Some(if base == "https://github.com" {
        "https://api.github.com".to_owned()
    } else if host.ends_with(".ghe.com") {
        format!("https://api.{host}")
    } else {
        format!("{base}/api/v3")
    })
}

pub(crate) fn valid_github_permalink(
    destination: &SourceCommentDestination,
    id: &str,
    link: &str,
) -> bool {
    valid_comment_id(id)
        && permalink_destination(destination, id, link).is_some_and(|actual| {
            actual
                .repository
                .eq_ignore_ascii_case(&destination.repository)
                && actual.discussion_id == destination.discussion_id
        })
}

fn valid_comment_id(id: &str) -> bool {
    id.parse::<u64>()
        .ok()
        .filter(|id| *id > 0)
        .map(|id| id.to_string())
        .as_deref()
        == Some(id)
}

fn permalink_destination(
    destination: &SourceCommentDestination,
    id: &str,
    link: &str,
) -> Option<SourceCommentDestination> {
    let mut parsed = url::Url::parse(link).ok()?;
    if parsed.fragment()? != format!("issuecomment-{id}") {
        return None;
    }
    parsed.set_fragment(None);
    let base = openengine_cluster_protocol::SourceForgeBaseUrl::new(
        destination
            .base_url
            .as_deref()
            .unwrap_or("https://github.com"),
    )
    .ok()?;
    comment_destination_at(parsed.as_str(), &base)
}

/// Validates captured comparison identity without credentials, files, or network effects.
pub fn validate_comment_review_scope(
    destination: &SourceCommentDestination,
    scope: &Value,
) -> Result<(), SourceDeliveryError> {
    let fields = scope.as_object().ok_or(SourceDeliveryError::Rejected)?;
    if fields.is_empty() {
        return if destination.kind == SourceDiscussionKind::Issue {
            Ok(())
        } else {
            Err(SourceDeliveryError::identity("PR review scope is empty"))
        };
    }
    if destination.kind != SourceDiscussionKind::PullRequest || fields.len() != 6 {
        return Err(SourceDeliveryError::Rejected);
    }
    let parsed: CapturedReviewScope =
        serde_json::from_value(scope.clone()).map_err(|_| SourceDeliveryError::Rejected)?;
    if !super::super::valid_github_repository(parsed.base_repository.as_str())
        || !super::super::valid_github_repository(parsed.head_repository.as_str())
        || !parsed
            .base_repository
            .as_str()
            .eq_ignore_ascii_case(&destination.repository)
    {
        return Err(SourceDeliveryError::Rejected);
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CapturedReviewScope {
    base_repository: openengine_cluster_protocol::SourceRepositoryId,
    head_repository: openengine_cluster_protocol::SourceRepositoryId,
    #[serde(rename = "baseBranch")]
    _base_branch: openengine_cluster_protocol::SourceBranchId,
    #[serde(rename = "headBranch")]
    _head_branch: openengine_cluster_protocol::SourceBranchId,
    #[serde(rename = "baseRevision")]
    _base_revision: openengine_cluster_protocol::SourceRevisionId,
    #[serde(rename = "headRevision")]
    _head_revision: openengine_cluster_protocol::SourceRevisionId,
}

#[derive(Deserialize)]
struct CommentWire {
    id: u64,
    html_url: String,
    issue_url: String,
    body: String,
    user: ActorWire,
}

#[derive(Deserialize)]
struct ActorWire {
    node_id: String,
}

fn decode_comment(
    value: Value,
    destination: &SourceCommentDestination,
) -> Result<SourceComment, SourceDeliveryError> {
    let wire: CommentWire =
        serde_json::from_value(value).map_err(|_| SourceDeliveryError::Rejected)?;
    let suffix = format!("/issues/{}", destination.discussion_id);
    let repository = wire
        .issue_url
        .strip_prefix(&format!(
            "{}/repos/",
            api_base(destination).ok_or(SourceDeliveryError::Rejected)?
        ))
        .and_then(|value| value.strip_suffix(suffix.as_str()));
    if wire.id == 0
        || !repository.is_some_and(|value| value.eq_ignore_ascii_case(&destination.repository))
        || wire.body.len() > 128 * 1024
        || wire.user.node_id.is_empty()
        || wire.user.node_id.len() > 256
    {
        return Err(SourceDeliveryError::Rejected);
    }
    Ok(SourceComment {
        id: wire.id.to_string(),
        url: wire.html_url,
        discussion_url: destination.url.clone(),
        author: wire.user.node_id,
        body: wire.body,
    })
}

#[async_trait]
impl SourceCommentAuthority for GhCliDeliveryAuthority {
    fn validate_comment(
        &self,
        destination: &SourceCommentDestination,
        comment: &SourceComment,
    ) -> Result<(), SourceDeliveryError> {
        validate_comment_identity(destination, comment)?;
        valid_github_permalink(destination, &comment.id, &comment.url)
            .then_some(())
            .ok_or(SourceDeliveryError::Rejected)
    }
    fn comment_credential_field(&self) -> &'static str {
        "GH_TOKEN"
    }

    async fn actor(
        &self,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<String, SourceDeliveryError> {
        let value = self
            .bounded_json(
                Request::Graphql {
                    document: "query { viewer { id } }",
                    variables: &json!({}),
                },
                credential,
                MAX_COMMENT_RESPONSE_BYTES,
            )
            .await?;
        let id = value
            .pointer("/data/viewer/id")
            .and_then(Value::as_str)
            .filter(|value| {
                !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
            })
            .ok_or(SourceDeliveryError::Rejected)?;
        if value.get("errors").is_some() {
            return Err(SourceDeliveryError::Rejected);
        }
        Ok(id.to_owned())
    }

    async fn list_comments(
        &self,
        destination: &SourceCommentDestination,
        page: u32,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceCommentPage, SourceDeliveryError> {
        if page == 0 {
            return Err(SourceDeliveryError::Rejected);
        }
        let page = page.to_string();
        let path = repository_path(
            &destination.repository,
            &["issues", &destination.discussion_id, "comments"],
        )?;
        let value = self
            .bounded_json(
                Request::Rest {
                    method: Method::GET,
                    path: &path,
                    query: &[("per_page", "100"), ("page", &page)],
                    body: None,
                },
                credential,
                MAX_COMMENT_RESPONSE_BYTES,
            )
            .await?;
        let values = value
            .as_array()
            .filter(|values| values.len() <= 100)
            .ok_or(SourceDeliveryError::Rejected)?;
        Ok(SourceCommentPage {
            has_more: values.len() == 100,
            comments: values
                .iter()
                .cloned()
                .map(|value| decode_comment(value, destination))
                .collect::<Result<_, _>>()?,
        })
    }

    async fn create_comment(
        &self,
        destination: &SourceCommentDestination,
        body: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceComment, SourceDeliveryError> {
        let path = repository_path(
            &destination.repository,
            &["issues", &destination.discussion_id, "comments"],
        )?;
        let value = self
            .bounded_json(
                Request::Rest {
                    method: Method::POST,
                    path: &path,
                    query: &[],
                    body: Some(&json!({"body": body})),
                },
                credential,
                MAX_COMMENT_RESPONSE_BYTES,
            )
            .await?;
        decode_comment(value, destination)
    }

    async fn get_comment(
        &self,
        destination: &SourceCommentDestination,
        id: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceComment, SourceDeliveryError> {
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(SourceDeliveryError::Rejected);
        }
        let path = repository_path(&destination.repository, &["issues", "comments", id])?;
        let value = self
            .bounded_json(Request::get(&path), credential, MAX_COMMENT_RESPONSE_BYTES)
            .await?;
        decode_comment(value, destination)
    }

    async fn verify_review_scope(
        &self,
        destination: &SourceCommentDestination,
        scope: &Value,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        validate_comment_review_scope(destination, scope)?;
        let fields = scope.as_object().ok_or(SourceDeliveryError::Rejected)?;
        if fields.is_empty() {
            return Ok(());
        }
        let path = repository_path(
            &destination.repository,
            &["pulls", &destination.discussion_id],
        )?;
        let value = self
            .bounded_json(Request::get(&path), credential, MAX_COMMENT_RESPONSE_BYTES)
            .await?;
        for (field, pointer) in [
            ("baseRepository", "/base/repo/full_name"),
            ("baseBranch", "/base/ref"),
            ("baseRevision", "/base/sha"),
            ("headRepository", "/head/repo/full_name"),
            ("headBranch", "/head/ref"),
            ("headRevision", "/head/sha"),
        ] {
            let expected = fields
                .get(field)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or(SourceDeliveryError::Rejected)?;
            let same_identity = value.pointer(pointer).and_then(Value::as_str).is_some_and(
                |observed| match field {
                    "baseRepository" | "headRepository" => observed.eq_ignore_ascii_case(expected),
                    _ => observed == expected,
                },
            );
            if !same_identity {
                return Err(SourceDeliveryError::identity(
                    "review_scope_changed: captured PR comparison no longer matches",
                ));
            }
        }
        Ok(())
    }
}
