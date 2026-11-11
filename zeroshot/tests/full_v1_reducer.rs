use openengine_cluster_protocol::{GraphSpec, PositiveInteger, WorkerOutcome};
use openengine_cluster_server::admission::{GraphVerifier, VerifiedGraph};
use openengine_cluster_server::graph_verifier::ProductionGraphVerifier;
use serde_json::{json, Value};
use zeroshot_engine::full_v1_reducer::{
    Decision, DurableExecution, DurableExecutionState, ExecutionId, ExecutionVoidReason,
    FullV1Reducer, HistoryPosition, NodeInstanceId, ReducerError, Reduction, ReductionInput,
    StructuralOccurrence, TerminalProjection,
};

#[path = "support/impossible_joins.rs"]
mod impossible_joins;
#[path = "support/full_v1_reducer.rs"]
mod reducer_test_support;
use reducer_test_support::{verifier, TestWorkers};

async fn verified(root: Value, _attempts: Value) -> VerifiedGraph {
    let initial_input = root
        .get("state")
        .cloned()
        .unwrap_or_else(|| json!({"kind":"record","fields":{}}));
    reducer_test_support::verified_graph(root, initial_input, true).await
}

fn step(name: &str, attempts: u64) -> Value {
    reducer_test_support::step_node(
        name,
        attempts,
        json!({"kind":"record","fields":{
            "value":{"type":{"kind":"integer"},"required":true}
        }}),
    )
}

fn promoted_integer_step(name: &str, target: &str) -> Value {
    promoted_integer_step_at_path(name, &[target])
}

fn promoted_integer_step_at_path(name: &str, target: &[&str]) -> Value {
    json!({
        "kind":"step","name":name,"worker":"worker.test@1",
        "input":{"kind":"null"},
        "output":{"kind":"record","fields":{"value":{"type":{"kind":"integer"},"required":true}}},
        "inputBindings":[],
        "writeBindings":[{"value":{"node":name,"channel":"out","path":["value"]},"target":target}],
        "timeoutMs":1,"attempts":1
    })
}

fn succeed(name: &str) -> Value {
    json!({"kind":"succeed","name":name,"output":{"kind":"null"},"bindings":[]})
}

fn sequence(name: &str, children: Vec<Value>) -> Value {
    json!({
        "kind":"seq", "name":name, "state":{"kind":"record","fields":{}},
        "children":children, "promotedStatePaths":[]
    })
}

fn required_array_record(fields: &[(&str, &str)]) -> Value {
    let fields = fields
        .iter()
        .map(|(name, item_kind)| {
            (
                (*name).to_owned(),
                json!({
                    "type":{"kind":"array","items":{"kind":item_kind}},
                    "required":true
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    json!({"kind":"record","fields":fields})
}

struct ParallelSequenceSpec<'a> {
    root_name: &'a str,
    parallel_name: &'a str,
    branches: Vec<Value>,
    join: Value,
    terminal_name: &'a str,
    attempts: Value,
}

async fn verified_parallel_sequence(spec: ParallelSequenceSpec<'_>) -> VerifiedGraph {
    verified(
        sequence(
            spec.root_name,
            vec![
                json!({
                    "kind":"par", "name":spec.parallel_name,
                    "state":{"kind":"record","fields":{}},
                    "branches":spec.branches, "promotedStatePaths":[], "join":spec.join
                }),
                succeed(spec.terminal_name),
            ],
        ),
        spec.attempts,
    )
    .await
}

struct SettledSpec<'a> {
    execution: u64,
    node_instance: u64,
    node: &'a str,
    map_indices: Vec<u64>,
    attempt: u64,
    position: u64,
}

impl<'a> SettledSpec<'a> {
    fn new(execution: u64, node_instance: u64, node: &'a str) -> Self {
        Self {
            execution,
            node_instance,
            node,
            map_indices: Vec::new(),
            attempt: 1,
            position: 1,
        }
    }

    fn map_indices(mut self, map_indices: Vec<u64>) -> Self {
        self.map_indices = map_indices;
        self
    }

    fn attempt(mut self, attempt: u64) -> Self {
        self.attempt = attempt;
        self
    }

    fn position(mut self, position: u64) -> Self {
        self.position = position;
        self
    }
}

fn settled(spec: SettledSpec<'_>, outcome: WorkerOutcome) -> DurableExecution {
    DurableExecution {
        delivery_state: None,
        dispatch_position: HistoryPosition::new(spec.position.saturating_sub(1)).assert_value(),
        node_instance: NodeInstanceId::new(spec.node_instance).assert_value(),
        execution: ExecutionId::new(spec.execution).assert_value(),
        occurrence: StructuralOccurrence {
            node: spec.node.parse().assert_value(),
            map_indices: spec.map_indices,
        },
        attempt: PositiveInteger::new(spec.attempt).assert_value(),
        input: Value::Null,
        state: DurableExecutionState::Settled {
            position: HistoryPosition::new(spec.position).assert_value(),
            outcome,
        },
    }
}

fn active(execution: u64, node_instance: u64, node: &str, position: u64) -> DurableExecution {
    DurableExecution {
        delivery_state: None,
        dispatch_position: HistoryPosition::new(position).assert_value(),
        node_instance: NodeInstanceId::new(node_instance).assert_value(),
        execution: ExecutionId::new(execution).assert_value(),
        occurrence: StructuralOccurrence {
            node: node.parse().assert_value(),
            map_indices: Vec::new(),
        },
        attempt: PositiveInteger::new(1).assert_value(),
        input: Value::Null,
        state: DurableExecutionState::Active,
    }
}

fn success(value: i64) -> WorkerOutcome {
    WorkerOutcome::Verified {
        output: json!({"value":value}),
        artifacts: Vec::new(),
    }
}

fn verdict(label: &str) -> WorkerOutcome {
    WorkerOutcome::Verifier {
        output: json!({}),
        signals: [(
            "verdict".parse().assert_value(),
            label.parse().assert_value(),
        )]
        .into_iter()
        .collect(),
        diagnostic: json!({}),
        artifacts: Vec::new(),
    }
}

fn reduce(graph: &VerifiedGraph, input: &Value, executions: &[DurableExecution]) -> Reduction {
    FullV1Reducer::new(graph)
        .reduce(ReductionInput {
            initial_input: input,
            executions,
            next_node_instance: executions
                .iter()
                .map(|execution| execution.node_instance.get())
                .max()
                .unwrap_or(0)
                + 1,
            next_execution: executions
                .iter()
                .map(|execution| execution.execution.get())
                .max()
                .unwrap_or(0)
                + 1,
        })
        .assert_value()
}

#[path = "full_v1_reducer/cases_1.rs"]
mod cases_1;
#[path = "full_v1_reducer/cases_2.rs"]
mod cases_2;
#[path = "full_v1_reducer/cases_3.rs"]
mod cases_3;
#[path = "full_v1_reducer/cases_4.rs"]
mod cases_4;
#[path = "full_v1_reducer/map_collections.rs"]
mod map_collections;
#[path = "full_v1_reducer/trace.rs"]
mod trace;

use openengine_cluster_testkit::assertions::{AssertValue};

#[path = "full_v1_reducer/checkpoints.rs"]
mod checkpoints;

#[path = "full_v1_reducer/dispatch_metadata.rs"]
mod dispatch_metadata;

#[path = "full_v1_reducer/loop_evidence.rs"]
mod loop_evidence;

#[path = "full_v1_reducer/concurrency.rs"]
mod concurrency;

#[tokio::test]
async fn resolved_any_join_releases_losing_frontier_before_tail() {
    let graph = verified(
        sequence(
            "run",
            vec![
                json!({"kind":"par","name":"race","state":{"kind":"record","fields":{}},
            "branches":[step("a",1),step("b",1)],"join":{"kind":"any"},"promotedStatePaths":[]}),
                step("tail", 1),
                succeed("done"),
            ],
        ),
        json!({}),
    )
    .await;
    let history = vec![settled(SettledSpec::new(1, 1, "a").position(2), success(1))];
    for _ in 0..2 {
        let reduced = FullV1Reducer::native_v2(&graph)
            .with_dispatch_limit(1)
            .reduce(ReductionInput {
                initial_input: &json!({}),
                executions: &history,
                next_execution: 2,
                next_node_instance: 2,
            })
            .unwrap();
        assert!(reduced.decisions.iter().any(|decision| matches!(decision,
            Decision::Dispatch { occurrence, .. } if occurrence.node.as_str() == "tail")));
    }
}

async fn repeated_race_graph() -> VerifiedGraph {
    let condition = concurrency::accepted_race_guard();
    verified(sequence("run", vec![json!({"kind":"loop","name":"repeat","state":{"kind":"record","fields":{}},"maxIterations":2,"promotedStatePaths":[],
        "body":{"kind":"par","name":"race","state":{"kind":"record","fields":{}},"branches":[verifier("a",1),verifier("b",1)],"join":{"kind":"first","when":condition},"promotedStatePaths":[]}}), succeed("done")]), json!({})).await
}

#[tokio::test]
async fn repeated_races_revisit_a_previously_voided_loser() {
    let graph = repeated_race_graph().await;
    let mut loser = active(2, 2, "b", 2);
    loser.state = DurableExecutionState::Voided {
        position: HistoryPosition::new(4).unwrap(),
        reason: ExecutionVoidReason::ParallelJoin,
    };
    let mut first = settled(SettledSpec::new(1, 1, "a").position(3), verdict("accepted"));
    first.dispatch_position = HistoryPosition::new(1).unwrap();
    let history = vec![
        first,
        loser,
        settled(SettledSpec::new(3, 1, "a").position(6), verdict("rejected")),
    ];
    let reduced = FullV1Reducer::native_v2(&graph)
        .with_dispatch_limit(1)
        .reduce(ReductionInput {
            initial_input: &json!({}),
            executions: &history,
            next_execution: 4,
            next_node_instance: 3,
        })
        .unwrap();
    assert!(reduced.decisions.iter().any(
        |decision| matches!(decision, Decision::Dispatch { occurrence, attempt, .. }
        if occurrence.node.as_str() == "b" && attempt.get() == 1)
    ));
}

#[tokio::test]
async fn impossible_parallel_join_preserves_an_earlier_map_terminal_cancellation() {
    let state = required_array_record(&[("items", "null")]);
    let fail = |name: &str| json!({"kind":"fail","name":name,"reason":"ended"});
    let graph = reducer_test_support::verified_graph(json!({"kind":"seq","name":"run","state":state,"promotedStatePaths":[],"children":[
        {"kind":"par","name":"parallel","state":state,"promotedStatePaths":[],"join":{"kind":"any"},"branches":[
            {"kind":"map","name":"mapped","state":state,"over":{"source":"state","path":["items"]},"maxItems":2,"promotedStatePaths":[],
                "body":{"kind":"seq","name":"item","state":state,"promotedStatePaths":[],"children":[step("work",1),fail("item_end")]}},
            {"kind":"seq","name":"other_branch","state":state,"promotedStatePaths":[],"children":[step("other",1),fail("other_end")]}
        ]}, fail("finished")
    ]}), state, true).await;
    let first = settled(
        SettledSpec::new(1, 1, "work")
            .map_indices(vec![0])
            .position(4),
        success(1),
    );
    let mut cancelled = active(2, 2, "work", 2);
    cancelled.occurrence.map_indices = vec![1];
    cancelled.state = DurableExecutionState::Voided {
        position: HistoryPosition::new(5).unwrap(),
        reason: ExecutionVoidReason::MapTerminal,
    };
    let other = settled(SettledSpec::new(3, 3, "other").position(6), success(1));
    let history = vec![first, cancelled, other];
    let reduced = FullV1Reducer::native_v2(&graph)
        .reduce(ReductionInput {
            initial_input: &json!({"items":[null,null]}),
            executions: &history,
            next_execution: 4,
            next_node_instance: 4,
        })
        .unwrap();
    assert!(matches!(
        reduced.terminal,
        Some(TerminalProjection::Failed { .. })
    ));
}

#[tokio::test]
async fn repeated_race_does_not_revisit_or_cancel_a_later_rounds_first_execution() {
    let graph = repeated_race_graph().await;
    let mut history = vec![settled(
        SettledSpec::new(1, 1, "a").position(2),
        verdict("accepted"),
    )];
    let reduce = |history: &[DurableExecution], capacity| {
        FullV1Reducer::native_v2(&graph)
            .with_dispatch_limit(capacity)
            .reduce(ReductionInput {
                initial_input: &json!({}),
                executions: history,
                next_execution: history.len() as u64 + 1,
                next_node_instance: history.len() as u64 + 1,
            })
            .unwrap()
    };
    let next = reduce(&history, 1);
    let dispatches = next
        .decisions
        .iter()
        .filter_map(|decision| match decision {
            Decision::Dispatch {
                iteration,
                occurrence,
                ..
            } => Some((*iteration, occurrence)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].0, 2);
    assert_eq!(dispatches[0].1.node.as_str(), "b");
    let mut later = active(2, 2, "b", 3);
    history.push(later.clone());
    let running = reduce(&history, 0);
    assert!(running.terminal.is_none());
    assert!(
        !running.decisions.iter().any(|decision| matches!(
            decision,
            Decision::VoidLoser { .. } | Decision::Dispatch { .. }
        )),
        "later-round work must remain active: {:?}",
        running.decisions
    );
    later.state = DurableExecutionState::Settled {
        position: HistoryPosition::new(4).unwrap(),
        outcome: verdict("accepted"),
    };
    history[1] = later;
    let completed = reduce(&history, 1);
    assert!(
        matches!(
            completed.terminal,
            Some(TerminalProjection::Succeeded { .. })
        ),
        "accepted later-round work must complete the loop: {completed:?}"
    );
    assert!(!completed.decisions.iter().any(|decision| matches!(
        decision,
        Decision::VoidLoser { .. } | Decision::Dispatch { .. }
    )));
}
