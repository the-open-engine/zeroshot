//! Turns request decoding failures into editor-facing problems: a plain sentence plus the JSON
//! path of the offending field. Messages come from the protocol types' own serde errors; nothing
//! here knows which harnesses, providers, or models exist.
use openengine_cluster_protocol::{
    ClaudeProvider, CodexProvider, CopilotProvider, ModelId, NodeRuntimeBinding, RunSize,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// A decoding failure located at a field, with the raw serde text kept for diagnostics.
pub(super) struct FieldProblem {
    pub field: String,
    pub message: String,
    pub detail: String,
}

/// Decodes `bytes`, reporting data errors at the field that caused them. Syntax errors keep the
/// generic "Invalid profile JSON" wording because no field is involved.
pub(super) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Result<FieldProblem, String>> {
    let deserializer = &mut serde_json::Deserializer::from_slice(bytes);
    let error = match serde_path_to_error::deserialize(deserializer) {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    if !error.inner().is_data() {
        return Err(Err(format!("Invalid profile JSON: {}", error.inner())));
    }
    let document: Value = serde_json::from_slice(bytes).unwrap_or(Value::Null);
    let path = error.path().to_string();
    let (field, detail) = (path == "runtime")
        .then(|| document.get("runtime").and_then(locate_runtime))
        .flatten()
        .unwrap_or_else(|| (path, strip_position(&error.inner().to_string())));
    let value = lookup(&document, &field);
    Err(Ok(FieldProblem {
        message: describe(&field, &detail, value),
        field,
        detail,
    }))
}

/// `RuntimePlan` is an internally tagged enum, so serde loses the path below `runtime`. Re-check
/// its parts with the same protocol types, in declaration order, to find the field.
fn locate_runtime(runtime: &Value) -> Option<(String, String)> {
    let provider = match runtime.get("harness")?.as_str()? {
        "codex" => check::<CodexProvider>(runtime.get("provider")),
        "claude" => check::<ClaudeProvider>(runtime.get("provider")),
        "copilot" => check::<CopilotProvider>(runtime.get("provider")),
        _ => None,
    };
    if let Some(detail) = provider {
        return Some(("runtime.provider".into(), detail));
    }
    if let Some(detail) = check::<RunSize>(runtime.get("size")) {
        return Some(("runtime.size".into(), detail));
    }
    for (name, binding) in runtime.get("nodes")?.as_object()? {
        if binding.get("kind").and_then(Value::as_str) == Some("agent") {
            if let Some(detail) = check::<ModelId>(binding.get("model")) {
                return Some((format!("runtime.nodes.{name}.model"), detail));
            }
        }
        if let Some(detail) = check::<NodeRuntimeBinding>(Some(binding)) {
            return Some((format!("runtime.nodes.{name}"), detail));
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

fn lookup<'a>(document: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(document, |value, segment| match value {
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?),
            _ => value.get(segment),
        })
}

/// serde_json appends "at line N column M", which means nothing to someone using a form.
fn strip_position(message: &str) -> String {
    match message.rfind(" at line ") {
        Some(index) => message[..index].to_owned(),
        None => message.to_owned(),
    }
}

fn describe(field: &str, detail: &str, value: Option<&Value>) -> String {
    let name = label(field);
    if value.and_then(Value::as_str) == Some("") || detail == "missing field" {
        return format!("Choose {} {name}.", article(&name));
    }
    if let Some(rest) = detail.strip_prefix("missing field `") {
        let missing = rest.split('`').next().unwrap_or(rest);
        return format!(
            "{} is required.",
            capitalize(&label(&format!("{field}.{missing}")))
        );
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
fn label(field: &str) -> String {
    let segments: Vec<&str> = field.split('.').filter(|s| !s.is_empty()).collect();
    let last = segments.last().copied().unwrap_or("profile");
    match segments.iter().position(|s| *s == "nodes") {
        Some(index) if index + 2 < segments.len() => {
            format!("{last} for `{}`", segments[index + 1])
        }
        Some(index) if index + 2 == segments.len() => format!("runtime for `{last}`"),
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
