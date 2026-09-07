use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;
use crate::native_v2_delivery::DeliveryTarget;

fn request() -> GitHubReviewRequest {
    GitHubReviewRequest {
        target: DeliveryTarget::new(
            "acme/project",
            "main",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .assert_value(),
        head_branch: "zeroshot/v2-run".to_owned(),
        head_revision: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
        source_issue: None,
    }
}

#[test]
fn stale_review_head_is_retryable_but_changed_identity_is_rejected() {
    let mut value = json!({
        "number": 17,
        "body": null,
        "base": {
            "ref": "main",
            "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "repo": {"full_name": "acme/project"}
        },
        "head": {
            "ref": "zeroshot/v2-run",
            "sha": "cccccccccccccccccccccccccccccccccccccccc",
            "repo": {"full_name": "acme/project"}
        }
    });
    let stale = serde_json::from_value(value.clone()).assert_value();
    let error = review_receipt(stale, &request()).expect_err("stale head must defer");
    assert_eq!(error, GitHubAuthorityError::review_head_not_visible());
    assert!(error.retryable_review_sync());

    value["base"]["ref"] = json!("other");
    let changed = serde_json::from_value(value).assert_value();
    assert_eq!(
        review_receipt(changed, &request()),
        Err(GitHubAuthorityError::Rejected)
    );
}

#[test]
fn stale_reference_head_is_retryable_but_changed_reference_is_rejected() {
    let mut value = json!({
        "ref": "refs/heads/zeroshot/v2-run",
        "object": {
            "sha": "cccccccccccccccccccccccccccccccccccccccc",
            "type": "commit"
        }
    });
    let stale = serde_json::from_value(value.clone()).assert_value();
    let error = require_review_head(stale, &request()).expect_err("stale head must defer");
    assert_eq!(error, GitHubAuthorityError::review_head_not_visible());
    assert!(error.retryable_review_sync());

    value["ref"] = json!("refs/heads/other");
    let changed = serde_json::from_value(value).assert_value();
    assert_eq!(
        require_review_head(changed, &request()),
        Err(GitHubAuthorityError::Rejected)
    );
}
