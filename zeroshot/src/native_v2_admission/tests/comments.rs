use super::*;
use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};
use openengine_cluster_protocol::{NonEmptyVec, PayloadType, RecordField, VerifierNode};

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

fn publisher_mut(submission: &mut RunSubmission) -> &mut VerifierNode {
    let GraphNode::Seq(root) = &mut submission.graph.root else {
        panic!("comment fixture root")
    };
    let GraphNode::Verifier(publisher) = &mut root.children.as_mut_slice()[0] else {
        panic!("comment fixture publisher")
    };
    publisher
}

async fn reject_comment_profile(submission: RunSubmission, expected: &str) {
    match NativeV2Admission
        .validate_profile(
            &submission.graph,
            &submission.runtime,
            DeliveryPolicy::Required,
        )
        .await
    {
        Err(NativeV2AdmissionError::InvalidCommentDelivery(message)) => {
            assert_eq!(message, expected)
        }
        other => panic!("expected comment admission rejection, received {other:?}"),
    }
}

#[tokio::test]
async fn publisher_destination_cannot_be_missing_or_selected_from_a_map_item() {
    for missing in [false, true] {
        let mut submission = comment_submission();
        let bindings = &mut publisher_mut(&mut submission).input_bindings;
        let expected = if missing {
            bindings.retain(|binding| binding.target.segments()[0].as_str() != "url");
            "comment URL requires a caller input binding"
        } else {
            bindings[0].value =
                serde_json::from_value(json!({"source":"item","path":["url"]})).assert_value();
            "comment destination and scope must bind directly from caller input"
        };
        reject_comment_profile(submission, expected).await;
    }
}

#[tokio::test]
async fn publisher_destination_cannot_select_an_undefined_or_non_record_input_path() {
    for path in [vec!["notDeclared"], vec!["body", "url"]] {
        let mut submission = comment_submission();
        publisher_mut(&mut submission).input_bindings[0].value =
            serde_json::from_value(json!({"source":"state","path":path})).assert_value();
        reject_comment_profile(
            submission,
            "comment URL must select a required string in initial input",
        )
        .await;
    }
}

#[tokio::test]
async fn captured_review_scope_requires_each_input_segment_and_cannot_use_a_map_item() {
    for scope_path in ["context", "reviewScope", "item"] {
        let mut submission = comment_submission();
        let expected = if scope_path == "item" {
            publisher_mut(&mut submission).input_bindings[2].value =
                serde_json::from_value(json!({"source":"item","path":["reviewScope"]}))
                    .assert_value();
            "comment destination and scope must bind directly from caller input"
        } else {
            let mut input = serde_json::to_value(&submission.graph.initial_input).assert_value();
            if scope_path == "context" {
                input["fields"]["context"]["required"] = json!(false);
            } else {
                input["fields"]["context"]["type"]["fields"]["reviewScope"]["required"] =
                    json!(false);
            }
            submission.graph.initial_input = serde_json::from_value(input).assert_value();
            "comment reviewScope must select a required initial input value"
        };
        reject_comment_profile(submission, expected).await;
    }
}

#[tokio::test]
async fn custom_issue_publisher_can_omit_a_scope_binding_and_preserves_caller_input() {
    let mut submission = comment_submission();
    publisher_mut(&mut submission)
        .input_bindings
        .retain(|binding| binding.target.segments()[0].as_str() != "reviewScope");
    let input = submission.initial_input.clone();
    let admitted = NativeV2Admission.admit(submission).await.assert_value();
    let destination = admitted_comment_input(&admitted)
        .assert_value()
        .assert_value();
    assert_eq!(destination.url, input["url"].as_str().assert_value());
    assert!(destination.review_scope.is_none());
    assert_eq!(admitted.initial_input, input);
}

#[tokio::test]
async fn publisher_cannot_execute_inside_a_loop_or_even_a_single_item_map() {
    for kind in ["loop", "map"] {
        let mut submission = comment_submission();
        let mut graph = serde_json::to_value(&submission.graph).assert_value();
        let items = json!({"type":{"kind":"array","items":{"kind":"null"}},"required":true});
        graph["initialInput"]["fields"]["items"] = items.clone();
        graph["root"]["state"]["fields"]["items"] = items;
        let mut group = json!({
            "kind":kind,"name":"repeated_publication","state":graph["root"]["state"],
            "body":graph["root"]["children"][0],"promotedStatePaths":[]
        });
        if kind == "loop" {
            group["maxIterations"] = json!(1);
        } else {
            group["maxItems"] = json!(1);
            group["over"] = json!({"source":"state","path":["items"]});
        }
        graph["root"]["children"][0] = group;
        submission.graph = serde_json::from_value(graph).assert_value();
        submission.initial_input["items"] = json!([null]);
        reject_comment_profile(
            submission,
            "comment delivery cannot execute inside a loop or map",
        )
        .await;
    }
}

#[tokio::test]
async fn publication_cannot_precede_a_verifier_in_its_terminal_choice() {
    let mut submission = comment_submission();
    submission
        .runtime
        .nodes_mut()
        .insert(named("late_check"), binding("review-model", None));
    let GraphNode::Seq(root) = &mut submission.graph.root else {
        panic!("comment fixture root")
    };
    let GraphNode::Choice(result) = &mut root.children.as_mut_slice()[1] else {
        panic!("comment fixture terminal choice")
    };
    result.otherwise = Some(Box::new(
        serde_json::from_value(null_verifier("late_check", "agent.late-check@1")).assert_value(),
    ));
    reject_comment_profile(
        submission,
        "comment delivery must be the final executable node",
    )
    .await;
}

#[tokio::test]
async fn publisher_contract_cannot_redefine_its_input_receipt_signals_or_diagnostics() {
    for field in ["input", "output", "signals", "diagnostic"] {
        let mut submission = comment_submission();
        let publisher = publisher_mut(&mut submission);
        match field {
            "input" => publisher.input = PayloadType::Null,
            "output" => publisher.output = PayloadType::Null,
            "signals" => publisher.signals.clear(),
            _ => publisher.diagnostic = PayloadType::Null,
        }
        assert!(matches!(
            NativeV2Admission.validate_profile(
                &submission.graph, &submission.runtime, DeliveryPolicy::Required,
            ).await,
            Err(NativeV2AdmissionError::InvalidDeliveryContract { node, .. }) if node == named("deliver")
        ));
    }
}
