use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

#[test]
fn fallback_codes_classify_every_actionable_http_status() {
    for (status, expected) in [
        (StatusCode::BAD_REQUEST, "invalid_request"),
        (StatusCode::UNAUTHORIZED, "unauthorized"),
        (StatusCode::FORBIDDEN, "forbidden"),
        (StatusCode::NOT_FOUND, "not_found"),
        (StatusCode::CONFLICT, "request_conflict"),
        (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
        (StatusCode::SERVICE_UNAVAILABLE, "target.unavailable"),
        (StatusCode::IM_A_TEAPOT, "target.http_error"),
    ] {
        assert_eq!(default_http_error_code(status), expected);
    }
}

#[test]
fn http_status_is_added_without_discarding_native_details() {
    assert_eq!(http_error_details(409, None), json!({"httpStatus": 409}));
    assert_eq!(
        http_error_details(429, Some(json!({"retryAfterSeconds": 3}))),
        json!({"httpStatus": 429, "retryAfterSeconds": 3})
    );
    assert_eq!(
        http_error_details(502, Some(json!(["upstream", "timeout"]))),
        json!({
            "httpStatus": 502,
            "nativeDetails": ["upstream", "timeout"],
        })
    );

    let encoded = serde_json::to_string(&http_error_details(
        400,
        Some(json!({"httpStatus": "untrusted"})),
    ))
    .assert_value();
    assert_eq!(encoded, r#"{"httpStatus":400}"#);
}
