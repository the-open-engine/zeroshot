use super::*;
use openengine_cluster_protocol::WorkerFailureReason;

fn node() -> GraphNode {
    serde_json::from_value(serde_json::json!({
        "kind":"loop","name":"repeat","state":{"kind":"record","fields":{}},
        "maxIterations":65536,"promotedStatePaths":[],
        "body":{"kind":"step","name":"work","worker":"worker.test@1",
            "input":{"kind":"null"},"output":{"kind":"null"},
            "inputBindings":[],"writeBindings":[],"attempts":2}
    }))
    .expect("loop fixture")
}

fn visits(count: u64) -> Vec<DurableExecution> {
    (1..=count)
        .map(|id| DurableExecution {
            dispatch_position: HistoryPosition::new(id * 2).expect("position"),
            node_instance: NodeInstanceId::new(1).expect("instance"),
            execution: ExecutionId::new(id).expect("execution"),
            occurrence: StructuralOccurrence {
                node: "work".parse().expect("name"),
                map_indices: vec![],
            },
            attempt: PositiveInteger::new(1).expect("attempt"),
            input: Value::Null,
            state: DurableExecutionState::Settled {
                position: HistoryPosition::new(id * 2 + 1).expect("position"),
                outcome: WorkerOutcome::Verified {
                    output: Value::Null,
                    artifacts: vec![],
                },
            },
        })
        .collect()
}

fn admission(history: &[DurableExecution], capacity: usize) -> (Result<Status, ReducerError>, u64) {
    let root = node();
    let GraphNode::Loop(group) = &root else {
        unreachable!()
    };
    let count = history.iter().filter(|e| e.attempt.get() == 1).count() as u64;
    let mut engine = Engine::new(
        ReductionInput {
            initial_input: &Value::Null,
            executions: history,
            next_node_instance: 2,
            next_execution: history.len() as u64 + 1,
        },
        &root,
        ExecutionMode::NativeV2,
    )
    .expect("authentic visit/retry history");
    assert_eq!(engine.logical_visits, count);
    engine.dispatch_remaining = capacity;
    let mut context = Context::new(Value::Null);
    mark_visit(group.body.name(), &[], &mut context.controls, count);
    let result = engine.eval(&group.body, &mut context, Traversal::root());
    (result, engine.next_execution)
}

#[test]
fn ledger_visits_allow_last_admission_and_queue_without_identity_or_budget() {
    let mut history = visits(FULL_V1_MAX_NODE_EXECUTIONS - 1);
    let (last, next) = admission(&history, 1);
    assert!(matches!(last, Ok(Status::Pending)));
    assert_eq!(next, FULL_V1_MAX_NODE_EXECUTIONS + 1);
    history = visits(FULL_V1_MAX_NODE_EXECUTIONS);
    let (queued, next) = admission(&history, 0);
    assert!(matches!(queued, Ok(Status::Pending)));
    assert_eq!(next, FULL_V1_MAX_NODE_EXECUTIONS + 1);
    let (excess, next) = admission(&history, 1);
    assert!(matches!(
        excess,
        Err(ReducerError::WorkLimit {
            bound: FULL_V1_MAX_NODE_EXECUTIONS,
            ..
        })
    ));
    assert_eq!(next, FULL_V1_MAX_NODE_EXECUTIONS + 1);
}

#[test]
fn authentic_retry_does_not_precharge_another_logical_visit() {
    let mut history = visits(FULL_V1_MAX_NODE_EXECUTIONS - 1);
    let last = history.last_mut().expect("visit");
    last.state = DurableExecutionState::Settled {
        position: HistoryPosition::new(last.execution.get() * 2 + 1).expect("position"),
        outcome: WorkerOutcome::Error {
            code: WorkerErrorCode::Crash,
            reason: WorkerFailureReason::DeclaredFailure,
        },
    };
    let mut retry = last.clone();
    retry.execution = ExecutionId::new(FULL_V1_MAX_NODE_EXECUTIONS).expect("execution");
    retry.attempt = PositiveInteger::new(2).expect("attempt");
    retry.dispatch_position =
        HistoryPosition::new(FULL_V1_MAX_NODE_EXECUTIONS * 2).expect("position");
    retry.state = DurableExecutionState::Settled {
        position: HistoryPosition::new(FULL_V1_MAX_NODE_EXECUTIONS * 2 + 1).expect("position"),
        outcome: WorkerOutcome::Verified {
            output: Value::Null,
            artifacts: vec![],
        },
    };
    history.push(retry);
    let (result, next) = admission(&history, 1);
    assert!(matches!(result, Ok(Status::Pending)));
    assert_eq!(next, FULL_V1_MAX_NODE_EXECUTIONS + 2);
}
