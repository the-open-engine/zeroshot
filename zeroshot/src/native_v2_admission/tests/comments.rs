use super::*;
use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};
use openengine_cluster_protocol::{NonEmptyVec, PayloadType, RecordField};

fn comment_submission() -> RunSubmission {
    let mut graph = BuiltinGraphTemplate::IssueReview
        .materialize(TemplateDelivery::Comment)
        .assert_value();
    let GraphNode::Seq(root) = &mut graph.root else {
        panic!("review root")
    };
    let GraphNode::Choice(route) = &root.children.as_slice()[1] else {
        panic!("review result")
    };
    let GraphNode::Seq(continuation) = route.otherwise.as_deref().assert_value() else {
        panic!("review continuation")
    };
    root.children = continuation.children.clone();
    let PayloadType::Record { fields } = &mut graph.initial_input else {
        panic!("review input")
    };
    fields.insert(
        openengine_cluster_protocol::FieldName::new("body").assert_value(),
        RecordField {
            value_type: PayloadType::String,
            required: true,
        },
    );
    RunSubmission {
        title: RunTitle::new("Source-less comment").assert_value(),
        graph,
        initial_input: json!({"url":"https://github.com/acme/project/issues/7","body":"A report", "context":{"title":"An issue","body":"Description","reviewScope":{}}}),
        runtime: RuntimePlan::Codex {
            active_memory: Default::default(),
            global_agents: Default::default(),
            provider: CodexProvider::OpenAi,
            size: RunSize::Medium,
            nodes: BTreeMap::from([(
                named("deliver"),
                NodeRuntimeBinding::CommentDelivery {
                    connections: DeclaredConnections::empty(),
                },
            )]),
        },
        source: None,
        environment: None,
        submission_key: IdempotencyKey::new("comment-admission").assert_value(),
    }
}

#[tokio::test]
async fn required_comment_delivery_needs_neither_model_nor_source() {
    let admitted = NativeV2Admission
        .admit_with_policy(comment_submission(), DeliveryPolicy::Required)
        .await
        .assert_value();
    assert!(admitted.source.is_none());
    assert!(admitted.runtime.lanes().is_empty());
    assert_eq!(
        admitted_comment_input(&admitted)
            .assert_value()
            .assert_value()
            .url,
        "https://github.com/acme/project/issues/7"
    );
}

#[tokio::test]
async fn comment_publisher_cannot_supply_a_postmortem_model() {
    let mut submission = comment_submission();
    submission
        .runtime
        .global_agents_mut()
        .postmortem
        .source_node = Some(named("deliver"));
    assert!(matches!(
        NativeV2Admission.admit(submission).await,
        Err(NativeV2AdmissionError::MissingRuntimeBinding { node }) if node == named("deliver")
    ));
}

#[tokio::test]
async fn missing_url_fails_before_execution() {
    let mut submission = comment_submission();
    submission
        .initial_input
        .as_object_mut()
        .assert_value()
        .remove("url");
    assert!(matches!(
        NativeV2Admission.admit(submission).await,
        Err(NativeV2AdmissionError::InitialInput(_))
    ));
}

#[tokio::test]
async fn destination_must_be_a_required_caller_value() {
    let mut submission = comment_submission();
    let PayloadType::Record { fields } = &mut submission.graph.initial_input else {
        panic!("input")
    };
    fields
        .get_mut(&openengine_cluster_protocol::FieldName::new("url").assert_value())
        .assert_value()
        .required = false;
    assert!(matches!(
        NativeV2Admission
            .validate_profile(
                &submission.graph,
                &submission.runtime,
                DeliveryPolicy::Required
            )
            .await,
        Err(NativeV2AdmissionError::InvalidCommentDelivery(_))
    ));
}

#[tokio::test]
async fn publication_cannot_precede_later_executable_work() {
    let mut submission = comment_submission();
    let review = BuiltinGraphTemplate::IssueReview
        .materialize(TemplateDelivery::Comment)
        .assert_value();
    let GraphNode::Seq(original) = review.root else {
        panic!("root")
    };
    let worker = original.children.as_slice()[0].clone();
    submission
        .runtime
        .nodes_mut()
        .insert(named("reviewer"), binding("review-model", None));
    let GraphNode::Seq(root) = &mut submission.graph.root else {
        panic!("root")
    };
    let mut children = root.children.as_slice().to_vec();
    children.insert(1, worker);
    root.children = NonEmptyVec::new(children).assert_value();
    assert!(matches!(
        NativeV2Admission
            .validate_profile(
                &submission.graph,
                &submission.runtime,
                DeliveryPolicy::Required
            )
            .await,
        Err(NativeV2AdmissionError::InvalidCommentDelivery(_))
    ));
}

#[tokio::test]
async fn invalid_review_scope_is_refused_before_execution() {
    for (url, scope) in [
        ("https://github.com/acme/project/pull/7", json!({})),
        (
            "https://github.com/acme/project/pull/7",
            json!({"headRevision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}),
        ),
        (
            "https://github.com/acme/project/issues/7",
            json!({"headRevision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}),
        ),
    ] {
        let mut submission = comment_submission();
        submission.initial_input["url"] = json!(url);
        submission.initial_input["context"]["reviewScope"] = scope;
        assert!(matches!(
            NativeV2Admission.admit(submission).await,
            Err(NativeV2AdmissionError::InvalidCommentDelivery(_))
        ));
    }
}

#[tokio::test]
async fn parallel_publication_is_refused_in_both_branch_orders() {
    for publisher_first in [false, true] {
        let mut submission = comment_submission();
        let review = BuiltinGraphTemplate::IssueReview
            .materialize(TemplateDelivery::Comment)
            .assert_value();
        let GraphNode::Seq(original) = review.root else {
            panic!("root")
        };
        let worker = original.children.as_slice()[0].clone();
        submission
            .runtime
            .nodes_mut()
            .insert(named("reviewer"), binding("review-model", None));
        let GraphNode::Seq(root) = &mut submission.graph.root else {
            panic!("root")
        };
        let comment = root.children.as_slice()[0].clone();
        let branches = if publisher_first {
            vec![comment, worker]
        } else {
            vec![worker, comment]
        };
        let parallel = serde_json::from_value(json!({"kind":"par","name":"unsafe_parallel","state":root.state,"branches":branches,"promotedStatePaths":[],"join":{"kind":"all"}})).assert_value();
        let mut children = root.children.as_slice().to_vec();
        children[0] = parallel;
        root.children = NonEmptyVec::new(children).assert_value();
        assert!(matches!(
            NativeV2Admission
                .validate_profile(
                    &submission.graph,
                    &submission.runtime,
                    DeliveryPolicy::Required
                )
                .await,
            Err(NativeV2AdmissionError::ConcurrentDelivery { .. })
        ));
    }
}
