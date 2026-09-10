use reqwest::{StatusCode, Url};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use openengine_cluster_protocol::TargetHttpProblem;

use super::{read_json, read_json_with_limit, require_response_route};
use crate::native_v2_target::TargetAuthorityError;

pub(in crate::native_v2_target::controller_authority) async fn read_success_json<
    T: DeserializeOwned,
>(
    response: reqwest::Response,
    expected: &Url,
    operation: &'static str,
) -> Result<T, TargetAuthorityError> {
    read_success_json_with_limit(response, expected, operation, super::MAX_RESPONSE_BYTES).await
}

pub(in crate::native_v2_target::controller_authority) async fn read_success_json_with_limit<
    T: DeserializeOwned,
>(
    response: reqwest::Response,
    expected: &Url,
    operation: &'static str,
    maximum_bytes: usize,
) -> Result<T, TargetAuthorityError> {
    require_response_route(&response, expected)?;
    if !response.status().is_success() {
        return Err(http_error(response, operation).await);
    }
    read_json_with_limit(response, operation, maximum_bytes).await
}

pub(in crate::native_v2_target::controller_authority) async fn http_error(
    response: reqwest::Response,
    operation: &'static str,
) -> TargetAuthorityError {
    let status = response.status();
    let status_code = status.as_u16();
    let fallback = || {
        TargetAuthorityError::remote(
            default_http_error_code(status),
            format!("{operation} request failed with status {status_code}"),
            Some(http_error_details(status_code, None)),
        )
    };
    let problem = match read_json::<TargetHttpProblem>(response, operation).await {
        Ok(problem) => problem,
        Err(_) => return fallback(),
    };
    let (code, message, details) = problem.into_parts();
    let disposition = if status.is_client_error() {
        "was rejected"
    } else {
        "failed"
    };
    TargetAuthorityError::remote(
        code,
        format!("{operation} request {disposition}: {message}"),
        Some(http_error_details(status_code, details)),
    )
}

fn default_http_error_code(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "invalid_request",
        StatusCode::UNAUTHORIZED => "unauthorized",
        StatusCode::FORBIDDEN => "forbidden",
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::CONFLICT => "request_conflict",
        StatusCode::TOO_MANY_REQUESTS => "rate_limited",
        StatusCode::SERVICE_UNAVAILABLE => "target.unavailable",
        _ => "target.http_error",
    }
}

fn http_error_details(status: u16, details: Option<Value>) -> Value {
    let mut details = match details {
        Some(Value::Object(details)) => details,
        Some(details) => Map::from_iter([("nativeDetails".to_owned(), details)]),
        None => Map::new(),
    };
    details.insert("httpStatus".to_owned(), json!(status));
    Value::Object(details)
}
