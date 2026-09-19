use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::*;

const MAX_FEEDBACK_ITEMS: usize = 100_000;

#[derive(Clone, Debug, Deserialize)]
struct UserWire {
    login: String,
}

#[derive(Clone, Debug, Deserialize)]
struct IssueCommentWire {
    id: u64,
    updated_at: String,
    user: UserWire,
    body: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ReviewWire {
    id: u64,
    state: String,
    submitted_at: Option<String>,
    commit_id: Option<String>,
    user: UserWire,
    body: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ReviewCommentWire {
    id: u64,
    updated_at: String,
    user: UserWire,
    body: String,
    path: Option<String>,
    line: Option<u64>,
    original_line: Option<u64>,
    commit_id: Option<String>,
    in_reply_to_id: Option<u64>,
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
    review: &GitHubReviewReceipt,
    credential: GitHubCredential<'_>,
) -> Result<GitHubReviewFeedback, GitHubAuthorityError> {
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

    let after = authority.pull_request(review, credential).await?;
    require_review_identity(&after, review).map_err(|_| {
        GitHubAuthorityError::Unavailable
            .with_context("GitHub PR identity changed while feedback was paginated")
    })?;

    collect_items(issue_comments, reviews, review_comments)
}

fn collect_items(
    issue_comments: Vec<IssueCommentWire>,
    reviews: Vec<ReviewWire>,
    review_comments: Vec<ReviewCommentWire>,
) -> Result<GitHubReviewFeedback, GitHubAuthorityError> {
    let count = issue_comments
        .len()
        .saturating_add(reviews.len())
        .saturating_add(review_comments.len());
    if count > MAX_FEEDBACK_ITEMS {
        return Err(GitHubAuthorityError::api(
            None,
            format!(
                "GitHub PR feedback exceeded the absolute backstop of {MAX_FEEDBACK_ITEMS} items"
            ),
        ));
    }

    let mut items = Vec::with_capacity(count);
    items.extend(issue_comments.into_iter().filter_map(issue_item));
    items.extend(reviews.into_iter().filter_map(review_item));
    items.extend(review_comments.into_iter().filter_map(review_comment_item));
    items.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(GitHubReviewFeedback { items })
}

fn issue_item(comment: IssueCommentWire) -> Option<GitHubReviewFeedbackItem> {
    Some(item(FeedbackItemDraft {
        key: format!("issue_comment:{}", comment.id),
        provider_version: &comment.updated_at,
        author: comment.user.login,
        location: None,
        body: nonempty_clean(comment.body.as_deref())?,
    }))
}

fn review_item(summary: ReviewWire) -> Option<GitHubReviewFeedbackItem> {
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

fn review_comment_item(comment: ReviewCommentWire) -> Option<GitHubReviewFeedbackItem> {
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

async fn pages<T: serde::de::DeserializeOwned>(
    authority: &GhCliDeliveryAuthority,
    endpoint: String,
    credential: GitHubCredential<'_>,
) -> Result<Vec<T>, GitHubAuthorityError> {
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
            GitHubAuthorityError::api(
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

fn item(draft: FeedbackItemDraft<'_>) -> GitHubReviewFeedbackItem {
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
    GitHubReviewFeedbackItem {
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

#[cfg(test)]
mod tests {
    use super::*;
    use openengine_cluster_testkit::assertions::AssertValue;

    fn user(login: &str) -> UserWire {
        UserWire {
            login: login.to_owned(),
        }
    }

    #[test]
    fn visible_feedback_surfaces_are_normalized_sorted_and_sanitized() {
        let feedback = collect_items(
            vec![IssueCommentWire {
                id: 9,
                updated_at: "2026-09-19T00:00:00Z".to_owned(),
                user: user("issue\0author"),
                body: Some("issue\0 body".to_owned()),
            }],
            vec![ReviewWire {
                id: 4,
                state: "CHANGES_REQUESTED".to_owned(),
                submitted_at: None,
                commit_id: None,
                user: user("reviewer"),
                body: None,
            }],
            vec![ReviewCommentWire {
                id: 2,
                updated_at: "2026-09-19T00:00:01Z".to_owned(),
                user: user("inline"),
                body: "fix this\nplease".to_owned(),
                path: Some("src/lib.rs".to_owned()),
                line: Some(17),
                original_line: None,
                commit_id: Some("a".repeat(40)),
                in_reply_to_id: Some(1),
            }],
        )
        .assert_value();

        assert_eq!(feedback.items.len(), 3);
        assert_eq!(feedback.items[0].key, "issue_comment:9");
        assert_eq!(feedback.items[1].key, "review:4");
        assert_eq!(feedback.items[2].key, "review_comment:2");
        assert_eq!(feedback.items[0].author, "issueauthor");
        assert_eq!(feedback.items[0].body, "issue body");
        assert!(feedback.items[1].body.contains("without a written summary"));
        assert!(
            feedback.items[2]
                .location
                .as_deref()
                .unwrap()
                .contains("line=17")
        );
        assert!(feedback.items.iter().all(|item| item.version.len() == 64));
    }

    #[test]
    fn edits_change_the_private_checkpoint_version() {
        let first = item(FeedbackItemDraft {
            key: "issue_comment:1".to_owned(),
            provider_version: "one",
            author: "author".to_owned(),
            location: None,
            body: "body".to_owned(),
        });
        let edited = item(FeedbackItemDraft {
            key: "issue_comment:1".to_owned(),
            provider_version: "two",
            author: "author".to_owned(),
            location: None,
            body: "edited".to_owned(),
        });
        assert_ne!(first.version, edited.version);
    }
}
