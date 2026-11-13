//! Native named errors and provider-owned response codes.
use serde_json::Value;
use crate::native_v2_runner::NodeRunnerError;

pub(super) fn classify(error: &Value) -> Option<NodeRunnerError> {
    let kind = error.get("type")?.as_str()?;
    let detail = error.get("message").and_then(Value::as_str).unwrap_or(kind);
    let status = error.get("status").and_then(Value::as_u64);
    if kind == "provider.auth" || status == Some(401) {
        return Some(NodeRunnerError::AuthenticationRequired(detail.to_owned()));
    }
    if kind == "provider.invalid-request" && status == Some(404) {
        // A 404 alone does not establish that the requested model was rejected.
        let body: Value = serde_json::from_str(error.pointer("/response/body")?.as_str()?).ok()?;
        if body.pointer("/error/code")?.as_str()? == "model_not_found" {
            return Some(NodeRunnerError::ModelUnavailable(detail.to_owned()));
        }
    }
    if kind == "provider.no-route" && detail.starts_with("Model unavailable:") {
        return Some(NodeRunnerError::ModelUnavailable(detail.to_owned()));
    }
    None
}
