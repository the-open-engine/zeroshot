use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::native_v2_delivery::command::capture_sensitive_with_input;
use crate::native_v2_review::{ReviewTargetKind, parse_review_url};
use crate::source_code_provider::*;

use super::{GhCliDeliveryAuthority, clean_command};

#[cfg(test)]
mod tests;

pub(crate) fn comment_destination(value: &str) -> Option<SourceCommentDestination> {
    let target = parse_review_url(value).ok()?;
    Some(SourceCommentDestination {
        provider: SourceProviderRef::new(
            SourceProviderId::new(super::super::GITHUB_PROVIDER_ID).ok()?,
            1,
        )
        .ok()?,
        repository: target.repository,
        discussion_id: target.number.to_string(),
        kind: match target.kind {
            ReviewTargetKind::Issue => SourceDiscussionKind::Issue,
            ReviewTargetKind::PullRequest => SourceDiscussionKind::PullRequest,
        },
        url: target.url,
    })
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
        .strip_prefix("https://api.github.com/repos/")
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

impl GhCliDeliveryAuthority {
    async fn comment_json_input(
        &self,
        endpoint: String,
        body: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Value, SourceDeliveryError> {
        let bytes =
            serde_json::to_vec(&json!({"body":body})).map_err(|_| SourceDeliveryError::Rejected)?;
        self.comment_api(
            &[
                endpoint,
                "--method".into(),
                "POST".into(),
                "--input".into(),
                "-".into(),
            ],
            &bytes,
            credential,
        )
        .await
    }

    async fn comment_api(
        &self,
        arguments: &[String],
        bytes: &[u8],
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Value, SourceDeliveryError> {
        let mut command = clean_command(&self.config, &self.config.gh_program, credential);
        command.arg("api").args(arguments);
        let secrets = vec![credential.expose().to_owned()];
        let output =
            capture_sensitive_with_input(&mut command, self.config.api_deadline, &secrets, bytes)
                .await
                .and_then(|output| output.require_success())
                .map_err(|failure| super::api::command_failure(failure, credential))?;
        if output.stdout_truncated() {
            return Err(SourceDeliveryError::api(
                None,
                "comment response exceeded its bound",
            ));
        }
        serde_json::from_str(output.stdout())
            .map_err(|_| SourceDeliveryError::api(None, "comment response was not valid JSON"))
    }
}

#[async_trait]
impl SourceCommentAuthority for GhCliDeliveryAuthority {
    fn comment_credential_field(&self) -> &'static str {
        "GH_TOKEN"
    }

    async fn actor(
        &self,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<String, SourceDeliveryError> {
        let value = self
            .comment_api(
                &[
                    "graphql".to_owned(),
                    "-f".to_owned(),
                    "query=query { viewer { id } }".to_owned(),
                ],
                &[],
                credential,
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
        let value = self
            .comment_api(
                &[format!(
                    "repos/{}/issues/{}/comments?per_page=100&page={page}",
                    destination.repository, destination.discussion_id
                )],
                &[],
                credential,
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
        let value = self
            .comment_json_input(
                format!(
                    "repos/{}/issues/{}/comments",
                    destination.repository, destination.discussion_id
                ),
                body,
                credential,
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
        let value = self
            .comment_api(
                &[format!(
                    "repos/{}/issues/comments/{id}",
                    destination.repository
                )],
                &[],
                credential,
            )
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
        let value = self
            .comment_api(
                &[format!(
                    "repos/{}/pulls/{}",
                    destination.repository, destination.discussion_id
                )],
                &[],
                credential,
            )
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
