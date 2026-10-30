use async_trait::async_trait;
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};
use super::*;
use wire::{Actor, Note};

fn segment(kind: SourceDiscussionKind) -> &'static str {
    match kind {
        SourceDiscussionKind::Issue => "issues",
        SourceDiscussionKind::PullRequest => "merge_requests",
    }
}

fn noteable_type(kind: SourceDiscussionKind) -> &'static str {
    match kind {
        SourceDiscussionKind::Issue => "Issue",
        SourceDiscussionKind::PullRequest => "MergeRequest",
    }
}

/// GitLab removes carriage returns and trailing whitespace before storing report text.
pub(super) fn normalize_body(body: &str) -> String {
    body.replace('\r', "").trim_end().to_owned()
}

/// GitLab executes slash commands in notes and descriptions; never publish one as report text.
pub(super) fn require_literal_body(body: &str) -> Result<(), SourceDeliveryError> {
    let command = body.lines().any(|line| {
        let Some(rest) = line.trim_start().strip_prefix('/') else {
            return false;
        };
        let word = rest.split_whitespace().next().unwrap_or("");
        !word.is_empty()
            && word
                .bytes()
                .all(|byte| byte.is_ascii_alphabetic() || byte == b'_')
    });
    if command {
        return Err(SourceDeliveryError::identity(
            "GitLab report text contains a slash command; escape the slash before publishing",
        ));
    }
    Ok(())
}

fn note_scope_error() -> SourceDeliveryError {
    SourceDeliveryError::identity("GitLab note response changed the bound destination")
}

fn require_note_identity(note: &Note, project: u64, kind: &str) -> Result<(), SourceDeliveryError> {
    if note.id == 0
        || note.author.id == 0
        || note.noteable_id == 0
        || note.project_id != project
        || note.noteable_type != kind
        || note.body.len() > 128 * 1024
    {
        return Err(note_scope_error());
    }
    Ok(())
}

pub(super) fn require_note_scope(
    note: &Note,
    project: u64,
    kind: &str,
    id: &str,
) -> Result<(), SourceDeliveryError> {
    require_note_identity(note, project, kind)?;
    (note
        .noteable_iid
        .map(|number| number.to_string())
        .as_deref()
        == Some(id))
    .then_some(())
    .ok_or_else(note_scope_error)
}

/// Discussion responses can omit the IID; their global noteable ID must match the fetched MR.
pub(super) fn require_review_note_scope(
    note: &Note,
    review: &MergeRequest,
    project: u64,
) -> Result<(), SourceDeliveryError> {
    require_note_identity(note, project, "MergeRequest")?;
    (note.noteable_id == review.id && note.noteable_iid.is_none_or(|iid| iid == review.iid))
        .then_some(())
        .ok_or_else(note_scope_error)
}

impl GitLabDeliveryAuthority {
    fn require_destination(
        &self,
        destination: &SourceCommentDestination,
    ) -> Result<(), SourceDeliveryError> {
        if comment_destination_at(&destination.url, &self.base_url).as_ref() != Some(destination) {
            return Err(SourceDeliveryError::identity(
                "GitLab comment destination differs from admitted instance",
            ));
        }
        Ok(())
    }

    async fn read_note(
        &self,
        destination: &SourceCommentDestination,
        id: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceComment, SourceDeliveryError> {
        if !wire::valid_number(id) {
            return Err(SourceDeliveryError::Rejected);
        }
        self.require_destination(destination)?;
        let project = self.project(&destination.repository, credential).await?;
        let note: Note = self
            .api
            .json(
                ApiRequest::get(&[
                    "projects",
                    &destination.repository,
                    segment(destination.kind),
                    &destination.discussion_id,
                    "notes",
                    id,
                ]),
                credential,
            )
            .await?;
        require_note_scope(
            &note,
            project.id,
            noteable_type(destination.kind),
            &destination.discussion_id,
        )?;
        if note.id.to_string() != id {
            return Err(SourceDeliveryError::Rejected);
        }
        Ok(as_comment(note, destination))
    }
}

fn as_comment(note: Note, destination: &SourceCommentDestination) -> SourceComment {
    SourceComment {
        id: note.id.to_string(),
        url: format!("{}#note_{}", destination.url, note.id),
        discussion_url: destination.url.clone(),
        author: note.author.id.to_string(),
        body: note.body,
    }
}

#[async_trait]
impl SourceCommentAuthority for GitLabDeliveryAuthority {
    fn prepare_comment_body(&self, body: &str) -> Result<String, SourceDeliveryError> {
        let body = normalize_body(body);
        require_literal_body(&body)?;
        Ok(body)
    }

    fn resolve_destination(
        &self,
        url: &str,
    ) -> Result<Option<SourceCommentDestination>, SourceDeliveryError> {
        comment_destination_at(url, &self.base_url)
            .map(Some)
            .ok_or_else(|| {
                SourceDeliveryError::identity(
                    "GitLab comment URL differs from its selected instance",
                )
            })
    }

    fn comment_credential_field(&self) -> &'static str {
        TOKEN_FIELD
    }

    async fn actor(
        &self,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<String, SourceDeliveryError> {
        self.api.ensure_version(credential).await?;
        let actor: Actor = self
            .api
            .json(ApiRequest::get(&["user"]), credential)
            .await?;
        if actor.id == 0 {
            return Err(SourceDeliveryError::Rejected);
        }
        Ok(actor.id.to_string())
    }

    async fn list_comments(
        &self,
        destination: &SourceCommentDestination,
        page: u32,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceCommentPage, SourceDeliveryError> {
        if page == 0 || page > api::PAGE_LIMIT {
            return Err(SourceDeliveryError::Rejected);
        }
        self.require_destination(destination)?;
        let project = self.project(&destination.repository, credential).await?;
        let (notes, has_more) = self
            .api
            .json_page::<Note>(
                ApiRequest::get(&[
                    "projects",
                    &destination.repository,
                    segment(destination.kind),
                    &destination.discussion_id,
                    "notes",
                ])
                .query("per_page", "100")
                .query("page", page.to_string()),
                page,
                credential,
            )
            .await?;
        let comments = notes
            .into_iter()
            .map(|note| {
                require_note_scope(
                    &note,
                    project.id,
                    noteable_type(destination.kind),
                    &destination.discussion_id,
                )?;
                Ok(as_comment(note, destination))
            })
            .collect::<Result<Vec<_>, SourceDeliveryError>>()?;
        Ok(SourceCommentPage { comments, has_more })
    }

    async fn create_comment(
        &self,
        destination: &SourceCommentDestination,
        body: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceComment, SourceDeliveryError> {
        self.require_destination(destination)?;
        let body = self.prepare_comment_body(body)?;
        let project = self.project(&destination.repository, credential).await?;
        let note: Note = self
            .api
            .json(
                ApiRequest::write(
                    &[
                        "projects",
                        &destination.repository,
                        segment(destination.kind),
                        &destination.discussion_id,
                        "notes",
                    ],
                    Method::POST,
                    json!({"body":body}),
                ),
                credential,
            )
            .await?;
        require_note_scope(
            &note,
            project.id,
            noteable_type(destination.kind),
            &destination.discussion_id,
        )?;
        if note.body != body || note.system {
            return Err(SourceDeliveryError::identity(
                "GitLab changed the published comment body",
            ));
        }
        Ok(as_comment(note, destination))
    }

    async fn get_comment(
        &self,
        destination: &SourceCommentDestination,
        id: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceComment, SourceDeliveryError> {
        self.read_note(destination, id, credential).await
    }

    fn validate_comment(
        &self,
        destination: &SourceCommentDestination,
        comment: &SourceComment,
    ) -> Result<(), SourceDeliveryError> {
        self.require_destination(destination)?;
        if !valid_comment_permalink(destination, &comment.id, &comment.url)
            || !wire::valid_number(&comment.author)
            || comment.discussion_url != destination.url
        {
            return Err(SourceDeliveryError::identity(
                "GitLab comment permalink changed destination",
            ));
        }
        Ok(())
    }

    async fn verify_review_scope(
        &self,
        destination: &SourceCommentDestination,
        scope: &Value,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        self.require_destination(destination)?;
        if destination.kind == SourceDiscussionKind::Issue {
            return if scope.as_object().is_some_and(|value| value.is_empty()) {
                Ok(())
            } else {
                Err(SourceDeliveryError::Rejected)
            };
        }
        let scope: CapturedScope =
            serde_json::from_value(scope.clone()).map_err(|_| SourceDeliveryError::Rejected)?;
        self.require_captured_review(destination, &scope, credential)
            .await?;
        self.require_comparison_base(destination, &scope, credential)
            .await?;
        self.require_captured_review(destination, &scope, credential)
            .await?;
        Ok(())
    }
}

impl GitLabDeliveryAuthority {
    async fn require_captured_review(
        &self,
        destination: &SourceCommentDestination,
        scope: &CapturedScope,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        let target = self.project(&destination.repository, credential).await?;
        let review: MergeRequest = self
            .api
            .json(
                ApiRequest::get(&[
                    "projects",
                    &destination.repository,
                    "merge_requests",
                    &destination.discussion_id,
                ]),
                credential,
            )
            .await?;
        let source = if review.source_project_id == target.id {
            target.clone()
        } else {
            self.project(&review.source_project_id.to_string(), credential)
                .await?
        };
        scope.require_review(destination, &review, (&target, &source))
    }

    async fn require_comparison_base(
        &self,
        destination: &SourceCommentDestination,
        scope: &CapturedScope,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        if self
            .branch_revision(&destination.repository, &scope.base_branch, credential)
            .await?
            .as_deref()
            != Some(&scope.base_revision)
        {
            return Err(SourceDeliveryError::identity(
                "GitLab comment comparison base changed",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapturedScope {
    base_repository: String,
    base_branch: String,
    base_revision: String,
    head_repository: String,
    head_branch: String,
    head_revision: String,
}

impl CapturedScope {
    fn require_review(
        &self,
        destination: &SourceCommentDestination,
        review: &MergeRequest,
        projects: (&Project, &Project),
    ) -> Result<(), SourceDeliveryError> {
        let (target, source) = projects;
        let repositories_match = self.base_repository == target.path_with_namespace
            && self.head_repository == source.path_with_namespace
            && review.target_project_id == target.id
            && review.source_project_id == source.id;
        if review.id == 0
            || !repositories_match
            || !self.revisions_match(review)
            || !self.branches_match(review)
            || review.iid.to_string() != destination.discussion_id
        {
            return Err(SourceDeliveryError::identity(
                "GitLab comment comparison scope changed",
            ));
        }
        Ok(())
    }

    fn revisions_match(&self, review: &MergeRequest) -> bool {
        review.sha == self.head_revision
            && valid_git_revision(&self.head_revision)
            && valid_git_revision(&self.base_revision)
    }

    fn branches_match(&self, review: &MergeRequest) -> bool {
        review.source_branch == self.head_branch
            && review.target_branch == self.base_branch
            && valid_git_branch(&self.head_branch)
            && valid_git_branch(&self.base_branch)
    }
}
