use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

#[test]
fn target_http_problem_is_bounded_structured_and_closed() {
    let problem = TargetHttpProblem::new(
        TARGET_RUN_REJECTED_CODE,
        "required input binding is missing",
        Some(serde_json::json!({"binding": "issueNumber"})),
    )
    .assert_value();
    let encoded = serde_json::to_value(&problem).assert_value();
    assert_eq!(
        encoded,
        serde_json::json!({
            "code": TARGET_RUN_REJECTED_CODE,
            "message": "required input binding is missing",
            "details": {"binding": "issueNumber"}
        })
    );
    assert!(TargetHttpProblem::new("bad code", "rejected", None).is_err());
    assert!(
        TargetHttpProblem::new(
            TARGET_RUN_REJECTED_CODE,
            "x".repeat(MAX_TARGET_HTTP_PROBLEM_MESSAGE_BYTES + 1),
            None,
        )
        .is_err()
    );
    assert!(
        TargetHttpProblem::new(
            TARGET_RUN_REJECTED_CODE,
            "rejected",
            Some(serde_json::json!(["not", "an", "object"])),
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<TargetHttpProblem>(serde_json::json!({"message": "rejected"}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<TargetHttpProblem>(serde_json::json!({
            "code": TARGET_RUN_REJECTED_CODE,
            "message": "rejected",
            "extra": true
        }))
        .is_err()
    );
}
