use serde::Deserialize;

use super::*;
use crate::native_v2_delivery::github::valid_generated_description;

const LEGACY_TITLE: &str = "feat: complete Zeroshot task";
const LEGACY_DESCRIPTION: &str = "Created by Zeroshot v2.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ManifestInput {
    title: String,
    description: String,
    issue_number: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LegacyIssueInput {
    issue_number: String,
}

pub(super) struct PreparedDeliveryInput {
    pub(super) title: String,
    pub(super) description: String,
    pub(super) source_issue: Option<GitHubSourceIssue>,
}

pub(super) fn delivery_input(input: &Value) -> Result<PreparedDeliveryInput, DeliveryStop> {
    if input.is_null() {
        return Ok(legacy_input(None));
    }
    if let Ok(input) = serde_json::from_value::<ManifestInput>(input.clone()) {
        if !valid_generated_description(&input.description) {
            return Err(DeliveryStop::Outcome(WorkerOutcome::malformed()));
        }
        return Ok(PreparedDeliveryInput {
            title: input.title,
            description: input.description,
            source_issue: optional_source_issue(input.issue_number.as_deref())?,
        });
    }
    let input: LegacyIssueInput = malformed(serde_json::from_value(input.clone()))?;
    Ok(legacy_input(Some(parse_source_issue(&input.issue_number)?)))
}

fn legacy_input(source_issue: Option<GitHubSourceIssue>) -> PreparedDeliveryInput {
    PreparedDeliveryInput {
        title: LEGACY_TITLE.to_owned(),
        description: LEGACY_DESCRIPTION.to_owned(),
        source_issue,
    }
}

fn optional_source_issue(value: Option<&str>) -> Result<Option<GitHubSourceIssue>, DeliveryStop> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    parse_source_issue(value).map(Some)
}

fn parse_source_issue(value: &str) -> Result<GitHubSourceIssue, DeliveryStop> {
    let number = value
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0 && number.to_string() == value)
        .ok_or_else(|| DeliveryStop::Outcome(WorkerOutcome::malformed()))?;
    Ok(GitHubSourceIssue { number })
}

fn malformed<T>(result: Result<T, serde_json::Error>) -> Result<T, DeliveryStop> {
    result.map_err(|_| DeliveryStop::Outcome(WorkerOutcome::malformed()))
}

#[cfg(test)]
mod tests {
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
}
