use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

fn decode_problem(response: HttpResponse) -> TargetHttpProblem {
    assert_eq!(response.content_type, Some("application/json"));
    assert!(response.no_store);
    serde_json::from_slice(&response.body).assert_value()
}

#[test]
fn authority_failures_are_structured_and_status_specific() {
    let cases = [
        (
            TargetAuthorityError::invalid("invalid target request"),
            400,
            INVALID_REQUEST_CODE,
            "invalid target request",
        ),
        (
            TargetAuthorityError::unauthorized(),
            401,
            UNAUTHORIZED_CODE,
            "unauthorized",
        ),
        (
            TargetAuthorityError::conflict("run already exists"),
            409,
            CONFLICT_CODE,
            "run already exists",
        ),
        (
            TargetAuthorityError::unavailable("internal provider failure"),
            503,
            TARGET_UNAVAILABLE_CODE,
            "target is temporarily unavailable",
        ),
    ];

    for (error, status, code, message) in cases {
        let response = authority_error_response(error);
        assert_eq!(response.status, status);
        let problem = decode_problem(response);
        assert_eq!(problem.code(), code);
        assert_eq!(problem.message(), message);
    }
}

#[test]
fn invalid_public_diagnostic_fails_closed_without_an_empty_body() {
    let response = authority_error_response(TargetAuthorityError::invalid("x".repeat(1_025)));
    assert_eq!(response.status, 400);
    let problem = decode_problem(response);
    assert_eq!(problem.code(), TARGET_INTERNAL_ERROR_CODE);
    assert_eq!(problem.message(), "target request failed");
}
