use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;

#[test]
fn accepts_manifest_with_optional_canonical_issue_number() {
    let without_issue = delivery_input(&json!({
        "title": "fix: repair checkout",
        "description": "Repair the checkout flow."
    }))
    .assert_value();
    assert_eq!(without_issue.title, "fix: repair checkout");
    assert_eq!(without_issue.description, "Repair the checkout flow.");
    assert_eq!(without_issue.source_issue, None);

    let with_issue = delivery_input(&json!({
        "title": "fix: repair checkout",
        "description": "Repair the checkout flow.",
        "issueNumber": "208"
    }))
    .assert_value();
    assert_eq!(
        with_issue.source_issue,
        Some(GitHubSourceIssue { number: 208 })
    );
    assert_eq!(
        delivery_input(&json!({
            "title": "fix: repair checkout",
            "description": "Repair the checkout flow.",
            "issueNumber": ""
        }))
        .assert_value()
        .source_issue,
        None
    );
}

#[test]
fn accepts_legacy_null_and_issue_only_inputs() {
    let null = delivery_input(&Value::Null).assert_value();
    assert_eq!(null.title, LEGACY_TITLE);
    assert_eq!(null.description, LEGACY_DESCRIPTION);
    assert_eq!(null.source_issue, None);
    assert_eq!(
        delivery_input(&json!({"issueNumber":"208"}))
            .assert_value()
            .source_issue,
        Some(GitHubSourceIssue { number: 208 })
    );
}

#[test]
fn rejects_partial_manifests_noncanonical_issues_and_unknown_fields() {
    for malformed in [
        json!({}),
        json!({"title":"fix: repair checkout"}),
        json!({"description":"Repair the checkout flow."}),
        json!({"issueNumber":208}),
        json!({"issueNumber":"0"}),
        json!({"issueNumber":""}),
        json!({"issueNumber":"0208"}),
        json!({"issueNumber":"208","extra":true}),
        json!({
            "title":"fix: repair checkout",
            "description":"Repair the checkout flow.",
            "extra":true
        }),
        json!({
            "title":"fix: repair checkout",
            "description":"<!-- zeroshot-delivery:generated:v1:start -->"
        }),
        json!({
            "title":"fix: repair checkout",
            "description":"<!-- zeroshot-delivery:generated:v1:end -->"
        }),
    ] {
        assert!(delivery_input(&malformed).is_err());
    }
}
