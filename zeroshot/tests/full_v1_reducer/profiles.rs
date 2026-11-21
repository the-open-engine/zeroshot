use super::*;

fn profile(body: Value, initial_input: Value) -> Value {
    json!({
        "kind":"profile","name":"call","profile":"called","body":body,
        "initialInput":initial_input
    })
}

fn parallel_profile(terminal: Value) -> Value {
    let input = json!({"kind":"record","fields":{}});
    profile(
        sequence(
            "called_root",
            vec![
                json!({
                    "kind":"par","name":"child_parallel","state":input,
                    "branches":[
                        sequence("gate_path", vec![verifier("gate",1), terminal]),
                        step("sibling",1)
                    ],
                    "promotedStatePaths":[],"join":{"kind":"all"}
                }),
                succeed("child_fallback"),
            ],
        ),
        input,
    )
}

fn start_profile(graph: &VerifiedGraph) -> Vec<DurableExecution> {
    reduce(graph, &json!({}), &[])
        .decisions
        .into_iter()
        .filter_map(|decision| match decision {
            Decision::Dispatch {
                execution,
                node_instance,
                occurrence,
                ..
            } => Some(active(
                execution.get(),
                node_instance.get(),
                occurrence.node.as_str(),
                execution.get(),
            )),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn profile_success_continues_at_early_terminal_and_preserves_loser_replay() {
    let graph = verified(
        sequence(
            "root",
            vec![
                parallel_profile(succeed("child_success")),
                step("after_call", 1),
                succeed("run_success"),
            ],
        ),
        json!({}),
    )
    .await;
    let mut history = start_profile(&graph);
    assert_eq!(history.len(), 2);
    history
        .iter_mut()
        .find(|execution| execution.occurrence.node.as_str() == "gate")
        .assert_value()
        .state = DurableExecutionState::Settled {
        position: HistoryPosition::new(3).assert_value(),
        outcome: verdict("accepted"),
    };
    let early = reduce(&graph, &json!({}), &history);
    assert_eq!(early.terminal, None);
    let sibling = history
        .iter_mut()
        .find(|execution| execution.occurrence.node.as_str() == "sibling")
        .assert_value();
    assert!(early.decisions.iter().any(|decision| matches!(
        decision,
        Decision::VoidLoser { execution, reason: ExecutionVoidReason::ParallelJoin } if *execution == sibling.execution
    )));
    sibling.state = DurableExecutionState::Voided {
        position: HistoryPosition::new(4).assert_value(),
        reason: ExecutionVoidReason::ParallelJoin,
    };
    let replay = reduce(&graph, &json!({}), &history);
    assert_eq!(replay.terminal, None);
    assert!(
        !replay
            .decisions
            .iter()
            .any(|decision| matches!(decision, Decision::VoidLoser { .. }))
    );
    assert!(replay.decisions.iter().any(|decision| matches!(
        decision,
        Decision::Dispatch { occurrence, .. } if occurrence.node.as_str() == "after_call"
    )));
}

#[tokio::test]
async fn profile_failure_keeps_the_authored_reason() {
    let graph = verified(
        sequence(
            "root",
            vec![profile(
                json!({"kind":"fail","name":"child_failure","reason":"blocked"}),
                json!({"kind":"null"}),
            )],
        ),
        json!({}),
    )
    .await;
    let result = reduce(&graph, &json!({}), &[]);
    assert!(
        matches!(result.terminal, Some(TerminalProjection::Failed { reason, .. }) if reason.as_str() == "blocked")
    );
}

#[tokio::test]
async fn profile_can_project_null_input_from_record_state() {
    let graph = verified(
        sequence(
            "root",
            vec![
                profile(succeed("child_success"), json!({"kind":"null"})),
                succeed("run_success"),
            ],
        ),
        json!({}),
    )
    .await;
    assert_eq!(
        reduce(&graph, &json!({}), &[]).terminal,
        Some(TerminalProjection::Succeeded {
            output: Value::Null
        })
    );
}

fn replay_profile(graph: &VerifiedGraph, input: &Value, history: &[DurableExecution]) -> Reduction {
    FullV1Reducer::native_v2(graph)
        .with_dispatch_limit(1)
        .reduce(ReductionInput {
            initial_input: input,
            executions: history,
            next_node_instance: history
                .iter()
                .map(|execution| execution.node_instance.get())
                .max()
                .unwrap_or(0)
                + 1,
            next_execution: history.len() as u64 + 1,
        })
        .assert_value()
}

#[tokio::test]
async fn nested_profiles_replay_distinct_map_visits_in_each_loop_round() {
    let empty = json!({"kind":"record","fields":{}});
    let indexes = required_array_record(&[("items", "null")]);
    let mut inner = profile(
        sequence(
            "inner_root",
            vec![step("child_work", 1), succeed("inner_done")],
        ),
        empty.clone(),
    );
    *inner.get_mut("name").assert_value() = json!("inner_call");
    let outer = profile(
        sequence("outer_root", vec![inner, succeed("outer_done")]),
        empty,
    );
    let mapped = json!({
        "kind":"map","name":"mapped","state":indexes,"body":outer,
        "over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]
    });
    let repeated = json!({
        "kind":"loop","name":"repeated","state":indexes,"body":mapped,
        "maxIterations":2,"promotedStatePaths":[]
    });
    let root = json!({
        "kind":"seq","name":"root","state":indexes,
        "children":[repeated, succeed("run_done")],"promotedStatePaths":[]
    });
    let graph = verified(root, json!({})).await;
    let input = json!({"items":[null,null]});
    let mut history = Vec::new();
    for (visit, (item, round)) in [(0, 1), (1, 1), (0, 2), (1, 2)].into_iter().enumerate() {
        let reduction = replay_profile(&graph, &input, &history);
        let dispatches = reduction
            .decisions
            .iter()
            .filter(|decision| matches!(decision, Decision::Dispatch { .. }))
            .collect::<Vec<_>>();
        let [
            Decision::Dispatch {
                occurrence,
                execution,
                node_instance,
                attempt,
                iteration,
                ..
            },
        ] = dispatches.as_slice()
        else {
            panic!("expected one child dispatch, got {dispatches:?}");
        };
        assert_eq!(occurrence.node.as_str(), "child_work");
        assert_eq!(occurrence.map_indices, [item]);
        assert_eq!(attempt.get(), 1);
        assert_eq!(*iteration, round);
        let position = visit as u64 * 2 + 2;
        history.push(settled(
            SettledSpec::new(execution.get(), node_instance.get(), "child_work")
                .map_indices(vec![item])
                .dispatched(position - 1)
                .position(position),
            success(1),
        ));
    }
    assert_eq!(
        replay_profile(&graph, &input, &history).terminal,
        Some(TerminalProjection::Succeeded {
            output: Value::Null
        })
    );
}

#[tokio::test]
async fn profile_discards_the_child_state_and_terminal_output() {
    let payload = json!({
        "kind":"record","fields":{"value":{"type":{"kind":"integer"},"required":true}}
    });
    let output = |name| {
        json!({
            "kind":"succeed","name":name,"output":payload,
            "bindings":[{"target":["value"],"value":{"source":"state","path":["value"]}}]
        })
    };
    let child = json!({
        "kind":"seq","name":"child_root","state":payload,
        "children":[promoted_integer_step("child_work", "value"), output("child_done")],
        "promotedStatePaths":[["value"]]
    });
    let root = json!({
        "kind":"seq","name":"root","state":payload,
        "children":[profile(child, payload.clone()), output("run_done")],"promotedStatePaths":[]
    });
    let graph = verified(root, json!({})).await;
    let history = [settled(
        SettledSpec::new(1, 1, "child_work")
            .dispatched(1)
            .position(2),
        success(99),
    )];

    assert_eq!(
        replay_profile(&graph, &json!({"value":1}), &history).terminal,
        Some(TerminalProjection::Succeeded {
            output: json!({"value":1})
        })
    );
}
