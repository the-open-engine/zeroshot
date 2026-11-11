use super::*;

fn state() -> Value {
    required_array_record(&[("items", "null")])
}
fn mapped(body: Value, max_items: u64) -> Value {
    json!({"kind":"map","name":"items_map","state":state(),"body":body,
        "over":{"source":"state","path":["items"]},"maxItems":max_items,"promotedStatePaths":[]})
}
fn input_for<'a>(input: &'a Value, history: &'a [DurableExecution]) -> ReductionInput<'a> {
    ReductionInput {
        initial_input: input,
        executions: history,
        next_node_instance: history
            .iter()
            .map(|execution| execution.node_instance.get())
            .max()
            .unwrap_or(0)
            + 1,
        next_execution: history
            .iter()
            .map(|execution| execution.execution.get())
            .max()
            .unwrap_or(0)
            + 1,
    }
}
fn root(children: Vec<Value>) -> Value {
    json!({"kind":"seq","name":"run","state":state(),"children":children,"promotedStatePaths":[]})
}

#[tokio::test]
async fn complete_input_runs_in_item_batches_including_1025_items() {
    for count in [0, 1, 33, 1024, 1025] {
        let graph = reducer_test_support::verified_graph(
            root(vec![mapped(step("work", 1), 7), succeed("done")]),
            state(),
            true,
        )
        .await;
        let input = json!({"items":vec![Value::Null;count]});
        let mut history = Vec::new();
        let mut observed = Vec::new();
        let mut clock = 1;
        loop {
            let reduced = FullV1Reducer::native_v2(&graph)
                .with_dispatch_limit(11)
                .reduce(input_for(&input, &history))
                .unwrap();
            if let Some(terminal) = reduced.terminal {
                assert!(matches!(terminal, TerminalProjection::Succeeded { .. }));
                break;
            }
            for decision in reduced.decisions {
                if let Decision::Dispatch {
                    node_instance,
                    execution,
                    occurrence,
                    attempt,
                    input,
                    ..
                } = decision
                {
                    observed.push(occurrence.map_indices[0]);
                    history.push(DurableExecution {
                        node_instance,
                        execution,
                        occurrence,
                        attempt,
                        input,
                        dispatch_position: HistoryPosition::new(clock).unwrap(),
                        state: DurableExecutionState::Active,
                    });
                    clock += 1;
                }
            }
            let active = history
                .iter()
                .enumerate()
                .filter_map(|(index, execution)| {
                    matches!(execution.state, DurableExecutionState::Active).then_some(index)
                })
                .collect::<Vec<_>>();
            assert!(!active.is_empty(), "pending input must make progress");
            assert!(active.len() <= 7, "whole item bodies own the map capacity");
            if observed.len() == 7 {
                assert_eq!(active.len(), 7);
            }
            // Keep a body active while admitting the next batch, rather than draining every slot.
            let settle = active.len().saturating_sub(1).max(1);
            for index in active.into_iter().take(settle) {
                history[index].state = DurableExecutionState::Settled {
                    position: HistoryPosition::new(clock).unwrap(),
                    outcome: success(clock as i64),
                };
                clock += 1;
            }
        }
        assert_eq!(observed, (0..count as u64).collect::<Vec<_>>());
    }
}

#[tokio::test]
async fn pending_item_holds_its_slot_through_nested_work_and_parallel_siblings_get_service() {
    let body = sequence("item_body", vec![step("head", 1), step("tail", 1)]);
    let parallel = json!({"kind":"par","name":"parallel","state":state(),"join":{"kind":"all"},
        "branches":[mapped(body,1),step("sibling",1)],"promotedStatePaths":[]});
    let graph =
        reducer_test_support::verified_graph(root(vec![parallel, succeed("done")]), state(), true)
            .await;
    let input = json!({"items":[null,null]});
    let history = [settled(
        SettledSpec::new(1, 1, "head")
            .map_indices(vec![0])
            .position(2),
        success(1),
    )];
    let reduced = one_slot(&graph, &input, &history);
    let dispatches = reduced
        .decisions
        .iter()
        .filter_map(|decision| match decision {
            Decision::Dispatch { occurrence, .. } => Some(occurrence),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].node.as_str(), "sibling");
    assert!(!reduced.decisions.iter().any(|decision| matches!(decision,Decision::Dispatch { occurrence,.. } if occurrence.map_indices==[1])));
}

#[tokio::test]
async fn rejecting_map_waits_for_branch_tail_before_first_join_settles() {
    let aggregate = json!({"kind":"k_of_map","count":1,"value":{"name":"a","source":"signal","field":"verdict"},"labels":["accepted"]});
    let guard = json!({"kind":"any","guards":[aggregate,{"kind":"in","value":{"name":"b","source":"signal","field":"verdict"},"labels":["accepted"]}]});
    let mut branch = sequence(
        "a_branch",
        vec![mapped(verifier("a", 1), 1), step("irrelevant_tail", 1)],
    );
    branch["state"] = state();
    let graph = verified_race(vec![branch, verifier("b", 1)], guard).await;
    let mut history = vec![
        settled(
            SettledSpec::new(1, 1, "a").map_indices(vec![0]).position(3),
            verdict("rejected"),
        ),
        settled(SettledSpec::new(2, 2, "b").position(4), verdict("rejected")),
    ];
    let input = json!({"items":[null]});
    let reduced = one_slot(&graph, &input, &history);
    assert_pending_dispatch(&reduced, "irrelevant_tail");
    history.push(settled(
        SettledSpec::new(3, 3, "irrelevant_tail").position(5),
        success(1),
    ));
    assert_complete(&one_slot(&graph, &input, &history));
}

#[tokio::test]
async fn first_any_guard_accepts_own_evidence_and_preserves_queued_winners() {
    let guard = accepted_race_guard();
    let graph = verified_race(vec![verifier("a", 1), verifier("b", 1)], guard).await;
    for label in ["accepted", "rejected"] {
        let input = json!({"items":[]});
        let history = [settled(
            SettledSpec::new(1, 1, "a").position(2),
            verdict(label),
        )];
        let reduced = one_slot(&graph, &input, &history);
        assert_eq!(reduced.terminal.is_some(), label == "accepted");
        assert_eq!(reduced.decisions.iter().any(|decision|matches!(decision,Decision::Dispatch {occurrence,..} if occurrence.node.as_str()=="b")),label=="rejected");
    }
}

#[tokio::test]
async fn fresh_loop_round_does_not_inherit_map_frontier_exemptions() {
    let repeat = json!({"kind":"loop","name":"twice","state":state(),"body":mapped(step("work",1),65536),"maxIterations":2,"promotedStatePaths":[]});
    let graph =
        reducer_test_support::verified_graph(root(vec![repeat, succeed("done")]), state(), true)
            .await;
    let input = json!({"items":vec![Value::Null;1025]});
    let history = (0..1025)
        .map(|index| {
            settled(
                SettledSpec::new(index + 1, index + 1, "work")
                    .map_indices(vec![index])
                    .position(2 * (index + 1)),
                success(1),
            )
        })
        .collect::<Vec<_>>();
    let reduced = FullV1Reducer::native_v2(&graph)
        .with_dispatch_limit(1)
        .reduce_with_trace(input_for(&input, &history))
        .assert_value();
    let dispatches = reduced
        .reduction
        .decisions
        .iter()
        .filter_map(|decision| match decision {
            Decision::Dispatch {
                occurrence,
                iteration,
                attempt,
                ..
            } => Some((occurrence, iteration, attempt)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].0.map_indices, [0]);
    assert_eq!(*dispatches[0].1, 2);
    assert_eq!(dispatches[0].2.get(), 1);
}

#[tokio::test]
async fn large_map_admits_actual_work_without_precharging_queued_items() {
    let graph = reducer_test_support::verified_graph(
        root(vec![mapped(step("work", 1), 1), succeed("done")]),
        state(),
        true,
    )
    .await;
    let input = json!({"items":vec![Value::Null;65537]});
    let reduced = one_slot(&graph, &input, &[]);
    assert!(reduced.terminal.is_none());
    assert!(
        reduced
            .decisions
            .iter()
            .any(|d| matches!(d,Decision::Dispatch{occurrence,..} if occurrence.map_indices==[0]))
    );
}

#[tokio::test]
async fn completed_empty_nested_maps_do_not_consume_pending_frontier() {
    let empty_state = json!({"kind":"record","fields":{
        "items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true},
        "empty":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}});
    let inner = json!({"kind":"map","name":"empty_inner","state":empty_state,"maxItems":1,
        "over":{"source":"state","path":["empty"]},"body":step("work",1),"promotedStatePaths":[]});
    let outer = json!({"kind":"map","name":"outer","state":empty_state,"maxItems":1,
        "over":{"source":"state","path":["items"]},"body":inner,"promotedStatePaths":[]});
    let root = json!({"kind":"seq","name":"run","state":empty_state,"children":[outer,succeed("done")],"promotedStatePaths":[]});
    let graph = reducer_test_support::verified_graph(root, empty_state, true).await;
    let input = json!({"items":[null,null],"empty":[]});
    let reduced = one_slot(&graph, &input, &[]);
    assert_complete(&reduced);
}

fn empty_loop_state() -> Value {
    json!({"kind":"record","fields":{"empty":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}})
}
fn empty_loop(name: &str, iterations: u64) -> Value {
    let state = empty_loop_state();
    json!({"kind":"loop","name":name,"state":state,"maxIterations":iterations,"promotedStatePaths":[],
        "body":{"kind":"map","name":format!("{name}_empty"),"state":state,"maxItems":1,"over":{"source":"state","path":["empty"]},
        "body":step(&format!("{name}_unused"),1),"promotedStatePaths":[]}})
}

#[tokio::test]
async fn replay_keeps_accepted_loop_prefixes_of_completed_race_losers() {
    let state = empty_loop_state();
    let mut left = sequence(
        "left",
        vec![empty_loop("accepted_prefix", 65535), verifier("a", 1)],
    );
    left["state"] = state.clone();
    let guard = accepted_race_guard();
    let race = json!({"kind":"par","name":"race","state":state,"branches":[left,verifier("b",1)],"join":{"kind":"first","when":guard},"promotedStatePaths":[]});
    let mut root = json!({"kind":"seq","name":"run","state":state,"children":[race,empty_loop("tail",2),succeed("done")],"promotedStatePaths":[]});
    for tail in [1, 2] {
        root["children"][1]["maxIterations"] = json!(tail);
        let graph = reducer_test_support::verified_graph(root.clone(), state.clone(), true).await;
        let history = race_history("rejected", "accepted");
        let input = json!({"empty":[]});
        let result = FullV1Reducer::native_v2(&graph).reduce(input_for(&input, &history));
        if tail == 1 {
            assert_complete(&result.assert_value());
        } else {
            assert!(
                matches!(
                    result,
                    Err(ReducerError::WorkLimit {
                        bound: 65536,
                        kind: "loop-body entries",
                        ..
                    })
                ),
                "{result:?}"
            );
        }
    }
}

#[tokio::test]
async fn loop_history_probing_is_bounded_without_charging_unstarted_loser() {
    let state = empty_loop_state();
    let nested = json!({"kind":"loop","name":"huge","state":state,"maxIterations":65536,"body":empty_loop("inner",65536),"promotedStatePaths":[]});
    let mut left = sequence("left", vec![nested.clone(), verifier("a", 1)]);
    left["state"] = state.clone();
    let root = root_with_join(vec![left, verifier("b", 1)], json!({"kind":"any"}), &state);
    let graph = reducer_test_support::verified_graph(root, state.clone(), true).await;
    let input = json!({"empty":[]});
    let history = [settled(
        SettledSpec::new(1, 1, "b").position(2),
        verdict("accepted"),
    )];
    let reduced = FullV1Reducer::native_v2(&graph)
        .reduce(input_for(&input, &history))
        .assert_value();
    assert!(reduced.terminal.is_some());
    let root = json!({"kind":"seq","name":"run","state":state,"children":[nested,succeed("done")],"promotedStatePaths":[]});
    let graph = reducer_test_support::verified_graph(root, state, true).await;
    assert!(matches!(
        FullV1Reducer::native_v2(&graph).reduce(input_for(&input, &[])),
        Err(ReducerError::WorkLimit { bound: 65536, .. })
    ));
}

pub(super) fn accepted_race_guard() -> Value {
    json!({"kind":"any","guards":[
        {"kind":"in","value":{"name":"a","source":"signal","field":"verdict"},"labels":["accepted"]},
        {"kind":"in","value":{"name":"b","source":"signal","field":"verdict"},"labels":["accepted"]}]})
}

#[tokio::test]
async fn pending_rejecting_racer_enters_its_tail_until_actual_work_limit() {
    for through_choice in [false, true] {
        let state = empty_loop_state();
        let huge = json!({"kind":"loop","name":"huge","state":state,"maxIterations":65536,"body":empty_loop("inner",65536),"promotedStatePaths":[]});
        let mut left = json!({"kind":"seq","name":"left","state":state,"children":[verifier("a",1),huge],"promotedStatePaths":[]});
        if through_choice {
            left = json!({"kind":"choice","name":"route","state":state,
                "branches":[{"when":{"kind":"in","value":{"name":"gate","source":"signal","field":"verdict"},"labels":["accepted"]},"node":left}],
                "otherwise":verifier("unused",1),"promotedStatePaths":[]});
        }
        let race = json!({"kind":"par","name":"race","state":state,"branches":[left,verifier("b",1)],"join":{"kind":"first","when":accepted_race_guard()},"promotedStatePaths":[]});
        let root = json!({"kind":"seq","name":"run","state":state,"children":[verifier("gate",1),race,succeed("done")],"promotedStatePaths":[]});
        let graph = reducer_test_support::verified_graph(root, state, true).await;
        let history = [
            settled(
                SettledSpec::new(1, 1, "gate").position(2),
                verdict("accepted"),
            ),
            settled(
                SettledSpec::new(2, 2, "a").position(5).dispatched(3),
                verdict("rejected"),
            ),
            settled(
                SettledSpec::new(3, 3, "b").position(6).dispatched(4),
                verdict("rejected"),
            ),
        ];
        let input = json!({"empty":[]});
        assert!(matches!(
            FullV1Reducer::native_v2(&graph)
                .with_dispatch_limit(1)
                .reduce(input_for(&input, &history)),
            Err(ReducerError::WorkLimit {
                bound: 65536,
                kind: "loop-body entries",
                ..
            })
        ));
    }
}

#[tokio::test]
async fn reached_loop_prefix_does_not_charge_unstarted_rounds_in_a_pruned_racer() {
    let state = required_array_record(&[("items", "null"), ("empty", "null")]);
    let mapped = json!({"kind":"map","name":"outer","state":state,"body":empty_loop("big",65534),
        "over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]});
    let body = json!({"kind":"seq","name":"body","state":state,"children":[mapped,verifier("a",1)],"promotedStatePaths":[]});
    let twice = json!({"kind":"loop","name":"twice","state":state,"body":body,"maxIterations":2,"promotedStatePaths":[]});
    let wrapped = json!({"kind":"seq","name":"wrapped","state":state,"children":[twice],"promotedStatePaths":[]});
    let root = root_with_join(
        vec![wrapped, verifier("b", 1)],
        json!({"kind":"first","when":accepted_race_guard()}),
        &state,
    );
    let graph = reducer_test_support::verified_graph(root, state, true).await;
    let history = race_history("rejected", "accepted");
    let reduced = one_slot(&graph, &json!({"items":[null],"empty":[]}), &history);
    assert_complete(&reduced);
}

fn one_slot(graph: &VerifiedGraph, input: &Value, history: &[DurableExecution]) -> Reduction {
    FullV1Reducer::native_v2(graph)
        .with_dispatch_limit(1)
        .reduce(input_for(input, history))
        .assert_value()
}

async fn verified_race(branches: Vec<Value>, guard: Value) -> VerifiedGraph {
    let race = json!({"kind":"par","name":"race","state":state(),"branches":branches,"join":{"kind":"first","when":guard},"promotedStatePaths":[]});
    reducer_test_support::verified_graph(root(vec![race, succeed("done")]), state(), true).await
}

fn assert_complete(reduced: &Reduction) {
    assert!(reduced.terminal.is_some());
    assert!(
        !reduced
            .decisions
            .iter()
            .any(|decision| matches!(decision, Decision::Dispatch { .. }))
    );
}

fn race_history(a: &str, b: &str) -> [DurableExecution; 2] {
    [
        settled(SettledSpec::new(1, 1, "a").position(2), verdict(a)),
        settled(SettledSpec::new(2, 2, "b").position(3), verdict(b)),
    ]
}
fn root_with_join(branches: Vec<Value>, join: Value, state: &Value) -> Value {
    let race = json!({"kind":"par","name":"race","state":state,"branches":branches,"join":join,"promotedStatePaths":[]});
    json!({"kind":"seq","name":"run","state":state,"children":[race,succeed("done")],"promotedStatePaths":[]})
}

#[path = "outcomes.rs"]
mod outcomes;

fn assert_pending_dispatch(reduced: &Reduction, node: &str) {
    assert!(reduced.terminal.is_none(), "{reduced:#?}");
    assert!(reduced.decisions.iter().any(|decision| matches!(decision,Decision::Dispatch{occurrence,..} if occurrence.node.as_str()==node)),"{reduced:#?}");
}

#[tokio::test]
async fn partially_settled_batches_fill_all_available_execution_capacity() {
    let graph = reducer_test_support::verified_graph(
        root(vec![mapped(step("work", 1), 128), succeed("done")]),
        state(),
        true,
    )
    .await;
    let input = json!({"items":vec![Value::Null;128]});
    let mut history = Vec::new();
    let mut clock = 1;
    for available in [64, 57, 7] {
        let reduced = FullV1Reducer::native_v2(&graph)
            .with_dispatch_limit(available)
            .reduce(input_for(&input, &history))
            .unwrap();
        let mut started = 0;
        for decision in reduced.decisions {
            if let Decision::Dispatch {
                node_instance,
                execution,
                occurrence,
                attempt,
                input,
                ..
            } = decision
            {
                history.push(DurableExecution {
                    node_instance,
                    execution,
                    occurrence,
                    attempt,
                    input,
                    dispatch_position: HistoryPosition::new(clock).unwrap(),
                    state: DurableExecutionState::Active,
                });
                clock += 1;
                started += 1;
            }
        }
        assert_eq!(
            started, available,
            "active bodies do not consume the free frontier budget"
        );
        let settle = match available {
            64 => 57,
            57 => 7,
            _ => 64,
        };
        for execution in history
            .iter_mut()
            .filter(|execution| matches!(execution.state, DurableExecutionState::Active))
            .take(settle)
        {
            execution.state = DurableExecutionState::Settled {
                position: HistoryPosition::new(clock).unwrap(),
                outcome: success(1),
            };
            clock += 1;
        }
    }
    let done = FullV1Reducer::native_v2(&graph)
        .reduce(input_for(&input, &history))
        .unwrap();
    assert!(matches!(
        done.terminal,
        Some(TerminalProjection::Succeeded { .. })
    ));
    assert_eq!(history.len(), 128);
}
