use super::*;

#[test]
fn pr_feedback_defaults_to_consider_and_runtime_can_select_ignore() {
    let default: NodeRuntimeBinding =
        serde_json::from_value(serde_json::json!({"kind":"git_delivery"})).unwrap();
    assert!(matches!(
        default,
        NodeRuntimeBinding::GitDelivery {
            pull_request_feedback: PullRequestFeedback::Consider,
            ..
        }
    ));
    assert_eq!(
        serde_json::to_value(default).unwrap(),
        serde_json::json!({"kind":"git_delivery"})
    );

    let ignored: NodeRuntimeBinding = serde_json::from_value(serde_json::json!({
        "kind":"git_delivery",
        "pullRequestFeedback":"ignore"
    }))
    .unwrap();
    assert!(matches!(
        ignored,
        NodeRuntimeBinding::GitDelivery {
            pull_request_feedback: PullRequestFeedback::Ignore,
            ..
        }
    ));
}
