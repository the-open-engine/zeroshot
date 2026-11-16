use super::*;
use crate::full_v1_reducer::{DurableExecutionState, HistoryPosition};

fn error_reads(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::Object(fields) => {
            if fields.get("source").and_then(Value::as_str) == Some("error") {
                names.insert(fields["name"].as_str().assert_value().to_owned());
            }
            for value in fields.values() {
                error_reads(value, names);
            }
        }
        Value::Array(values) => {
            for value in values {
                error_reads(value, names);
            }
        }
        _ => {}
    }
}

#[test]
fn every_builtin_template_explicitly_handles_each_executable_error() {
    for template in BuiltinGraphTemplate::all() {
        for delivery in [
            TemplateDelivery::None,
            TemplateDelivery::Push,
            TemplateDelivery::PullRequest,
            TemplateDelivery::Merge,
        ] {
            let Ok(graph) = template.materialize(delivery) else {
                continue;
            };
            let mut reads = BTreeSet::new();
            error_reads(&serde_json::to_value(&graph).assert_value(), &mut reads);
            let unchecked = executable_leaves(&graph.root)
                .iter()
                .filter(|node| !reads.contains(node.name().as_str()))
                .map(|node| node.name().as_str().to_owned())
                .collect::<BTreeSet<_>>();
            assert!(
                unchecked.is_empty(),
                "{} / {} unchecked: {unchecked:?}",
                template.name(),
                delivery.name()
            );
        }
    }
}

#[tokio::test]
async fn documented_review_loop_stops_on_an_unchecked_worker_error_in_round_one() {
    let graph: openengine_cluster_protocol::GraphSpec = serde_json::from_str(include_str!(
        "../../../../docs/assets/review-loop/review-loop.graph.json"
    ))
    .assert_value();
    let mut reads = BTreeSet::new();
    error_reads(&serde_json::to_value(&graph).assert_value(), &mut reads);
    assert!(reads.is_empty());
    let runtime = serde_json::from_str(include_str!(
        "../../../../docs/assets/review-loop/review-loop.runtime.json"
    ))
    .assert_value();
    let initial_input = json!({"task":"repair checkout"});
    let admitted = NativeV2Admission
        .admit(RunSubmission {
            environment: None,
            title: RunTitle::new("Documented review loop").assert_value(),
            graph,
            initial_input: initial_input.clone(),
            runtime,
            source: None,
            submission_key: IdempotencyKey::new("documented-unhandled").assert_value(),
        })
        .await
        .assert_value();
    let graph = VerifiedGraph {
        compiled_ir: admitted.graph,
        diagnostics: vec![],
    };
    let reducer = FullV1Reducer::native_v2(&graph);
    let first = reducer
        .reduce(ReductionInput {
            initial_input: &initial_input,
            executions: &[],
            next_node_instance: 1,
            next_execution: 1,
        })
        .assert_value();
    let Decision::Dispatch {
        node_instance,
        execution,
        occurrence,
        attempt,
        iteration,
        input,
        ..
    } = first
        .decisions
        .into_iter()
        .find(|decision| matches!(decision, Decision::Dispatch { .. }))
        .assert_value()
    else {
        panic!("missing worker dispatch");
    };
    assert_eq!(occurrence.node.as_str(), "worker");
    assert_eq!(iteration, 1);
    let history = [DurableExecution {
        delivery_state: None,
        node_instance,
        execution,
        occurrence,
        attempt,
        input,
        dispatch_position: HistoryPosition::new(1).assert_value(),
        state: DurableExecutionState::Settled {
            position: HistoryPosition::new(2).assert_value(),
            outcome: WorkerOutcome::declared_failure(WorkerErrorCode::Refusal),
        },
    }];
    let result = reducer
        .reduce(ReductionInput {
            initial_input: &initial_input,
            executions: &history,
            next_node_instance: 2,
            next_execution: 2,
        })
        .assert_value();
    assert!(
        matches!(result.terminal, Some(TerminalProjection::Failed { reason, .. }) if reason.as_str()=="unhandled")
    );
    assert!(
        !result
            .decisions
            .iter()
            .any(|decision| matches!(decision, Decision::Dispatch { .. }))
    );
}
