use super::*;
use serde_json::json;

#[test]
fn native_auth_and_model_rejections_keep_their_classes() {
    for reason in ["auth", "auth_permanent"] {
        assert!(matches!(
            classify(
                &json!({"failure_reason":reason}),
                "missing-model",
                "denied".into()
            ),
            NodeRunnerError::AuthenticationRequired(_)
        ));
    }
    for reason in ["model_not_found", "model_entitlement"] {
        assert!(
            matches!(classify(&json!({"failure_reason":reason}), "missing-model", "denied".into()),
            NodeRunnerError::ModelUnavailable(detail) if detail.contains("missing-model"))
        );
    }
}

#[test]
fn legacy_errors_and_stream_failures_remain_crashes() {
    for event in [
        json!({"error":"invalid API key"}),
        json!({"failure_reason":"server_error"}),
        json!({"error":"model not found"}),
        json!({"failure_reason":"provider_stream_non_json_data"}),
    ] {
        assert!(matches!(
            classify(&event, "model", "cut stream".into()),
            NodeRunnerError::DriverDetail(_)
        ));
    }
}
