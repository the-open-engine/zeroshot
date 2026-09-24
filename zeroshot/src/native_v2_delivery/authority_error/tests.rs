use super::*;

fn kind(error: &GitHubAuthorityError) -> &'static str {
    match error {
        GitHubAuthorityError::Unavailable => "unavailable",
        GitHubAuthorityError::Rejected => "rejected",
        GitHubAuthorityError::Identity(_) => "identity",
        GitHubAuthorityError::Api(_) => "api",
        GitHubAuthorityError::Repairable(_) => "repairable",
        GitHubAuthorityError::Command(_) => "command",
    }
}

#[test]
fn api_diagnostics_redact_nul_and_truncate_utf8_safely() {
    let GitHubAuthorityError::Api(redacted) =
        GitHubAuthorityError::api(Some(500), "visible\0secret")
    else {
        panic!("expected API failure");
    };
    assert_eq!(redacted.to_string(), "GitHub API diagnostic redacted");

    let marker = "\n[diagnostic truncated]";
    let boundary = MAX_GITHUB_API_DIAGNOSTIC_BYTES - marker.len();
    let diagnostic = format!("{}🦀{}", "x".repeat(boundary - 1), "tail".repeat(16));
    let GitHubAuthorityError::Api(truncated) = GitHubAuthorityError::api(None, diagnostic) else {
        panic!("expected API failure");
    };
    let truncated = truncated.to_string();
    assert!(truncated.len() <= MAX_GITHUB_API_DIAGNOSTIC_BYTES);
    assert!(truncated.ends_with(marker));
    assert!(!truncated.contains('🦀'));
}

#[test]
fn context_preserves_authority_failure_policy() {
    let cases = [
        (
            GitHubAuthorityError::identity("wrong review"),
            "identity",
            None,
            false,
            false,
            false,
        ),
        (
            GitHubAuthorityError::repairable("conflicted index"),
            "repairable",
            None,
            false,
            false,
            false,
        ),
        (
            GitHubAuthorityError::Rejected,
            "api",
            None,
            false,
            false,
            false,
        ),
        (
            GitHubAuthorityError::Unavailable,
            "api",
            None,
            true,
            false,
            true,
        ),
        (
            GitHubAuthorityError::api(Some(401), "bad credentials"),
            "api",
            Some(401),
            false,
            true,
            false,
        ),
        (
            GitHubAuthorityError::api(Some(429), "rate limited"),
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

    let refusal = GitHubAuthorityError::Rejected.temporary();
    assert_eq!(kind(&refusal), "rejected");
    assert!(!refusal.retryable_review_sync());
    assert!(!refusal.retryable_operation());
}
