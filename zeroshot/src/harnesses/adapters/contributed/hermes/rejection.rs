//! Preserve native result classifications; legacy message-only failures remain crashes.
use serde_json::Value;
use crate::native_v2_runner::NodeRunnerError;

pub(super) fn classify(event: &Value, model: &str, detail: String) -> NodeRunnerError {
    match event.get("failure_reason").and_then(Value::as_str) {
        Some("auth" | "auth_permanent") => NodeRunnerError::AuthenticationRequired(detail),
        Some("model_not_found" | "model_entitlement") => {
            NodeRunnerError::ModelUnavailable(format!("model {model}: {detail}"))
        }
        _ => NodeRunnerError::DriverDetail(detail),
    }
}

#[cfg(test)]
mod tests;
