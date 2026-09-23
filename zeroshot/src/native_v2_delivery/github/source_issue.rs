use super::*;
use super::metadata::{generated_body, generated_body_range, refresh_generated_body};
use super::wire::{IssueCommentWire, IssueWire, require_review_identity};

const LEGACY_UNMANAGED_BODY: &str = "Created by Zeroshot v2.";

pub(super) async fn connect_source_issue(
    authority: &GhCliDeliveryAuthority,
    request: &GitHubReviewRequest,
    review: &GitHubReviewReceipt,
    credential: GitHubCredential<'_>,
) -> Result<(), GitHubAuthorityError> {
    let Some(issue) = request.source_issue.as_ref() else {
        return Ok(());
    };
    let wire = authority.pull_request(review, credential).await?;
    require_review_identity(&wire, review)?;
    let closing_reference = closing_reference(issue.number);
    if !body_has_closing_reference(wire.body.as_deref(), &closing_reference) {
        let body = refresh_pull_request_body(wire.body.as_deref(), request)?;
        let updated = authority
            .patch_review(review, &[format!("body={body}")], credential)
            .await?;
        if updated.body.as_deref() != Some(body.as_str()) {
            return Err(GitHubAuthorityError::Rejected);
        }
    }
    comment_on_source_issue(authority, request, review, credential).await
}

async fn comment_on_source_issue(
    authority: &GhCliDeliveryAuthority,
    request: &GitHubReviewRequest,
    review: &GitHubReviewReceipt,
    credential: GitHubCredential<'_>,
) -> Result<(), GitHubAuthorityError> {
    let issue = request
        .source_issue
        .as_ref()
        .ok_or(GitHubAuthorityError::Rejected)?;
    let marker = delivery_comment_marker(&request.head_branch);
    let value = authority
        .api(
            &[
                format!("repos/{}/issues/{}", review.repository, issue.number),
                "--method".to_owned(),
                "GET".to_owned(),
            ],
            credential,
        )
        .await?;
    let issue_wire: IssueWire =
        serde_json::from_value(value).map_err(|_| GitHubAuthorityError::Rejected)?;
    let last_comment_page = issue_wire.comments.saturating_sub(1) / 100 + 1;
    for page in 1..=last_comment_page {
        let value = authority
            .api(
                &[
                    format!(
                        "repos/{}/issues/{}/comments",
                        review.repository, issue.number
                    ),
                    "--method".to_owned(),
                    "GET".to_owned(),
                    "-f".to_owned(),
                    "per_page=100".to_owned(),
                    "-f".to_owned(),
                    format!("page={page}"),
                ],
                credential,
            )
            .await?;
        let comments: Vec<IssueCommentWire> =
            serde_json::from_value(value).map_err(|_| GitHubAuthorityError::Rejected)?;
        if comments_have_marker(&comments, &marker) {
            return Ok(());
        }
    }
    authority
        .api(
            &[
                format!(
                    "repos/{}/issues/{}/comments",
                    review.repository, issue.number
                ),
                "--method".to_owned(),
                "POST".to_owned(),
                "-f".to_owned(),
                format!(
                    "body=Zeroshot opened pull request #{} for this issue.\n\n{}",
                    review.review_id, marker
                ),
            ],
            credential,
        )
        .await?;
    Ok(())
}

fn closing_reference(issue_number: u64) -> String {
    format!("Closes #{issue_number}")
}

fn generated_review_content(request: &GitHubReviewRequest) -> String {
    request.source_issue.as_ref().map_or_else(
        || request.description.clone(),
        |issue| {
            format!(
                "{}\n\n{}",
                request.description,
                closing_reference(issue.number)
            )
        },
    )
}

pub(super) fn pull_request_body(
    request: &GitHubReviewRequest,
) -> Result<String, GitHubAuthorityError> {
    generated_body(&generated_review_content(request))
}

pub(super) fn refresh_pull_request_body(
    current: Option<&str>,
    request: &GitHubReviewRequest,
) -> Result<String, GitHubAuthorityError> {
    if let Some(body) = current {
        if has_unowned_legacy_closing_reference(body)? {
            return Err(GitHubAuthorityError::Rejected);
        }
    }
    refresh_generated_body(current, &generated_review_content(request))
}

fn has_unowned_legacy_closing_reference(body: &str) -> Result<bool, GitHubAuthorityError> {
    let suffix = match generated_body_range(body)? {
        Some(range) => &body[range.end..],
        None => body.strip_prefix(LEGACY_UNMANAGED_BODY).unwrap_or_default(),
    };
    Ok(is_canonical_closing_reference_suffix(suffix))
}

fn is_canonical_closing_reference_suffix(suffix: &str) -> bool {
    let Some(line) = suffix.strip_prefix("\n\n") else {
        return false;
    };
    let line = line.split_once('\n').map_or(line, |(line, _)| line);
    line.strip_prefix("Closes #").is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn body_has_closing_reference(body: Option<&str>, closing_reference: &str) -> bool {
    body.is_some_and(|body| {
        body.lines()
            .any(|line| line.trim().eq_ignore_ascii_case(closing_reference))
    })
}

fn delivery_comment_marker(head_branch: &str) -> String {
    format!("<!-- zeroshot-delivery:{head_branch} -->")
}

fn comments_have_marker(comments: &[IssueCommentWire], marker: &str) -> bool {
    comments.iter().any(|comment| {
        comment
            .body
            .as_deref()
            .is_some_and(|body| body.contains(marker))
    })
}

#[cfg(test)]
#[path = "source_issue/tests.rs"]
mod tests;
