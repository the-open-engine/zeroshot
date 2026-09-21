use super::*;
use openengine_cluster_protocol::WorkerErrorCode;
use zeroshot_engine::full_v1_reducer::ExecutionBoundary;

fn state() -> Value {
    required_array_record(&[("items", "null")])
}

fn step(name: &str) -> Value {
    super::step(name, 2)
}

fn sequence(name: &str, children: Vec<Value>) -> Value {
    json!({"kind":"seq","name":name,"state":state(),"children":children,"promotedStatePaths":[]})
}

fn parallel(name: &str, branches: Vec<Value>) -> Value {
    json!({"kind":"par","name":name,"state":state(),"branches":branches,
        "join":{"kind":"all"},"promotedStatePaths":[]})
}

fn mapped(name: &str, body: Value) -> Value {
    json!({"kind":"map","name":name,"state":state(),
        "over":{"source":"state","path":["items"]},"maxItems":3,
        "body":body,"promotedStatePaths":[]})
}

fn repeated(name: &str, body: Value) -> Value {
    json!({"kind":"loop","name":name,"state":state(),"maxIterations":2,
        "body":body,"promotedStatePaths":[]})
}

async fn verified(children: Vec<Value>) -> VerifiedGraph {
    let mut children = children;
    children.push(json!({"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}));
    super::verified(sequence("root", children), json!({})).await
}

fn reduce(graph: &VerifiedGraph, input: &Value, history: &[DurableExecution]) -> Reduction {
    let (next_node_instance, next_execution) =
        history
            .iter()
            .fold((1_u64, 1_u64), |(node, execution), entry| {
                (
                    node.max(entry.node_instance.get() + 1),
                    execution.max(entry.execution.get() + 1),
                )
            });
    FullV1Reducer::native_v2(graph)
        .reduce(ReductionInput {
            initial_input: input,
            executions: history,
            next_node_instance,
            next_execution,
        })
        .assert_value()
}

fn expected_boundary(name: &str, loops: &[u64], attempt: u64) -> Option<ExecutionBoundary> {
    Some(ExecutionBoundary {
        node: name.parse().assert_value(),
        map_indices: Vec::new(),
        loop_iterations: loops.to_vec(),
        attempt,
    })
}

fn success() -> WorkerOutcome {
    super::success(0)
}

fn settle(
    reduction: &Reduction,
    history: &mut Vec<DurableExecution>,
    limit: usize,
    outcome: WorkerOutcome,
) {
    let dispatches = reduction
        .decisions
        .iter()
        .filter_map(|decision| match decision {
            Decision::Dispatch {
                node_instance,
                execution,
                occurrence,
                attempt,
                input,
                ..
            } => Some((node_instance, execution, occurrence, attempt, input)),
            _ => None,
        });
    for (node_instance, execution, occurrence, attempt, input) in dispatches.take(limit) {
        let position = u64::try_from(history.len()).assert_value() * 2 + 1;
        history.push(DurableExecution {
            dispatch_position: HistoryPosition::new(position).assert_value(),
            node_instance: *node_instance,
            execution: *execution,
            occurrence: occurrence.clone(),
            attempt: *attempt,
            input: input.clone(),
            state: DurableExecutionState::Settled {
                position: HistoryPosition::new(position + 1).assert_value(),
                outcome: outcome.clone(),
            },
        });
    }
}

#[tokio::test]
async fn ordinary_nodes_and_crash_retries_have_distinct_entry_boundaries() {
    let graph = verified(vec![step("write"), verifier("review", 1)]).await;
    let input = json!({"items":[]});
    let mut history = Vec::new();
    let first = reduce(&graph, &input, &history);
    assert_eq!(first.boundary, expected_boundary("write", &[], 1));
    settle(
        &first,
        &mut history,
        1,
        WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
    );
    let retry = reduce(&graph, &input, &history);
    assert_eq!(retry.boundary, expected_boundary("write", &[], 2));
    settle(&retry, &mut history, 1, success());
    let review = reduce(&graph, &input, &history);
    assert_eq!(review.boundary, expected_boundary("review", &[], 1));
    settle(
        &review,
        &mut history,
        1,
        WorkerOutcome::Verifier {
            output: json!({}),
            diagnostic: json!({}),
            artifacts: Vec::new(),
            signals: [(
                "verdict".parse().assert_value(),
                "accepted".parse().assert_value(),
            )]
            .into(),
        },
    );
    let done = reduce(&graph, &input, &history);
    assert!(done.boundary.is_none());
    assert!(done.terminal.is_some());
}

#[tokio::test]
async fn every_map_wave_and_nested_parallel_writer_shares_the_outer_map_entry() {
    let graph = verified(vec![
        mapped(
            "batch",
            parallel("item_writers", vec![step("left"), step("right")]),
        ),
        step("after_batch"),
    ])
    .await;
    let input = json!({"items":[null,null,null]});
    let mut history = Vec::new();
    // A scheduler may admit only one offered writer at a time. Draining those waves must never
    // produce another checkpoint inside this map, even when no writer remains active between them.
    for completed in 0..6 {
        let wave = reduce(&graph, &input, &history);
        assert_eq!(wave.boundary, expected_boundary("batch", &[], 0));
        assert_eq!(
            wave.decisions
                .iter()
                .filter(|d| matches!(d, Decision::Dispatch { .. }))
                .count(),
            6 - completed
        );
        settle(&wave, &mut history, 1, success());
    }
    let after = reduce(&graph, &input, &history);
    assert_eq!(after.boundary, expected_boundary("after_batch", &[], 1));
    let empty = reduce(&graph, &json!({"items":[]}), &[]);
    assert_eq!(empty.boundary, expected_boundary("after_batch", &[], 1));
}

#[tokio::test]
async fn nested_loop_visits_and_maps_do_not_split_an_enclosing_parallel_group() {
    let graph = verified(vec![
        parallel(
            "outer_writers",
            vec![
                sequence(
                    "nested_work",
                    vec![
                        repeated("nested_rounds", step("repeat")),
                        mapped("nested_items", step("item")),
                    ],
                ),
                step("peer"),
            ],
        ),
        step("after_parallel"),
    ])
    .await;
    let input = json!({"items":[null,null]});
    let mut history = Vec::new();
    for _ in 0..5 {
        let wave = reduce(&graph, &input, &history);
        assert_eq!(wave.boundary, expected_boundary("outer_writers", &[], 0));
        settle(&wave, &mut history, 1, success());
    }
    let after = reduce(&graph, &input, &history);
    assert_eq!(after.boundary, expected_boundary("after_parallel", &[], 1));
}

#[tokio::test]
async fn loop_reentry_creates_a_distinct_boundary_for_each_atomic_group_visit() {
    let graph = verified(vec![repeated(
        "rounds",
        parallel("writers", vec![step("a"), step("b")]),
    )])
    .await;
    let input = json!({"items":[]});
    let mut history = Vec::new();
    for iteration in 1..=2 {
        let visit = reduce(&graph, &input, &history);
        assert_eq!(
            visit.boundary,
            expected_boundary("writers", &[iteration], 0)
        );
        settle(&visit, &mut history, usize::MAX, success());
    }
    let done = reduce(&graph, &input, &history);
    assert!(done.boundary.is_none());
    assert!(done.terminal.is_some());
}
