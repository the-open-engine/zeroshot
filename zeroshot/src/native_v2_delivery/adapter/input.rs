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
#[path = "input/tests.rs"]
mod tests;
