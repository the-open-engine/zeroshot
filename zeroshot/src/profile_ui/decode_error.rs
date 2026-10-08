//! Turns runtime-settings decoding failures into editor-facing problems: a plain sentence plus
//! the JSON path of the offending field. Messages come from the protocol types' own serde errors;
//! nothing here knows which harnesses, providers, or models exist. Other failures keep the
//! generic "Invalid profile JSON" message.
use openengine_cluster_protocol::{
    ClaudeProvider, CodexProvider, CopilotProvider, ModelId, NodeRuntimeBinding, RunSize,
    RuntimePlan,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// A decoding failure located at a field, with the raw serde text kept for diagnostics.
pub(super) struct FieldProblem {
    pub field: String,
    pub message: String,
    pub detail: String,
}

/// Why a request body could not be decoded.
pub(super) enum DecodeError {
    /// A runtime setting has the wrong value or shape.
    Field(FieldProblem),
    /// Anything else, reported with serde's own wording.
    Malformed(String),
}

/// Decodes `bytes`. When the failure is in the runtime settings, it is reported at that field.
pub(super) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, DecodeError> {
    let error = match serde_json::from_slice(bytes) {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    let located = error
        .is_data()
        .then(|| serde_json::from_slice::<Value>(bytes).ok())
        .flatten()
        .and_then(|document| {
            let (field, detail) = locate_runtime(document.get("runtime")?)?;
            let message = describe(&field, &detail, lookup(&document, &field));
            Some(FieldProblem {
                field: field.join("."),
                message,
                detail,
            })
        });
    Err(located.map_or_else(
        || DecodeError::Malformed(format!("Invalid profile JSON: {error}")),
        DecodeError::Field,
    ))
}

/// Field paths stay as whole segments: node names may contain dots.
fn path(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

/// serde reports errors without a path, so re-check the runtime's parts with the same protocol
/// types, in declaration order, to find the field.
fn locate_runtime(runtime: &Value) -> Option<(Vec<String>, String)> {
    runtime.as_object()?;
    let provider = match runtime.get("harness").and_then(Value::as_str) {
        Some("codex") => check::<CodexProvider>(runtime.get("provider")),
        Some("claude") => check::<ClaudeProvider>(runtime.get("provider")),
        Some("copilot") => check::<CopilotProvider>(runtime.get("provider")),
        // An unknown or missing harness: let the runtime type itself say what it expects.
        _ => {
            let detail = check::<RuntimePlan>(Some(runtime))?;
            return Some((path(&["runtime", "harness"]), detail));
        }
    };
    if let Some(detail) = provider {
        return Some((path(&["runtime", "provider"]), detail));
    }
    if let Some(detail) = check::<RunSize>(runtime.get("size")) {
        return Some((path(&["runtime", "size"]), detail));
    }
    // A missing `nodes` falls back to serde's own "missing field" report at `runtime`.
    let nodes = runtime.get("nodes")?;
    let Some(nodes) = nodes.as_object() else {
        let detail = check::<serde_json::Map<String, Value>>(Some(nodes))?;
        return Some((path(&["runtime", "nodes"]), detail));
    };
    for (name, binding) in nodes {
        if binding.get("kind").and_then(Value::as_str) == Some("agent") {
            if let Some(detail) = check::<ModelId>(binding.get("model")) {
                return Some((path(&["runtime", "nodes", name, "model"]), detail));
            }
        }
        if let Some(detail) = check::<NodeRuntimeBinding>(Some(binding)) {
            return Some((path(&["runtime", "nodes", name]), detail));
        }
    }
    None
}

fn check<T: DeserializeOwned>(value: Option<&Value>) -> Option<String> {
    let Some(value) = value else {
        return Some("missing field".into());
    };
    serde_json::from_value::<T>(value.clone())
        .err()
        .map(|error| strip_position(&error.to_string()))
}

fn lookup<'a>(document: &'a Value, field: &[String]) -> Option<&'a Value> {
    field
        .iter()
        .try_fold(document, |value, segment| match value {
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?),
            _ => value.get(segment.as_str()),
        })
}

/// serde_json appends "at line N column M", which means nothing to someone using a form.
fn strip_position(message: &str) -> String {
    match message.rfind(" at line ") {
        Some(index) => message[..index].to_owned(),
        None => message.to_owned(),
    }
}

fn describe(field: &[String], detail: &str, value: Option<&Value>) -> String {
    let name = label(field);
    if value.and_then(Value::as_str) == Some("") || detail == "missing field" {
        return format!("Choose {} {name}.", article(&name));
    }
    if let Some(rest) = detail.strip_prefix("missing field `") {
        let missing = rest.split('`').next().unwrap_or(rest);
        let mut missing_field = field.to_vec();
        missing_field.push(missing.to_owned());
        return format!("{} is required.", capitalize(&label(&missing_field)));
    }
    if let Some(rest) = detail.strip_prefix("unknown variant `") {
        let (given, expected) = rest.split_once("`, expected ").unwrap_or((rest, ""));
        return format!(
            "{} `{given}` is not supported. Expected {}.",
            capitalize(&name),
            expected.replace('`', "")
        );
    }
    if let Some(rest) = detail.strip_prefix("unknown field `") {
        let unknown = rest.split('`').next().unwrap_or(rest);
        return format!("`{unknown}` is not a setting of {name}.");
    }
    if let Some(rest) = detail.strip_prefix("invalid type: ") {
        let expected = rest.split_once(", expected ").map_or(rest, |(_, e)| e);
        return format!("{} must be {expected}.", capitalize(&name));
    }
    format!("{}: {detail}.", capitalize(&name))
}

/// `runtime.nodes.work.model` reads as "model for `work`"; other paths use their last segment.
fn label(field: &[String]) -> String {
    let last = field.last().map_or("profile", String::as_str);
    match field.iter().position(|segment| segment == "nodes") {
        Some(index) if index + 2 < field.len() => {
            format!("{last} for `{}`", field[index + 1])
        }
        Some(index) if index + 2 == field.len() => format!("runtime for `{last}`"),
        _ => last.replace('_', " "),
    }
}

fn article(label: &str) -> &'static str {
    if label.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "decode_error/tests.rs"]
mod tests;
