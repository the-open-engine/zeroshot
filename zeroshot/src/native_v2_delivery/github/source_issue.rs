use super::*;
use super::metadata::{generated_body, refresh_generated_body};
use super::wire::{IssueCommentWire, IssueWire, require_review_identity};

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
    refresh_generated_body(current, &generated_review_content(request))
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
mod tests {
    use super::*;
    use crate::native_v2_delivery::GitHubSourceIssue;

    #[test]
    fn reference_is_created_inside_generated_metadata() {
        let request = GitHubReviewRequest {
            source_issue: Some(GitHubSourceIssue { number: 208 }),
            ..test_review_request()
        };
        assert_eq!(
            pull_request_body(&request).unwrap(),
            concat!(
                "<!-- zeroshot-delivery:generated:v1:start -->\n",
                "Repair the checkout flow.\n\n",
                "Closes #208\n",
                "<!-- zeroshot-delivery:generated:v1:end -->"
            )
        );
        assert!(body_has_closing_reference(
            Some("Human context\n\ncloses #208"),
            "Closes #208"
        ));
    }

    #[test]
    fn refresh_replaces_managed_issue_reference_and_preserves_human_text() {
        let original = GitHubReviewRequest {
            description: "Old description.".to_owned(),
            source_issue: Some(GitHubSourceIssue { number: 208 }),
            ..test_review_request()
        };
        let current = format!(
            "Human preface.\n\n{}\n\nHuman notes.\n\nCloses #999",
            pull_request_body(&original).unwrap()
        );
        let changed = GitHubReviewRequest {
            source_issue: Some(GitHubSourceIssue { number: 209 }),
            ..test_review_request()
        };
        assert_eq!(
            refresh_pull_request_body(Some(&current), &changed).unwrap(),
            concat!(
                "Human preface.\n\n",
                "<!-- zeroshot-delivery:generated:v1:start -->\n",
                "Repair the checkout flow.\n\n",
                "Closes #209\n",
                "<!-- zeroshot-delivery:generated:v1:end -->\n\n",
                "Human notes.\n\n",
                "Closes #999"
            )
        );

        let removed = GitHubReviewRequest {
            source_issue: None,
            ..test_review_request()
        };
        let body = refresh_pull_request_body(Some(&current), &removed).unwrap();
        assert!(!body.contains("Closes #208"));
        assert!(body.contains("Human preface."));
        assert!(body.contains("Human notes."));
        assert!(body.contains("Closes #999"));
    }

    #[test]
    fn delivery_marker_is_found_on_an_earlier_comment_page() {
        let marker = delivery_comment_marker("zeroshot/v2-run");
        let earlier_page = vec![IssueCommentWire {
            body: Some(format!("Run opened.\n\n{marker}")),
        }];
        let final_page = vec![IssueCommentWire {
            body: Some("A later human comment".to_owned()),
        }];

        assert!(comments_have_marker(&earlier_page, &marker));
        assert!(!comments_have_marker(&final_page, &marker));
    }
}
