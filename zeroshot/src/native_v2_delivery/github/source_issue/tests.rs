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
fn refresh_rejects_unowned_legacy_reference_without_rewriting_body() {
    let marked = concat!(
        "<!-- zeroshot-delivery:generated:v1:start -->\n",
        "Old description.\n",
        "<!-- zeroshot-delivery:generated:v1:end -->\n\n",
        "Closes #208"
    );
    let unmanaged = "Created by Zeroshot v2.\n\nCloses #208";
    let request = GitHubReviewRequest {
        source_issue: Some(GitHubSourceIssue { number: 209 }),
        ..test_review_request()
    };

    for body in [marked, unmanaged] {
        assert_eq!(
            refresh_pull_request_body(Some(body), &request),
            Err(GitHubAuthorityError::Rejected)
        );
    }
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
