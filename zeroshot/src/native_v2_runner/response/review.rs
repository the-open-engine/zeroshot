//! Bounded review verdict shape shared by verifier responses.
//!
//! A review verdict is provider output like any other, but its natural shape is unbounded: finding
//! lists, quoted diffs, and per-file notes all fit in one object. The runner keeps the shape small
//! enough to fit a bounded output budget and rejects an oversized verdict with a mechanical
//! correction that names the offending field instead of restating the whole object.

use openengine_cluster_protocol::PayloadType;
use serde_json::{Value, json};

/// Maximum number of findings one verdict may carry.
pub(crate) const MAX_REVIEW_FINDINGS: usize = 8;
/// Maximum character count of one finding summary.
pub(crate) const MAX_REVIEW_FINDING_SUMMARY_CHARS: usize = 400;
/// Maximum character count of an optional `file:line` location.
pub(crate) const MAX_REVIEW_LOCATION_CHARS: usize = 256;
/// Maximum character count of a separate short verdict string.
pub(crate) const MAX_REVIEW_VERDICT_CHARS: usize = 64;
/// Maximum character count of the prospective change description / feedback message.
pub(crate) const MAX_REVIEW_MESSAGE_CHARS: usize = 4_000;

pub(crate) const FINDINGS_FIELD: &str = "findings";
pub(crate) const SUMMARY_FIELD: &str = "summary";
pub(crate) const LOCATION_FIELD: &str = "location";
pub(crate) const VERDICT_FIELD: &str = "verdict";
pub(crate) const TRUNCATED_FIELD: &str = "truncated";

/// Per-finding room for JSON keys, punctuation, and a short location. It is derived from the caps
/// above rather than from a provider catalog, because model identifiers remain opaque.
const FINDING_OUTPUT_CHARS: usize =
    MAX_REVIEW_FINDING_SUMMARY_CHARS + MAX_REVIEW_LOCATION_CHARS + 96;
const REVIEW_OUTPUT_CHARS: usize =
    MAX_REVIEW_FINDINGS * FINDING_OUTPUT_CHARS + MAX_REVIEW_MESSAGE_CHARS + 512;

const fn round_up(value: usize, step: usize) -> usize {
    value.div_ceil(step) * step
}

/// Token budget sized to the bounded verdict shape. It is deliberately independent of any model
/// identifier; a caller-declared provider limit can only lower it.
pub(crate) const REVIEW_OUTPUT_TOKEN_BUDGET: u32 =
    round_up(REVIEW_OUTPUT_CHARS.div_ceil(4), 256) as u32;

/// The effective output budget for a review verdict: the schema-derived bound, lowered by a
/// caller-declared provider output limit when one exists.
pub(crate) fn output_token_budget(declared: Option<u32>) -> u32 {
    declared.map_or(REVIEW_OUTPUT_TOKEN_BUDGET, |limit| {
        limit.min(REVIEW_OUTPUT_TOKEN_BUDGET)
    })
}

/// True when a declared payload contains a `findings` field anywhere in its record structure.
pub(crate) fn payload_has_findings(payload: &PayloadType) -> bool {
    match payload {
        PayloadType::Record { fields } => fields.iter().any(|(name, field)| {
            name.as_str() == FINDINGS_FIELD || payload_has_findings(&field.value_type)
        }),
        PayloadType::Array { items } => payload_has_findings(items),
        PayloadType::Null
        | PayloadType::Boolean
        | PayloadType::Integer
        | PayloadType::Number
        | PayloadType::String
        | PayloadType::Enum { .. } => false,
    }
}

/// Rejects an over-long verdict field, naming the exact field and its cap.
pub(crate) fn validate_review_value(path: &str, value: &Value) -> Result<(), String> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    if let Some(message) = object.get("message").and_then(Value::as_str) {
        let chars = message.chars().count();
        if chars > MAX_REVIEW_MESSAGE_CHARS {
            return Err(format!(
                "{path}.message is {chars} characters; the maximum is {MAX_REVIEW_MESSAGE_CHARS}"
            ));
        }
    }
    if let Some(verdict) = object.get(VERDICT_FIELD).and_then(Value::as_str) {
        let chars = verdict.chars().count();
        if chars > MAX_REVIEW_VERDICT_CHARS {
            return Err(format!(
                "{path}.{VERDICT_FIELD} is {chars} characters; the maximum is \
                 {MAX_REVIEW_VERDICT_CHARS}"
            ));
        }
    }
    let Some(findings) = object.get(FINDINGS_FIELD).and_then(Value::as_array) else {
        return Ok(());
    };
    if findings.len() > MAX_REVIEW_FINDINGS {
        return Err(format!(
            "{path}.{FINDINGS_FIELD} has {} items; the maximum is {MAX_REVIEW_FINDINGS} findings",
            findings.len()
        ));
    }
    for (index, finding) in findings.iter().enumerate() {
        let Some(finding) = finding.as_object() else {
            continue;
        };
        if let Some(summary) = finding.get(SUMMARY_FIELD).and_then(Value::as_str) {
            let chars = summary.chars().count();
            if chars > MAX_REVIEW_FINDING_SUMMARY_CHARS {
                return Err(format!(
                    "{path}.{FINDINGS_FIELD}[{index}].{SUMMARY_FIELD} is {chars} characters; the \
                     maximum is {MAX_REVIEW_FINDING_SUMMARY_CHARS}"
                ));
            }
        }
        if let Some(location) = finding.get(LOCATION_FIELD).and_then(Value::as_str) {
            let chars = location.chars().count();
            if chars > MAX_REVIEW_LOCATION_CHARS {
                return Err(format!(
                    "{path}.{FINDINGS_FIELD}[{index}].{LOCATION_FIELD} is {chars} characters; the \
                     maximum is {MAX_REVIEW_LOCATION_CHARS}"
                ));
            }
        }
    }
    Ok(())
}

/// Adds the same caps to the provider schema so the model sees them before writing.
pub(crate) fn annotate_schema(schema: &mut Value, constraints: bool) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
        if let Some(findings) = properties.get_mut(FINDINGS_FIELD) {
            annotate_findings_schema(findings, constraints);
            if let Some(findings) = findings.as_object_mut() {
                findings.insert(
                    "description".to_owned(),
                    json!(format!(
                        "At most {MAX_REVIEW_FINDINGS} findings; never quote diffs. Each finding \
                         has a summary of at most {MAX_REVIEW_FINDING_SUMMARY_CHARS} characters \
                         and an optional file:line location."
                    )),
                );
            }
        }
        if let Some(message) = properties.get_mut("message") {
            annotate_string_schema(message, MAX_REVIEW_MESSAGE_CHARS, constraints);
        }
        if let Some(verdict) = properties.get_mut(VERDICT_FIELD) {
            annotate_string_schema(verdict, MAX_REVIEW_VERDICT_CHARS, constraints);
        }
    }
}

fn annotate_findings_schema(schema: &mut Value, constraints: bool) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    if let Some(any_of) = object.get_mut("anyOf").and_then(Value::as_array_mut) {
        for branch in any_of {
            annotate_findings_schema(branch, constraints);
        }
        return;
    }
    if object.get("type").and_then(Value::as_str) != Some("array") {
        return;
    }
    if constraints {
        object.insert("maxItems".to_owned(), json!(MAX_REVIEW_FINDINGS));
    }
    if let Some(items) = object.get_mut("items") {
        annotate_finding(items, constraints);
    }
}

fn annotate_string_schema(schema: &mut Value, max_length: usize, constraints: bool) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    if let Some(any_of) = object.get_mut("anyOf").and_then(Value::as_array_mut) {
        for branch in any_of {
            annotate_string_schema(branch, max_length, constraints);
        }
        return;
    }
    if constraints && object.get("type").and_then(Value::as_str) == Some("string") {
        object.insert("maxLength".to_owned(), json!(max_length));
    }
}

fn annotate_finding(schema: &mut Value, constraints: bool) {
    let Some(properties) = schema
        .as_object_mut()
        .and_then(|object| object.get_mut("properties"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if let Some(summary) = properties.get_mut(SUMMARY_FIELD) {
        annotate_string_schema(summary, MAX_REVIEW_FINDING_SUMMARY_CHARS, constraints);
    }
    if let Some(location) = properties.get_mut(LOCATION_FIELD) {
        annotate_string_schema(location, MAX_REVIEW_LOCATION_CHARS, constraints);
    }
}

/// The shape caps, independent of the provider's output limit.
pub(crate) fn schema_description() -> String {
    format!(
        "At most {MAX_REVIEW_FINDINGS} findings; each summary is at most \
         {MAX_REVIEW_FINDING_SUMMARY_CHARS} characters, each optional location is a short \
         file:line of at most {MAX_REVIEW_LOCATION_CHARS} characters, the message is at most \
         {MAX_REVIEW_MESSAGE_CHARS} characters, and the verdict string is at most \
         {MAX_REVIEW_VERDICT_CHARS} characters. Do not quote diffs."
    )
}

/// A prompt line stating the effective budget and shape, so the model does not try to fill the
/// provider's whole output window with quoted diffs.
pub(crate) fn guidance(budget: u32) -> String {
    let shrink = if budget < REVIEW_OUTPUT_TOKEN_BUDGET {
        " The applied limit is below the shape budget; shorten the message and findings to fit."
    } else {
        ""
    };
    format!(
        "Review output budget: at most {budget} tokens.{shrink} {}\n",
        schema_description()
    )
}

/// Append any structured finding summaries that the message does not already contain, so the
/// bounded findings still reach the repair worker through the message feedback channel.
pub(crate) fn merge_findings_into_message(value: &mut Value, path: &str) -> Result<(), String> {
    let Some(object) = value.as_object_mut() else {
        return Ok(());
    };
    let message = object
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let Some(findings) = object.get(FINDINGS_FIELD).and_then(Value::as_array) else {
        return Ok(());
    };
    let mut lines = Vec::new();
    for finding in findings {
        let Some(finding) = finding.as_object() else {
            continue;
        };
        let Some(summary) = finding.get(SUMMARY_FIELD).and_then(Value::as_str) else {
            continue;
        };
        let summary = summary.trim();
        if summary.is_empty() || message.contains(summary) {
            continue;
        }
        match finding.get(LOCATION_FIELD).and_then(Value::as_str) {
            Some(location) if !location.trim().is_empty() => {
                lines.push(format!("- {summary} ({})", location.trim()));
            }
            _ => lines.push(format!("- {summary}")),
        }
    }
    if lines.is_empty() {
        return Ok(());
    }
    let rendered = format!("Findings:\n{}", lines.join("\n"));
    let message = if message.is_empty() {
        rendered
    } else {
        format!("{message}\n\n{rendered}")
    };
    let chars = message.chars().count();
    if chars > MAX_REVIEW_MESSAGE_CHARS {
        return Err(format!(
            "{path}.message plus findings would be {chars} characters; the maximum is \
             {MAX_REVIEW_MESSAGE_CHARS}"
        ));
    }
    object.insert("message".to_owned(), Value::String(message));
    Ok(())
}

/// The smallest value that satisfies a declared payload type. Truncation replaces a missing
/// verifier answer, so the runtime must still be able to build a valid response envelope without
/// inventing review content.
pub(crate) fn default_payload_value(payload: &PayloadType) -> Value {
    match payload {
        PayloadType::Null => Value::Null,
        PayloadType::Boolean => json!(false),
        PayloadType::Integer => json!(0),
        PayloadType::Number => json!(0.0),
        PayloadType::String => json!(""),
        PayloadType::Record { fields } => Value::Object(
            fields
                .iter()
                .filter(|(_, field)| field.required)
                .map(|(name, field)| {
                    (
                        name.as_str().to_owned(),
                        default_payload_value(&field.value_type),
                    )
                })
                .collect(),
        ),
        PayloadType::Array { .. } => json!([]),
        PayloadType::Enum { values } => values
            .values()
            .first()
            .map_or(Value::Null, |label| json!(label.as_str())),
    }
}

/// A diagnostic that carries the truncation explanation in its required `message` field while
/// satisfying the rest of the declared shape.
pub(crate) fn diagnostic_with_message(payload: &PayloadType, message: &str) -> Value {
    match payload {
        PayloadType::String => json!(message),
        PayloadType::Record { fields } => Value::Object(
            fields
                .iter()
                .filter(|(_, field)| field.required)
                .map(|(name, field)| {
                    let value = if name.as_str() == "message" {
                        json!(message)
                    } else {
                        default_payload_value(&field.value_type)
                    };
                    (name.as_str().to_owned(), value)
                })
                .collect(),
        ),
        _ => default_payload_value(payload),
    }
}
