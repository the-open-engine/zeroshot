use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

#[test]
fn api_payload_budget_and_log_tail_are_bounded() {
    let payload = vec![b'x'; 512 * 1024];
    assert_eq!(
        validate_api_output(payload).assert_value().len(),
        512 * 1024
    );
    assert_eq!(
        validate_api_output(vec![b'x'; MAX_API_OUTPUT_BYTES + 1]),
        Err(GitHubAuthorityError::Rejected)
    );

    let mut output = b"discard".to_vec();
    output.extend(vec![b'x'; MAX_CHECK_LOG_TAIL_BYTES]);
    output.extend_from_slice(b"failure at end");
    let tail = check_log_tail(&output);
    assert_eq!(tail.len(), MAX_CHECK_LOG_TAIL_BYTES);
    assert!(!tail.contains("discard"));
    assert!(tail.ends_with("failure at end"));
}

#[test]
fn api_error_parser_preserves_bounded_provider_status_and_reason() {
    let error = github_api_error(
        br#"gh: Validation Failed (HTTP 422)
{
  "message":"Validation Failed",
  "errors":[{
    "resource":"PullRequest",
    "field":"head",
    "code":"invalid",
    "message":"Head sha can't be blank"
  }],
  "status":"422"
}
"#,
    );
    assert_eq!(
        error,
        GitHubAuthorityError::api(
            Some(422),
            concat!(
                "HTTP 422: validation failed; PullRequest head invalid ",
                "pull request head revision is not visible"
            ),
        )
    );
    assert!(error.retryable_review_sync());

    let unauthorized = GitHubAuthorityError::api(Some(401), "HTTP 401: Bad credentials");
    assert!(!unauthorized.retryable_review_sync());
}
