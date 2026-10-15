use super::*;

fn kind(error: &SourceDeliveryError) -> &'static str {
    match error {
        SourceDeliveryError::Unavailable => "unavailable",
        SourceDeliveryError::Rejected => "rejected",
        SourceDeliveryError::Identity(_) => "identity",
        SourceDeliveryError::Api(_) => "api",
        SourceDeliveryError::Repairable(_) => "repairable",
        SourceDeliveryError::Command(_) => "command",
    }
}

#[test]
fn api_diagnostics_redact_nul_and_truncate_utf8_safely() {
    let SourceDeliveryError::Api(redacted) = SourceDeliveryError::api(Some(500), "visible\0secret")
    else {
        panic!("expected API failure");
    };
    assert_eq!(redacted.to_string(), "source API diagnostic redacted");

    let marker = "\n[diagnostic truncated]";
    let boundary = MAX_SOURCE_API_DIAGNOSTIC_BYTES - marker.len();
    let diagnostic = format!("{}🦀{}", "x".repeat(boundary - 1), "tail".repeat(16));
    let SourceDeliveryError::Api(truncated) = SourceDeliveryError::api(None, diagnostic) else {
        panic!("expected API failure");
    };
    let truncated = truncated.to_string();
    assert!(truncated.len() <= MAX_SOURCE_API_DIAGNOSTIC_BYTES);
    assert!(truncated.ends_with(marker));
    assert!(!truncated.contains('🦀'));
}

#[test]
fn context_preserves_authority_failure_policy() {
    let cases = [
        (
            SourceDeliveryError::identity("wrong review"),
            "identity",
            None,
            false,
            false,
            false,
        ),
        (
            SourceDeliveryError::repairable("conflicted index"),
            "repairable",
            None,
            false,
            false,
            false,
        ),
        (
            SourceDeliveryError::Rejected,
            "api",
            None,
            false,
            false,
            false,
        ),
        (
            SourceDeliveryError::Unavailable,
            "api",
            None,
            true,
            false,
            true,
        ),
        (
            SourceDeliveryError::api(Some(401), "bad credentials"),
            "api",
            Some(401),
            false,
            true,
            false,
        ),
        (
            SourceDeliveryError::api(Some(429), "rate limited"),
            "api",
            Some(429),
            true,
            false,
            true,
        ),
    ];
    for (error, expected_kind, status, review_retry, authentication, operation_retry) in cases {
        let contextual = error.with_context("synchronizing review");
        assert_eq!(kind(&contextual), expected_kind);
        assert_eq!(contextual.api_status(), status);
        assert_eq!(contextual.retryable_review_sync(), review_retry);
        assert_eq!(contextual.authentication_failed(), authentication);
        assert_eq!(contextual.retryable_operation(), operation_retry);
        assert!(contextual.to_string().contains("synchronizing review"));
    }

    let refusal = SourceDeliveryError::Rejected.temporary();
    assert_eq!(kind(&refusal), "rejected");
    assert!(!refusal.retryable_review_sync());
    assert!(!refusal.retryable_operation());
}
