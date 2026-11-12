//! Native named errors and provider-owned response codes.
use serde_json::Value;
use crate::native_v2_runner::NodeRunnerError;

pub(super) fn classify(error: &Value) -> Option<NodeRunnerError> {
    let name = error.get("name")?.as_str()?;
    let data = error.get("data")?;
    let detail = data.get("message").and_then(Value::as_str).unwrap_or(name);
    match name {
        "ProviderAuthError" => Some(NodeRunnerError::AuthenticationRequired(detail.to_owned())),
        "APIError" if data.get("statusCode").and_then(Value::as_u64) == Some(401) => {
            Some(NodeRunnerError::AuthenticationRequired(detail.to_owned()))
        }
        // A 404 alone does not establish that the requested model was rejected.
        "APIError" => {
            let body: Value = serde_json::from_str(data.get("responseBody")?.as_str()?).ok()?;
            (body.pointer("/error/code")?.as_str()? == "model_not_found")
                .then(|| NodeRunnerError::ModelUnavailable(detail.to_owned()))
        }
        "ProviderModelNotFoundError" => {
            let model = data.get("modelID")?.as_str()?;
            Some(NodeRunnerError::ModelUnavailable(format!(
                "model {model}: {detail}"
            )))
        }
        _ => None,
    }
}
