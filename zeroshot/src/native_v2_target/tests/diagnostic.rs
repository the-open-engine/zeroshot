use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::json;
use zeroshot_engine::native_v2_cli::oecp::TargetConnector;

use super::super::*;
use super::fixtures::{FakeAuthority, FakeDialer, MemoryRegistry};

#[test]
fn registry_refusal_diagnostics_are_invalid_requests() {
    for source in [
        TargetConnectorError::NotFound("unknown".to_owned()),
        TargetConnectorError::RecoveryAuthorizationUnavailable,
        TargetConnectorError::RecoveryAuthorizationMismatch,
        TargetConnectorError::RecoveryAuthorizationInvalid,
    ] {
        let message = format!("target operation failed: {source}");
        let error = cli_target_error(source);
        let diagnostic = serde_json::to_value(error.diagnostic()).assert_value();
        assert_eq!(diagnostic["schema"], "zeroshot.error/v1");
        assert_eq!(diagnostic["kind"], "invalid_request", "{message}");
        assert_eq!(diagnostic["code"], "request.invalid", "{message}");
        assert_eq!(diagnostic["message"], message);
        assert_eq!(diagnostic["details"], json!({}));
    }
}

#[tokio::test]
async fn unknown_target_is_rejected_before_contacting_authority() {
    let authority = FakeAuthority::new("wss://target.example/oecp");
    let connector = NativeV2TargetConnector::new(
        MemoryRegistry::default(),
        authority.clone(),
        FakeDialer::default(),
    );
    let error = connector
        .hosted_run_list(
            "unknown",
            openengine_cluster_protocol::RunListParams::default(),
        )
        .await
        .assert_error();
    let diagnostic = serde_json::to_value(error.diagnostic()).assert_value();
    assert_eq!(diagnostic["code"], "request.invalid");
    assert!(authority.calls().is_empty());
}
