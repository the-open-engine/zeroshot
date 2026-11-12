use serde::Deserialize;

use super::*;
use crate::source_code_provider::valid_delivery_description;

const LEGACY_TITLE: &str = "feat: complete Zeroshot task";
const LEGACY_DESCRIPTION: &str = "Created by Zeroshot v2.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ManifestInput {
    title: String,
    description: String,
    issue_number: Option<String>,
    #[serde(default)]
    review_responses: Vec<SourceReviewResponse>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LegacyIssueInput {
    issue_number: String,
}

pub(super) struct PreparedDeliveryInput {
    pub(super) title: String,
    pub(super) description: String,
    pub(super) source_issue: Option<SourceDeliveryIssue>,
    pub(super) review_responses: Vec<SourceReviewResponse>,
}

pub(super) fn delivery_input(input: &Value) -> Result<PreparedDeliveryInput, DeliveryStop> {
    if input.is_null() {
        return Ok(legacy_input(None));
    }
    if let Ok(input) = serde_json::from_value::<ManifestInput>(input.clone()) {
        if !valid_delivery_description(&input.description)
            || !valid_responses(&input.review_responses)
        {
            return Err(DeliveryStop::Outcome(WorkerOutcome::malformed()));
        }
        return Ok(PreparedDeliveryInput {
            review_responses: input.review_responses,
            title: input.title,
            description: input.description,
            source_issue: optional_source_issue(input.issue_number.as_deref())?,
        });
    }
    let input: LegacyIssueInput = malformed(serde_json::from_value(input.clone()))?;
    Ok(legacy_input(Some(parse_source_issue(&input.issue_number)?)))
}

fn legacy_input(source_issue: Option<SourceDeliveryIssue>) -> PreparedDeliveryInput {
    PreparedDeliveryInput {
        review_responses: Vec::new(),
        title: LEGACY_TITLE.to_owned(),
        description: LEGACY_DESCRIPTION.to_owned(),
        source_issue,
    }
}

fn optional_source_issue(value: Option<&str>) -> Result<Option<SourceDeliveryIssue>, DeliveryStop> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    parse_source_issue(value).map(Some)
}

fn parse_source_issue(value: &str) -> Result<SourceDeliveryIssue, DeliveryStop> {
    let number = value
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0 && number.to_string() == value)
        .ok_or_else(|| DeliveryStop::Outcome(WorkerOutcome::malformed()))?;
    Ok(SourceDeliveryIssue { number })
}

fn malformed<T>(result: Result<T, serde_json::Error>) -> Result<T, DeliveryStop> {
    result.map_err(|_| DeliveryStop::Outcome(WorkerOutcome::malformed()))
}

#[cfg(test)]
#[path = "input/tests.rs"]
mod tests;

fn valid_responses(responses: &[SourceReviewResponse]) -> bool {
    let mut ids = std::collections::BTreeSet::new();
    responses.len() <= 1000
        && serde_json::to_vec(responses).is_ok_and(|bytes| bytes.len() <= 1024 * 1024)
        && responses.iter().all(|response| {
            !response.thread_id.is_empty()
                && response.thread_id.len() <= 2048
                && !response.thread_id.chars().any(char::is_control)
                && ids.insert(&response.thread_id)
                && response.version.len() == 64
                && response.version.bytes().all(|b| b.is_ascii_hexdigit())
                && !response.response.trim().is_empty()
                && response.response.len() <= 64 * 1024
                && !response
                    .response
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        })
}
