use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::json;

use super::*;

fn request() -> GitHubReviewRequest {
    super::super::test_review_request()
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
    let error = review_receipt(stale, &request()).assert_error_with("stale head must defer");
    assert_eq!(error, GitHubAuthorityError::review_head_not_visible());
    assert!(error.retryable_review_sync());

    *value.pointer_mut("/base/ref").assert_value() = json!("other");
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
    let error = require_review_head(stale, &request()).assert_error_with("stale head must defer");
    assert_eq!(error, GitHubAuthorityError::review_head_not_visible());
    assert!(error.retryable_review_sync());

    *value.get_mut("ref").assert_value() = json!("refs/heads/other");
    let changed = serde_json::from_value(value).assert_value();
    assert_eq!(
        require_review_head(changed, &request()),
        Err(GitHubAuthorityError::Rejected)
    );
}

#[test]
fn target_reference_requires_the_exact_branch_and_commit_revision() {
    let valid = json!({
        "ref": "refs/heads/main",
        "object": {
            "sha": "cccccccccccccccccccccccccccccccccccccccc",
            "type": "commit"
        }
    });
    assert_eq!(
        reference_revision(serde_json::from_value(valid.clone()).assert_value(), "main")
            .assert_value(),
        "cccccccccccccccccccccccccccccccccccccccc"
    );

    for pointer in ["/ref", "/object/sha", "/object/type"] {
        let mut changed = valid.clone();
        *changed.pointer_mut(pointer).assert_value() = json!("invalid");
        assert_eq!(
            reference_revision(serde_json::from_value(changed).assert_value(), "main"),
            Err(GitHubAuthorityError::Rejected)
        );
    }
}
