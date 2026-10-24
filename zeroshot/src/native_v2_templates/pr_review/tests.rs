use super::*;
use super::super::review::tests::{dispatch, reduce_with_input, settle};
use openengine_cluster_protocol::{
    IdempotencyKey, RunSubmission, RunTitle, RuntimePlan, TerminalResult, WorkerOutcome,
};
use openengine_cluster_server::admission::VerifiedGraph;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};
use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission};
use crate::full_v1_reducer::{Decision, DurableExecution, Reduction};

const URL: &str = "https://github.com/acme/project/pull/42";

fn input() -> Value {
    json!({"url":URL,"context":{
        "title":"Fix stale writes","body":"Closes #7",
        "reviewScope":{
            "baseRepository":"acme/project","baseBranch":"main","baseRevision":"b".repeat(40),
            "headRepository":"contributor/project","headBranch":"fix","headRevision":"a".repeat(40)
        },
        "linkedIssues":[{"url":"https://github.com/acme/project/issues/7","title":"Prevent stale writes","body":"Reject an outdated version."}]
    }})
}

async fn graph(delivery: TemplateDelivery) -> VerifiedGraph {
    let mut runtime: RuntimePlan = serde_json::from_str(include_str!(
        "../../../../docs/assets/pr-review/runtime.json"
    ))
    .assert_value();
    if delivery == TemplateDelivery::None {
        runtime
            .nodes_mut()
            .remove(&node_name(DELIVERY_NODE).assert_value());
    }
    for (name, binding) in runtime.nodes() {
        if name.as_str() == DELIVERY_NODE {
            continue;
        }
        let value = serde_json::to_value(binding).assert_value();
        assert_eq!(value["model"], "gpt-6.1-sol");
        assert_eq!(value["effort"], "xhigh");
        assert!(matches!(
            binding,
            openengine_cluster_protocol::NodeRuntimeBinding::Agent {
                session_scope: openengine_cluster_protocol::SessionScope::Execution,
                ..
            }
        ));
    }
    let admitted = NativeV2Admission.admit_with_policy(RunSubmission {
        title: RunTitle::new("PR review").assert_value(),
        graph: pr_review_graph(delivery).assert_value(),
        initial_input: input(), runtime,
        source: Some(serde_json::from_value(json!({"repository":"acme/project","branch":"main","revision":"a".repeat(40)})).assert_value()),
        environment: None,
        submission_key: IdempotencyKey::new("pr-review-test").assert_value(),
    }, DeliveryPolicy::Optional).await.assert_value();
    VerifiedGraph {
        compiled_ir: admitted.graph,
        diagnostics: Vec::new(),
    }
}

fn verified(output: Value) -> WorkerOutcome {
    WorkerOutcome::Verifier {
        output,
        artifacts: Vec::new(),
        signals: BTreeMap::new(),
        diagnostic: Value::Null,
    }
}

fn classified(tasks: Value) -> WorkerOutcome {
    verified(json!({TASKS:tasks,REASON:"Behavioral risk determines the required check."}))
}

fn advance(
    graph: &VerifiedGraph,
    history: &mut Vec<DurableExecution>,
    node: &str,
    outcome: WorkerOutcome,
) -> Value {
    let reduction = reduce_with_input(graph, history, &input());
    let action = dispatch(&reduction, node);
    let Decision::Dispatch { input, .. } = action else {
        unreachable!()
    };
    let value = input.clone();
    history.push(settle(action, history.len() as u64 * 2, outcome));
    value
}

fn assert_failed(reduction: Reduction) {
    assert!(matches!(
        reduction.terminal,
        Some(TerminalResult::Failed { .. })
    ));
    assert!(
        !reduction
            .decisions
            .iter()
            .any(|decision| matches!(decision, Decision::Dispatch { .. }))
    );
}

#[tokio::test]
async fn skip_and_required_reproduction_reach_checker_with_exact_context_and_evidence() {
    for delivery in [TemplateDelivery::None, TemplateDelivery::Comment] {
        let graph = graph(delivery).await;
        for required in [false, true] {
            let mut history = Vec::new();
            let tasks = if required {
                json!([{"plan":"Run the local API and database."}])
            } else {
                json!([])
            };
            let classifier_input = advance(&graph, &mut history, CLASSIFIER, classified(tasks));
            assert_eq!(
                classifier_input["context"]["linkedIssues"],
                input()["context"]["linkedIssues"]
            );
            let evidence = "BLOCKED: database could not start: port already in use.";
            if required {
                let dispatched = advance(
                    &graph,
                    &mut history,
                    REPRODUCER,
                    WorkerOutcome::Verified {
                        output: json!({EVIDENCE:evidence}),
                        artifacts: Vec::new(),
                    },
                );
                assert_eq!(dispatched["plan"], "Run the local API and database.");
            }
            let checker_input = advance(
                &graph,
                &mut history,
                CHECKER,
                verified(
                    json!({"body":"No actionable findings. Validation: blocked by unavailable database."}),
                ),
            );
            assert_eq!(
                checker_input[EVIDENCE],
                if required {
                    json!([evidence])
                } else {
                    json!([])
                }
            );
            assert_eq!(checker_input["context"], input()["context"]);
            if delivery == TemplateDelivery::None {
                assert!(matches!(
                    reduce_with_input(&graph, &history, &input()).terminal,
                    Some(TerminalResult::Succeeded { .. })
                ));
            } else {
                let receipt = json!({"receipt":{
                    "version":"v1","mode":"comment","outcome":"posted","destinationUrl":URL,
                    "repository":"acme/project","discussionKind":"pull_request","discussionId":"42",
                    "commentId":"23","url":format!("{URL}#issuecomment-23"),"bodyHash":"c".repeat(64),"sourceRevision":"a".repeat(40)
                }});
                let published = advance(
                    &graph,
                    &mut history,
                    DELIVERY_NODE,
                    WorkerOutcome::Verifier {
                        output: receipt.clone(),
                        artifacts: Vec::new(),
                        signals: BTreeMap::from([(
                            field_name("delivery").assert_value(),
                            enum_label("posted").assert_value(),
                        )]),
                        diagnostic: json!({"message":"Confirmed"}),
                    },
                );
                assert_eq!(published["reviewScope"], input()["context"]["reviewScope"]);
                assert_eq!(
                    reduce_with_input(&graph, &history, &input()).terminal,
                    Some(TerminalResult::Succeeded { output: receipt })
                );
            }
            assert_eq!(
                history
                    .iter()
                    .filter(|execution| execution.occurrence.node.as_str() == REPRODUCER)
                    .count(),
                usize::from(required)
            );
        }
    }
}

#[tokio::test]
async fn classifier_failure_or_multiple_tasks_cannot_publish() {
    let graph = graph(TemplateDelivery::Comment).await;
    for outcome in [
        WorkerOutcome::declared_failure(WorkerErrorCode::Malformed),
        classified(json!([{"plan":"first"},{"plan":"second"}])),
    ] {
        let mut history = Vec::new();
        advance(&graph, &mut history, CLASSIFIER, outcome);
        assert_failed(reduce_with_input(&graph, &history, &input()));
    }
}

#[tokio::test]
async fn mapped_reproducer_runtime_failure_cannot_look_like_skipped_reproduction() {
    let graph = graph(TemplateDelivery::Comment).await;
    for code in [
        WorkerErrorCode::Crash,
        WorkerErrorCode::Malformed,
        WorkerErrorCode::Refusal,
        WorkerErrorCode::Timeout,
    ] {
        let mut history = Vec::new();
        advance(
            &graph,
            &mut history,
            CLASSIFIER,
            classified(json!([{"plan":"Exercise the full stack."}])),
        );
        advance(
            &graph,
            &mut history,
            REPRODUCER,
            WorkerOutcome::declared_failure(code),
        );
        assert_failed(reduce_with_input(&graph, &history, &input()));
    }
}

#[tokio::test]
async fn code_checker_failure_cannot_publish() {
    let graph = graph(TemplateDelivery::Comment).await;
    let mut history = Vec::new();
    advance(&graph, &mut history, CLASSIFIER, classified(json!([])));
    advance(
        &graph,
        &mut history,
        CHECKER,
        WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
    );
    assert_failed(reduce_with_input(&graph, &history, &input()));
}

#[test]
fn pr_and_issue_graphs_require_their_own_prepared_context() {
    let pr = pr_review_graph(TemplateDelivery::Comment).assert_value();
    let issue = super::super::review::issue_review_graph(TemplateDelivery::Comment).assert_value();
    pr.initial_input.validate_value(&input()).assert_value();
    assert!(issue.initial_input.validate_value(&input()).is_err());
    let issue_input = json!({"url":"https://github.com/acme/project/issues/7","context":{"title":"Issue","body":"Requirements","reviewScope":{}}});
    assert!(pr.initial_input.validate_value(&issue_input).is_err());
}
