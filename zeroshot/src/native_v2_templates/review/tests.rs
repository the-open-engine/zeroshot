use super::*;
use openengine_cluster_protocol::{
    CodexProvider, IdempotencyKey, NodeRuntimeBinding, ResolvedSource, RunSize, RunSubmission,
    RunTitle, RuntimePlan, TerminalResult, WorkerOutcome,
};
use openengine_cluster_server::admission::VerifiedGraph;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use crate::full_v1_reducer::{
    Decision, DurableExecution, DurableExecutionState, FullV1Reducer, HistoryPosition, Reduction,
    ReductionInput,
};
use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission};

const URL: &str = "https://github.com/acme/project/issues/42";

fn input() -> Value {
    json!({"url":URL,"context":{"title":"Proposal","body":"Assess this proposal","reviewScope":{}}})
}

fn runtime(delivery: TemplateDelivery) -> RuntimePlan {
    let mut nodes = BTreeMap::from([(
        node_name("reviewer").assert_value(),
        serde_json::from_value::<NodeRuntimeBinding>(
            json!({"kind":"agent","model":"opaque-model"}),
        )
        .assert_value(),
    )]);
    if let Some((name, binding)) = BuiltinGraphTemplate::IssueReview
        .delivery_runtime_binding(delivery)
        .assert_value()
    {
        nodes.insert(name, binding);
    }
    RuntimePlan::Codex {
        active_memory: Default::default(),
        global_agents: Default::default(),
        provider: CodexProvider::OpenAi,
        size: RunSize::Small,
        nodes,
    }
}

#[tokio::test]
async fn review_requires_context_without_a_second_task_and_keeps_native_publication() {
    for delivery in [TemplateDelivery::None, TemplateDelivery::Comment] {
        let graph = issue_review_graph(delivery).assert_value();
        graph.initial_input.validate_value(&input()).assert_value();
        assert!(
            graph
                .initial_input
                .validate_value(&json!({"task":"review it"}))
                .is_err()
        );
        NativeV2Admission
            .validate_profile(&graph, &runtime(delivery), DeliveryPolicy::Optional)
            .await
            .assert_value();
    }
}

#[tokio::test]
async fn complete_review_preserves_exact_receipt_with_and_without_source_provenance() {
    for revision in [None, Some("a".repeat(40))] {
        let source = revision.as_ref().map(|revision| {
            serde_json::from_value::<ResolvedSource>(json!({
                "repository":"acme/project","branch":"main","revision":revision
            }))
            .assert_value()
        });
        let admitted = NativeV2Admission
            .admit_with_policy(
                RunSubmission {
                    title: RunTitle::new("Review receipt regression").assert_value(),
                    graph: issue_review_graph(TemplateDelivery::Comment).assert_value(),
                    initial_input: input(),
                    runtime: runtime(TemplateDelivery::Comment),
                    source,
                    environment: None,
                    submission_key: IdempotencyKey::new("review-receipt").assert_value(),
                },
                DeliveryPolicy::Required,
            )
            .await
            .assert_value();
        let verified = VerifiedGraph {
            compiled_ir: admitted.graph,
            diagnostics: Vec::new(),
        };
        let mut history = Vec::new();
        let first = reduce(&verified, &history);
        let reviewer = dispatch(&first, "reviewer");
        let Decision::Dispatch {
            input: reviewer_input,
            ..
        } = reviewer
        else {
            unreachable!()
        };
        assert_eq!(reviewer_input["instructions"], "");
        history.push(settle(
            reviewer,
            0,
            WorkerOutcome::Verified {
                output: json!({"body":"Found a reproducible defect."}),
                artifacts: Vec::new(),
            },
        ));
        let next = reduce(&verified, &history);
        let publisher = dispatch(&next, DELIVERY_NODE);
        let Decision::Dispatch {
            input: publisher_input,
            ..
        } = publisher
        else {
            unreachable!()
        };
        assert_eq!(
            publisher_input,
            &json!({"url":URL,"body":"Found a reproducible defect.","reviewScope":{}})
        );
        let receipt = receipt(revision.as_deref());
        comment_result_schema()
            .assert_value()
            .validate_value(&receipt)
            .assert_value();
        assert!(
            crate::native_v2_comment_delivery::is_matching_success_receipt(
                &receipt,
                URL,
                revision.as_deref()
            )
        );
        history.push(settle(
            publisher,
            2,
            WorkerOutcome::Verifier {
                output: receipt.clone(),
                artifacts: Vec::new(),
                signals: BTreeMap::from([(
                    field_name("delivery").assert_value(),
                    enum_label("posted").assert_value(),
                )]),
                diagnostic: json!({"message":"Confirmed"}),
            },
        ));
        assert_eq!(
            reduce(&verified, &history).terminal,
            Some(TerminalResult::Succeeded { output: receipt })
        );
    }
}

fn receipt(revision: Option<&str>) -> Value {
    let mut result = json!({"receipt":{
        "version":"v1","mode":"comment","outcome":"posted","destinationUrl":URL,
        "repository":"acme/project","discussionKind":"issue","discussionId":"42",
        "commentId":"23","url":format!("{URL}#issuecomment-23"),"bodyHash":"a".repeat(64)
    }});
    if let Some(revision) = revision {
        result["receipt"]["sourceRevision"] = json!(revision);
    }
    result
}

pub(in crate::native_v2_cli::templates) fn dispatch<'a>(
    reduction: &'a Reduction,
    node: &str,
) -> &'a Decision {
    reduction.decisions.iter().find(|decision| {
        matches!(decision, Decision::Dispatch { occurrence, .. } if occurrence.node.as_str() == node)
    }).assert_value()
}

fn reduce(graph: &VerifiedGraph, history: &[DurableExecution]) -> Reduction {
    reduce_with_input(graph, history, &input())
}

pub(in crate::native_v2_cli::templates) fn reduce_with_input(
    graph: &VerifiedGraph,
    history: &[DurableExecution],
    initial_input: &Value,
) -> Reduction {
    FullV1Reducer::native_v2(graph)
        .reduce(ReductionInput {
            initial_input,
            executions: history,
            next_node_instance: history.len() as u64 + 1,
            next_execution: history.len() as u64 + 1,
        })
        .assert_value()
}

pub(in crate::native_v2_cli::templates) fn settle(
    dispatch: &Decision,
    position: u64,
    outcome: WorkerOutcome,
) -> DurableExecution {
    let Decision::Dispatch {
        node_instance,
        execution,
        occurrence,
        attempt,
        input,
        ..
    } = dispatch
    else {
        panic!("expected dispatch");
    };
    DurableExecution {
        dispatch_position: HistoryPosition::new(position).assert_value(),
        node_instance: *node_instance,
        execution: *execution,
        occurrence: occurrence.clone(),
        attempt: *attempt,
        input: input.clone(),
        state: DurableExecutionState::Settled {
            position: HistoryPosition::new(position + 1).assert_value(),
            outcome,
        },
    }
}
