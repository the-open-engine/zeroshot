use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::*;

const MAX_FEEDBACK_ITEMS: usize = 100_000;

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub(super) struct UserWire {
    pub(super) id: u64,
    #[serde(default)]
    pub(super) node_id: Option<String>,
    pub(super) login: String,
    #[serde(default, rename = "type")]
    pub(super) kind: Option<String>,
}

#[derive(Deserialize)]
struct PermissionWire {
    permission: BasePermission,
    user: UserWire,
}

// GitHub returns the effective base permission, mapping maintain to write.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum BasePermission {
    None,
    Read,
    Write,
    Admin,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
struct IssueCommentWire {
    id: u64,
    updated_at: String,
    user: UserWire,
    body: Option<String>,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
struct ReviewWire {
    id: u64,
    state: String,
    submitted_at: Option<String>,
    commit_id: Option<String>,
    user: UserWire,
    body: Option<String>,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub(super) struct ReviewCommentWire {
    pub(super) id: u64,
    pub(super) updated_at: String,
    pub(super) user: UserWire,
    pub(super) body: String,
    pub(super) path: Option<String>,
    pub(super) line: Option<u64>,
    pub(super) original_line: Option<u64>,
    pub(super) commit_id: Option<String>,
    pub(super) in_reply_to_id: Option<u64>,
}

struct FeedbackItemDraft<'a> {
    key: String,
    provider_version: &'a str,
    author: String,
    location: Option<String>,
    body: String,
}

pub(super) async fn inspect(
    authority: &GhCliDeliveryAuthority,
    review: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewFeedback, SourceDeliveryError> {
    let before = authority.pull_request(review, credential).await?;
    require_review_identity(&before, review)?;

    let issue_comments = pages::<IssueCommentWire>(
        authority,
        format!(
            "repos/{}/issues/{}/comments?per_page=100",
            review.repository, review.review_id
        ),
        credential,
    )
    .await?;
    let reviews = pages::<ReviewWire>(
        authority,
        format!(
            "repos/{}/pulls/{}/reviews?per_page=100",
            review.repository, review.review_id
        ),
        credential,
    )
    .await?;
    let review_comments = pages::<ReviewCommentWire>(
        authority,
        format!(
            "repos/{}/pulls/{}/comments?per_page=100",
            review.repository, review.review_id
        ),
        credential,
    )
    .await?;

    let threads =
        super::conversations::read(authority, review, &review_comments, credential).await?;
    let mut permissions = BTreeMap::new();
    let mut items = Vec::new();
    for (author, item) in collect_items(
        issue_comments,
        reviews,
        review_comments
            .into_iter()
            .filter(|comment| !threads.comment_ids.contains(&comment.id))
            .collect(),
    )? {
        let allowed = match permissions.get(&(author.id, author.kind.clone())) {
            Some(allowed) => *allowed,
            None => {
                let allowed = can_repair(authority, review, &author, credential).await?;
                permissions.insert((author.id, author.kind.clone()), allowed);
                allowed
            }
        };
        if allowed {
            items.push(item);
        }
    }

    let after = authority.pull_request(review, credential).await?;
    require_review_identity(&after, review).map_err(|_| {
        SourceDeliveryError::Unavailable
            .with_context("GitHub PR identity changed while feedback was paginated")
    })?;

    items.extend(threads.items);
    Ok(SourceReviewFeedback {
        items,
        conversations: threads.conversations,
    })
}

pub(super) async fn can_repair(
    authority: &GhCliDeliveryAuthority,
    review: &SourceReviewReceipt,
    author: &UserWire,
    credential: SourceDeliveryCredential<'_>,
) -> Result<bool, SourceDeliveryError> {
    if author.id == 0 || author.login.is_empty() || matches!(author.login.as_str(), "." | "..") {
        return Err(SourceDeliveryError::identity(
            "GitHub feedback author is invalid",
        ));
    }
    // GitHub's authenticated Bot account type identifies app-installation actions.
    // User-to-server app actions retain a User author and require fresh write permission.
    // Posting on this bound repository proves the app's authority at publication time.
    if author.kind.as_deref() == Some("Bot") {
        return Ok(true);
    }
    let username: String = url::form_urlencoded::byte_serialize(author.login.as_bytes()).collect();
    let value = match authority
        .api(
            &[
                format!(
                    "repos/{}/collaborators/{username}/permission",
                    review.repository
                ),
                "--method".to_owned(),
                "GET".to_owned(),
            ],
            credential,
        )
        .await
    {
        Ok(value) => value,
        Err(error) if error.api_status() == Some(404) => return Ok(false),
        Err(error) => return Err(error),
    };
    let permission: PermissionWire = super::api::decode_response(value, credential)?;
    if permission.user.id != author.id {
        return Err(SourceDeliveryError::identity(
            "GitHub feedback permission changed author identity",
        ));
    }
    Ok(matches!(
        permission.permission,
        BasePermission::Write | BasePermission::Admin
    ))
}

fn collect_items(
    issue_comments: Vec<IssueCommentWire>,
    reviews: Vec<ReviewWire>,
    review_comments: Vec<ReviewCommentWire>,
) -> Result<Vec<(UserWire, SourceReviewFeedbackItem)>, SourceDeliveryError> {
    let count = issue_comments
        .len()
        .saturating_add(reviews.len())
        .saturating_add(review_comments.len());
    if count > MAX_FEEDBACK_ITEMS {
        return Err(SourceDeliveryError::api(
            None,
            format!(
                "GitHub PR feedback exceeded the absolute backstop of {MAX_FEEDBACK_ITEMS} items"
            ),
        ));
    }

    let mut items = Vec::with_capacity(count);
    items.extend(issue_comments.into_iter().filter_map(|comment| {
        let author = comment.user.clone();
        issue_item(comment).map(|item| (author, item))
    }));
    items.extend(reviews.into_iter().filter_map(|review| {
        let author = review.user.clone();
        review_item(review).map(|item| (author, item))
    }));
    items.extend(review_comments.into_iter().filter_map(|comment| {
        let author = comment.user.clone();
        review_comment_item(comment).map(|item| (author, item))
    }));
    for (author, item) in &mut items {
        item.version = format!(
            "{:x}",
            Sha256::digest(
                format!("{}\0{:?}\0{}", author.id, author.kind, item.version).as_bytes()
            )
        );
    }
    items.sort_by(|left, right| left.1.key.cmp(&right.1.key));
    Ok(items)
}

fn issue_item(comment: IssueCommentWire) -> Option<SourceReviewFeedbackItem> {
    Some(item(FeedbackItemDraft {
        key: format!("issue_comment:{}", comment.id),
        provider_version: &comment.updated_at,
        author: comment.user.login,
        location: None,
        body: nonempty_clean(comment.body.as_deref())?,
    }))
}

fn review_item(summary: ReviewWire) -> Option<SourceReviewFeedbackItem> {
    let body = nonempty_clean(summary.body.as_deref()).or_else(|| {
        (summary.state == "CHANGES_REQUESTED")
            .then(|| "Review requested changes without a written summary.".to_owned())
    })?;
    let location = Some(format!(
        "review state={} commit={}",
        clean(&summary.state),
        summary.commit_id.as_deref().map(clean).unwrap_or_default()
    ));
    Some(item(FeedbackItemDraft {
        key: format!("review:{}", summary.id),
        provider_version: summary.submitted_at.as_deref().unwrap_or_default(),
        author: summary.user.login,
        location,
        body,
    }))
}

fn review_comment_item(comment: ReviewCommentWire) -> Option<SourceReviewFeedbackItem> {
    let line = comment.line.or(comment.original_line);
    let location = Some(format!(
        "path={} line={} commit={} replyTo={}",
        comment.path.as_deref().map(clean).unwrap_or_default(),
        line.map(|value| value.to_string()).unwrap_or_default(),
        comment.commit_id.as_deref().map(clean).unwrap_or_default(),
        comment
            .in_reply_to_id
            .map(|value| value.to_string())
            .unwrap_or_default(),
    ));
    Some(item(FeedbackItemDraft {
        key: format!("review_comment:{}", comment.id),
        provider_version: &comment.updated_at,
        author: comment.user.login,
        location,
        body: nonempty_clean(Some(&comment.body))?,
    }))
}

pub(super) async fn pages<T: serde::de::DeserializeOwned>(
    authority: &GhCliDeliveryAuthority,
    endpoint: String,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Vec<T>, SourceDeliveryError> {
    let value = authority
        .api(
            &[
                endpoint,
                "--method".to_owned(),
                "GET".to_owned(),
                "--paginate".to_owned(),
                "--slurp".to_owned(),
            ],
            credential,
        )
        .await?;
    let pages: Vec<Vec<T>> = super::api::decode_response(value, credential)?;
    let count = pages
        .iter()
        .map(Vec::len)
        .try_fold(0usize, usize::checked_add)
        .filter(|count| *count <= MAX_FEEDBACK_ITEMS)
        .ok_or_else(|| {
            SourceDeliveryError::api(
                None,
                format!(
                    "GitHub PR feedback exceeded the absolute backstop of {MAX_FEEDBACK_ITEMS} items"
                ),
            )
        })?;
    let mut values = Vec::with_capacity(count);
    values.extend(pages.into_iter().flatten());
    Ok(values)
}

fn item(draft: FeedbackItemDraft<'_>) -> SourceReviewFeedbackItem {
    let FeedbackItemDraft {
        key,
        provider_version,
        author,
        location,
        body,
    } = draft;
    let author = clean(&author);
    let location = location.map(|value| clean(&value));
    let mut digest = Sha256::new();
    for value in [
        provider_version,
        author.as_str(),
        location.as_deref().unwrap_or_default(),
        body.as_str(),
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    SourceReviewFeedbackItem {
        key,
        version: format!("{:x}", digest.finalize()),
        author,
        location,
        body,
    }
}

fn nonempty_clean(value: Option<&str>) -> Option<String> {
    let value = clean(value?).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn clean(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect()
}

#[cfg(all(test, unix))]
#[path = "feedback/tests.rs"]
mod tests;
