use std::collections::BTreeSet;

use openengine_cluster_protocol::{IdempotencyKey, RunSubmission, RunTitle};
use openengine_cluster_server::admission::VerifiedGraph;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission};
use crate::native_v2_contract::RunSubmissionIntent;
use crate::native_v2_delivery::{DELIVERY_MERGED_LABEL, DELIVERY_OPENED_LABEL};
use crate::full_v1_reducer::{
    Decision, DurableExecution, FullV1Reducer, Reduction, ReductionInput, TerminalProjection,
};

use super::*;

#[path = "tests/support.rs"]
mod support;
use support::*;

#[test]
fn catalog_and_template_inputs_are_closed() {
    assert_eq!(
        BuiltinGraphTemplate::all(),
        &[
            BuiltinGraphTemplate::SingleWorker,
            BuiltinGraphTemplate::SoftwareChange,
        ]
    );
    assert_eq!(
        BuiltinGraphTemplate::parse("software-change"),
        Some(BuiltinGraphTemplate::SoftwareChange)
    );
    assert_eq!(BuiltinGraphTemplate::parse("unknown"), None);
    assert!(matches!(
        BuiltinGraphTemplate::SingleWorker.materialize(TemplateDelivery::Merge),
        Err(BuiltinTemplateError::UnsupportedDelivery { .. })
    ));

    let authored = json!({"task":"repair checkout"});
    let single = BuiltinGraphTemplate::SingleWorker
        .materialize(TemplateDelivery::None)
        .assert_value();
    single
        .initial_input
        .validate_value(&authored)
        .assert_value();
    let software = BuiltinGraphTemplate::SoftwareChange
        .materialize(TemplateDelivery::None)
        .assert_value();
    software
        .initial_input
        .validate_value(&authored)
        .assert_value();
    assert!(
        software
            .initial_input
            .validate_value(&json!({"task":"repair checkout","acceptanceFeedback":""}))
            .is_err()
    );
    assert!(
        software
            .initial_input
            .validate_value(&json!({"task":"repair checkout","issueNumber":"208"}))
            .is_err()
    );
    let delivered = BuiltinGraphTemplate::SoftwareChange
        .materialize(TemplateDelivery::PullRequest)
        .assert_value();
    delivered
        .initial_input
        .validate_value(&authored)
        .assert_value();
    delivered
        .initial_input
        .validate_value(&json!({"task":"repair checkout","issueNumber":"208"}))
        .assert_value();
}

#[tokio::test]
async fn every_supported_materialization_is_admissible() {
    let cases = vec![
        (
            BuiltinGraphTemplate::SingleWorker,
            TemplateDelivery::None,
            vec!["worker"],
        ),
        (
            BuiltinGraphTemplate::SoftwareChange,
            TemplateDelivery::None,
            vec!["acceptance", "code", "review_repair", "worker"],
        ),
        (
            BuiltinGraphTemplate::SoftwareChange,
            TemplateDelivery::PullRequest,
            vec![
                "acceptance",
                "code",
                "deliver",
                "delivery_repair",
                "review_repair",
                "worker",
            ],
        ),
        (
            BuiltinGraphTemplate::SoftwareChange,
            TemplateDelivery::Merge,
            vec![
                "acceptance",
                "code",
                "deliver",
                "delivery_repair",
                "review_repair",
                "worker",
            ],
        ),
    ];

    for (template, delivery, expected) in cases {
        assert_admissible(template, delivery, &expected).await;
    }
}

#[test]
fn software_change_has_one_global_ten_cycle_budget() {
    for delivery in [
        TemplateDelivery::None,
        TemplateDelivery::PullRequest,
        TemplateDelivery::Merge,
    ] {
        let graph = BuiltinGraphTemplate::SoftwareChange
            .materialize(delivery)
            .assert_value();
        let loops = all_nodes(&graph.root)
            .into_iter()
            .filter_map(|node| match node {
                GraphNode::Loop(loop_node) => Some(loop_node),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(loops.len(), 1, "software-change must contain one loop");
        let change_loop = loops.first().assert_value_with("software-change loop");
        assert_eq!(change_loop.name.as_str(), "change_loop");
        assert_eq!(change_loop.max_iterations.get(), 10);
        assert!(change_loop.until.is_none());
    }
}

#[tokio::test]
async fn rejected_parallel_reviews_dispatch_repair_with_both_diagnostics() {
    let (verified, initial_input) = verified_software_template(TemplateDelivery::None).await;
    let history = [
        settled_worker(),
        settled_review(
            2,
            "acceptance",
            REJECTED_LABEL,
            "missing requested behavior",
        ),
        settled_review(3, "code", REJECTED_LABEL, "unsafe error handling"),
    ];
    let reduction = reduce(&verified, &initial_input, &history);

    assert!(reduction.decisions.iter().any(|decision| matches!(
        decision,
        Decision::Dispatch { occurrence, input, .. }
            if occurrence.node.as_str() == "review_repair"
                && input == &json!({
                    "task":"repair checkout",
                    "acceptanceFeedback":"missing requested behavior",
                    "codeFeedback":"unsafe error handling",
                    "deliveryFeedback":""
                })
    )));
}

fn crashed_parallel_review_history() -> Vec<DurableExecution> {
    let review_input = json!({"task":"repair checkout","deliveryFeedback":""});
    vec![
        settled_worker(),
        settled_review(
            2,
            "acceptance",
            REJECTED_LABEL,
            "missing requested behavior",
        ),
        settled_failure(
            SettledExecutionSpec {
                execution: 3,
                node_instance: 3,
                node: "code",
                settled_at: 3,
                input: review_input,
            },
            WorkerErrorCode::Crash,
        ),
    ]
}

#[tokio::test]
async fn crashed_parallel_review_retries_only_the_failed_verifier() {
    let (verified, initial_input) = verified_software_template(TemplateDelivery::None).await;
    let mut history = crashed_parallel_review_history();
    let reduction = reduce(&verified, &initial_input, &history);

    assert_eq!(
        reduction
            .decisions
            .iter()
            .filter(|decision| matches!(decision, Decision::Dispatch { .. }))
            .count(),
        1
    );
    assert!(reduction.decisions.iter().any(|decision| matches!(
        decision,
        Decision::Dispatch { occurrence, attempt, input, .. }
            if occurrence.node.as_str() == "code"
                && attempt.get() == MAX_AGENT_VERIFIER_ATTEMPTS
                && input == &json!({"task":"repair checkout","deliveryFeedback":""})
    )));
    assert!(reduction.terminal.is_none());

    let mut recovered = settled_review_execution(
        SettledExecutionSpec {
            execution: 4,
            node_instance: 3,
            node: "code",
            settled_at: 5,
            input: json!({"task":"repair checkout","deliveryFeedback":""}),
        },
        ACCEPTED_LABEL,
        "implementation sound",
    );
    recovered.attempt = PositiveInteger::new(MAX_AGENT_VERIFIER_ATTEMPTS).assert_value();
    history.push(recovered);
    let reviewed = reduce(&verified, &initial_input, &history);
    assert_dispatched_together(&reviewed, &["review_repair"]);
    let repair_input = json!({
        "task":"repair checkout",
        "acceptanceFeedback":"missing requested behavior",
        "codeFeedback":"implementation sound",
        "deliveryFeedback":""
    });
    assert!(reviewed.decisions.iter().any(|decision| matches!(
        decision, Decision::Dispatch { input, .. } if input == &repair_input
    )));
    history.push(settled_agent(SettledExecutionSpec {
        execution: 5,
        node_instance: 4,
        node: "review_repair",
        settled_at: 6,
        input: repair_input,
    }));
    let next_round = reduce(&verified, &initial_input, &history);
    assert_dispatched_together(&next_round, &["acceptance", "code"]);
    assert!(next_round.decisions.iter().all(|decision| match decision {
        Decision::Dispatch { attempt, .. } => attempt.get() == 1,
        _ => true,
    }));
}

#[tokio::test]
async fn repeatedly_crashed_parallel_review_fails_after_the_retry_limit() {
    let (verified, initial_input) = verified_software_template(TemplateDelivery::None).await;
    let review_input = json!({"task":"repair checkout","deliveryFeedback":""});
    let mut history = crashed_parallel_review_history();
    history.push(settled_failure_attempt(
        SettledExecutionSpec {
            execution: 4,
            node_instance: 3,
            node: "code",
            settled_at: 5,
            input: review_input,
        },
        WorkerErrorCode::Crash,
        MAX_AGENT_VERIFIER_ATTEMPTS,
    ));

    assert_eq!(
        reduce(&verified, &initial_input, &history).terminal,
        Some(TerminalProjection::Failed {
            reason: "review_failed".parse().assert_value()
        })
    );
}

#[tokio::test]
async fn accepted_second_review_round_ignores_stale_sibling_verdicts() {
    let (verified, initial_input) = verified_software_template(TemplateDelivery::None).await;
    let review_input = json!({"task":"repair checkout","deliveryFeedback":""});
    let mut history = vec![
        settled_worker(),
        settled_review(2, "acceptance", ACCEPTED_LABEL, "requirements met"),
        settled_review(3, "code", REJECTED_LABEL, "unsafe error handling"),
        settled_agent(SettledExecutionSpec {
            execution: 4,
            node_instance: 4,
            node: "review_repair",
            settled_at: 4,
            input: json!({
                "task":"repair checkout",
                "acceptanceFeedback":"requirements met",
                "codeFeedback":"unsafe error handling",
                "deliveryFeedback":""
            }),
        }),
    ];
    assert_dispatched_together(
        &reduce(&verified, &initial_input, &history),
        &["acceptance", "code"],
    );

    history.extend([
        settled_review_execution(
            SettledExecutionSpec {
                execution: 5,
                node_instance: 2,
                node: "acceptance",
                settled_at: 6,
                input: review_input.clone(),
            },
            ACCEPTED_LABEL,
            "repaired change meets requirements",
        ),
        settled_review_execution(
            SettledExecutionSpec {
                execution: 6,
                node_instance: 3,
                node: "code",
                settled_at: 5,
                input: review_input,
            },
            ACCEPTED_LABEL,
            "repaired implementation is sound",
        ),
    ]);

    assert_eq!(
        reduce(&verified, &initial_input, &history).terminal,
        Some(TerminalProjection::Succeeded {
            output: serde_json::Value::Null,
        })
    );
}

#[tokio::test]
async fn accepted_reviews_complete_or_dispatch_pull_request_delivery() {
    for delivery in [TemplateDelivery::None, TemplateDelivery::PullRequest] {
        let (verified, initial_input) = verified_software_template(delivery).await;
        let reviews = accepted_review_history(delivery);
        let worker_history = [settled_worker()];
        let after_worker = reduce(&verified, &initial_input, &worker_history);
        assert_dispatched_together(&after_worker, &["acceptance", "code"]);
        let reviewed = reduce(&verified, &initial_input, &reviews);

        if delivery == TemplateDelivery::None {
            assert_eq!(
                reviewed.terminal,
                Some(TerminalProjection::Succeeded {
                    output: serde_json::Value::Null,
                })
            );
            continue;
        }
        assert_dispatched_together(&reviewed, &[DELIVERY_NODE]);
        let receipt = delivery_receipt(DeliveryMode::PullRequest, DELIVERY_OPENED_LABEL);
        let mut delivered = reviews;
        delivered.push(settled_delivery(
            SettledExecutionSpec {
                execution: 4,
                node_instance: 4,
                node: DELIVERY_NODE,
                settled_at: 4,
                input: delivery_input(),
            },
            DeliveryMode::PullRequest,
            DELIVERY_OPENED_LABEL,
        ));
        assert_eq!(
            reduce(&verified, &initial_input, &delivered).terminal,
            Some(TerminalProjection::Succeeded { output: receipt })
        );
    }
}

#[tokio::test]
async fn merge_delivery_repairs_recoverable_outcomes_then_returns_the_receipt() {
    for recoverable in [
        DELIVERY_CI_FAILED_LABEL,
        DELIVERY_CONFLICT_LABEL,
        DELIVERY_REPAIR_REQUIRED_LABEL,
    ] {
        assert_recoverable_delivery(recoverable).await;
    }
}

async fn assert_recoverable_delivery(recoverable: &str) {
    let delivery_feedback = format!("trusted delivery reported {recoverable}");
    let (verified, initial_input) = verified_software_template(TemplateDelivery::Merge).await;
    let mut history = accepted_review_history(TemplateDelivery::Merge);
    let repair_input = json!({
        "task":"repair checkout",
        "outcome":recoverable,
        "deliveryFeedback":delivery_feedback
    });
    history.push(settled_delivery_with_diagnostic(
        SettledExecutionSpec {
            execution: 4,
            node_instance: 4,
            node: DELIVERY_NODE,
            settled_at: 4,
            input: delivery_input(),
        },
        DeliveryMode::Merge,
        recoverable,
        &delivery_feedback,
    ));
    assert_dispatch(
        &reduce(&verified, &initial_input, &history),
        "delivery_repair",
        &repair_input,
    );
    history.push(settled_agent(SettledExecutionSpec {
        execution: 5,
        node_instance: 5,
        node: "delivery_repair",
        settled_at: 5,
        input: repair_input,
    }));
    assert_repaired_reviews_then_merge(&verified, &initial_input, &mut history, &delivery_feedback);
}

fn assert_repaired_reviews_then_merge(
    verified: &VerifiedGraph,
    initial_input: &Value,
    history: &mut Vec<DurableExecution>,
    delivery_feedback: &str,
) {
    let repaired = reduce(verified, initial_input, history);
    assert_dispatched_together(&repaired, &["acceptance", "code"]);
    let review_input = json!({
        "task":"repair checkout",
        "deliveryFeedback":delivery_feedback
    });
    assert_dispatch(&repaired, "acceptance", &review_input);
    assert_dispatch(&repaired, "code", &review_input);
    history.push(settled_review_execution_with_output(
        SettledExecutionSpec {
            execution: 6,
            node_instance: 2,
            node: "acceptance",
            settled_at: 6,
            input: review_input.clone(),
        },
        ACCEPTED_LABEL,
        "CI repair meets the request",
        repaired_change_manifest(),
    ));
    history.push(settled_review_execution(
        SettledExecutionSpec {
            execution: 7,
            node_instance: 3,
            node: "code",
            settled_at: 7,
            input: review_input,
        },
        ACCEPTED_LABEL,
        "CI repair is sound",
    ));
    assert_dispatch(
        &reduce(verified, initial_input, history),
        DELIVERY_NODE,
        &repaired_delivery_input(),
    );
    history.push(settled_delivery(
        SettledExecutionSpec {
            execution: 8,
            node_instance: 4,
            node: DELIVERY_NODE,
            settled_at: 8,
            input: repaired_delivery_input(),
        },
        DeliveryMode::Merge,
        DELIVERY_MERGED_LABEL,
    ));
    let receipt = delivery_receipt(DeliveryMode::Merge, DELIVERY_MERGED_LABEL);
    assert_eq!(
        reduce(verified, initial_input, history).terminal,
        Some(TerminalProjection::Succeeded { output: receipt })
    );
}

async fn assert_admissible(
    template: BuiltinGraphTemplate,
    delivery: TemplateDelivery,
    expected: &[&str],
) {
    let graph = template.materialize(delivery).assert_value();
    let leaves = executable_leaves(&graph.root);
    let names = leaves
        .iter()
        .map(|node| node.name().as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(names, expected.iter().copied().collect());
    assert_instruction_ownership(&leaves);

    let runtime = runtime_for(template, delivery, &leaves);
    let initial_input = json!({"task":"implement the requested change"});
    let policy = if delivery == TemplateDelivery::None {
        DeliveryPolicy::Optional
    } else {
        DeliveryPolicy::Required
    };
    NativeV2Admission
        .validate_intent(
            &RunSubmissionIntent {
                title: RunTitle::new("Built-in template admission").assert_value(),
                graph,
                initial_input,
                runtime,
                branch: None,
                submission_key: IdempotencyKey::new(format!(
                    "template-{}-{}",
                    template.name(),
                    delivery.name()
                ))
                .assert_value(),
            },
            policy,
        )
        .await
        .assert_value();
}

async fn verified_software_template(
    delivery: TemplateDelivery,
) -> (
    openengine_cluster_server::admission::VerifiedGraph,
    serde_json::Value,
) {
    let template = BuiltinGraphTemplate::SoftwareChange;
    let graph = template.materialize(delivery).assert_value();
    let runtime = runtime_for(template, delivery, &executable_leaves(&graph.root));
    let initial_input = if delivery == TemplateDelivery::None {
        json!({"task":"repair checkout"})
    } else {
        json!({"task":"repair checkout","issueNumber":"208"})
    };
    let admitted = NativeV2Admission
        .admit(RunSubmission {
            title: RunTitle::new("Template behavior").assert_value(),
            graph,
            initial_input: initial_input.clone(),
            runtime,
            source: resolved_source(),
            submission_key: IdempotencyKey::new(format!("template-behavior-{}", delivery.name()))
                .assert_value(),
        })
        .await
        .assert_value();
    (
        openengine_cluster_server::admission::VerifiedGraph {
            compiled_ir: admitted.graph,
            diagnostics: Vec::new(),
        },
        initial_input,
    )
}

fn reduce(
    verified: &openengine_cluster_server::admission::VerifiedGraph,
    initial_input: &serde_json::Value,
    history: &[DurableExecution],
) -> Reduction {
    let next_node_instance = history
        .iter()
        .map(|execution| execution.node_instance.get())
        .max()
        .unwrap_or(0)
        + 1;
    let next_execution = history
        .iter()
        .map(|execution| execution.execution.get())
        .max()
        .unwrap_or(0)
        + 1;
    FullV1Reducer::native_v2(verified)
        .reduce(ReductionInput {
            initial_input,
            executions: history,
            next_node_instance,
            next_execution,
        })
        .assert_value()
}

fn assert_dispatched_together(reduction: &Reduction, expected: &[&str]) {
    let dispatched = reduction
        .decisions
        .iter()
        .filter_map(|decision| match decision {
            Decision::Dispatch { occurrence, .. } => Some(occurrence.node.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(dispatched, expected.iter().copied().collect());
}

fn assert_dispatch(reduction: &Reduction, node: &str, expected_input: &serde_json::Value) {
    assert!(
        reduction.decisions.iter().any(|decision| matches!(
            decision,
            Decision::Dispatch { occurrence, input, .. }
                if occurrence.node.as_str() == node && input == expected_input
        )),
        "expected {node} input {expected_input}; decisions: {:?}",
        reduction.decisions
    );
}

fn all_nodes(root: &GraphNode) -> Vec<&GraphNode> {
    let mut nodes = Vec::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        nodes.push(node);
        pending.extend(openengine_cluster_server::graph_verifier::graph_node_children(node));
    }
    nodes
}

#[tokio::test]
async fn pull_request_delivery_routes_a_pre_review_git_failure_to_repair() {
    let (verified, initial_input) = verified_software_template(TemplateDelivery::PullRequest).await;
    let mut history = accepted_review_history(TemplateDelivery::PullRequest);
    history.push(settled_delivery_with_diagnostic(
        SettledExecutionSpec {
            execution: 4,
            node_instance: 4,
            node: DELIVERY_NODE,
            settled_at: 4,
            input: delivery_input(),
        },
        DeliveryMode::PullRequest,
        DELIVERY_REPAIR_REQUIRED_LABEL,
        "git push: unfamiliar failure",
    ));
    assert_dispatch(
        &reduce(&verified, &initial_input, &history),
        "delivery_repair",
        &json!({
            "task":"repair checkout", "outcome":"repair_required",
            "deliveryFeedback":"git push: unfamiliar failure"
        }),
    );
}
