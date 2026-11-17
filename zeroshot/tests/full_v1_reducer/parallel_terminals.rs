use super::*;

fn parallel(join: Value, branches: Vec<Value>) -> Value {
    json!({"kind":"par","name":"parallel","state":{"kind":"record","fields":{}},
        "promotedStatePaths":[],"join":join,"branches":branches})
}

fn fail(name: &str, reason: &str) -> Value {
    json!({"kind":"fail","name":name,"reason":reason})
}

fn joins() -> Vec<Value> {
    vec![
        json!({"kind":"all"}),
        json!({"kind":"any"}),
        json!({"kind":"quorum","count":2}),
        json!({"kind":"first","when":{"kind":"in",
            "value":{"name":"docs","source":"signal","field":"verdict"},
            "labels":["accepted"]}}),
    ]
}

#[tokio::test]
async fn branch_terminal_precedes_every_join_and_cancels_active_sibling() {
    for join in joins() {
        for succeeds in [false, true] {
            let terminal = if succeeds {
                succeed("stop")
            } else {
                fail("stop", "review_rejected")
            };
            let graph = verified(
                sequence(
                    "run",
                    vec![
                        parallel(
                            join.clone(),
                            vec![
                                sequence("gate_path", vec![verifier("gate", 1), terminal]),
                                sequence("docs_path", vec![verifier("docs", 1), step("queued", 1)]),
                            ],
                        ),
                        fail("after", "ran_past_terminal"),
                    ],
                ),
                json!({}),
            )
            .await;
            let mut history = [
                settled(
                    SettledSpec::new(1, 1, "gate").dispatched(1).position(3),
                    verdict("rejected"),
                ),
                active(2, 2, "docs", 2),
            ];
            let reduced = replay(&graph, &json!({}), &history);
            let expected = if succeeds {
                TerminalProjection::Succeeded {
                    output: Value::Null,
                }
            } else {
                TerminalProjection::Failed {
                    reason: "review_rejected".parse().unwrap(),
                    context: None,
                }
            };
            assert_eq!(reduced.terminal, Some(expected.clone()), "join={join}");
            assert!(reduced.decisions.iter().any(|decision| matches!(decision,
                Decision::VoidLoser { execution, .. } if execution.get() == 2)));
            assert!(
                !reduced
                    .decisions
                    .iter()
                    .any(|decision| matches!(decision, Decision::Dispatch { .. }))
            );
            history[1].state = DurableExecutionState::Voided {
                position: HistoryPosition::new(4).unwrap(),
                reason: ExecutionVoidReason::ParallelJoin,
            };
            let replayed = replay(&graph, &json!({}), &history);
            assert_eq!(replayed.terminal, Some(expected));
            assert!(!replayed.decisions.iter().any(|decision| matches!(
                decision,
                Decision::Dispatch { .. } | Decision::VoidLoser { .. }
            )));
        }
    }
}

#[tokio::test]
async fn rejected_review_gate_ends_par_all_with_its_reason() {
    let route = json!({"kind":"choice","name":"review_result","state":{"kind":"record","fields":{}},
        "promotedStatePaths":[],"branches":[{"when":{"kind":"in",
            "value":{"source":"signal","name":"review","field":"verdict"},"labels":["rejected"]},
            "node":fail("blocked", "review_rejected")}],"otherwise":step("note", 1)});
    let graph = verified(
        sequence(
            "run",
            vec![
                parallel(
                    json!({"kind":"all"}),
                    vec![
                        sequence("gate", vec![verifier("review", 1), route]),
                        step("docs", 1),
                    ],
                ),
                succeed("done"),
            ],
        ),
        json!({}),
    )
    .await;
    let history = [
        settled(
            SettledSpec::new(1, 1, "docs").dispatched(1).position(3),
            success(1),
        ),
        settled(
            SettledSpec::new(2, 2, "review").dispatched(2).position(4),
            verdict("rejected"),
        ),
    ];
    assert!(matches!(replay(&graph, &json!({}), &history).terminal,
        Some(TerminalProjection::Failed { reason, .. }) if reason.as_str() == "review_rejected"));
}

fn replay(graph: &VerifiedGraph, input: &Value, history: &[DurableExecution]) -> Reduction {
    let input = ReductionInput {
        initial_input: input,
        executions: history,
        next_execution: history.len() as u64 + 1,
        next_node_instance: history.len() as u64 + 1,
    };
    let reducer = FullV1Reducer::native_v2(graph).with_dispatch_limit(1);
    let plain = reducer.reduce(input.clone()).unwrap();
    assert_eq!(plain, reducer.reduce(input.clone()).unwrap());
    assert_eq!(plain, reducer.reduce_with_trace(input).unwrap().reduction);
    plain
}

#[tokio::test]
async fn earlier_join_discards_a_later_branch_terminal() {
    for terminal in [fail("stop", "review_rejected"), succeed("stop")] {
        for join in [
            json!({"kind":"any"}),
            json!({"kind":"quorum","count":1}),
            joins().pop().unwrap(),
        ] {
            let graph = verified(
                sequence(
                    "run",
                    vec![
                        parallel(
                            join,
                            vec![
                                sequence("gate_path", vec![verifier("gate", 1), terminal.clone()]),
                                verifier("docs", 1),
                            ],
                        ),
                        step("tail", 1),
                        succeed("done"),
                    ],
                ),
                json!({}),
            )
            .await;
            let history = [
                settled(
                    SettledSpec::new(1, 1, "gate").dispatched(1).position(4),
                    verdict("rejected"),
                ),
                settled(
                    SettledSpec::new(2, 2, "docs").dispatched(2).position(3),
                    verdict("accepted"),
                ),
            ];
            let reduced = replay(&graph, &json!({}), &history);
            assert!(reduced.terminal.is_none());
            assert!(reduced.decisions.iter().any(|decision| matches!(decision,
                Decision::Dispatch { occurrence, .. } if occurrence.node.as_str() == "tail")));
        }
    }
}

#[tokio::test]
async fn earliest_terminal_uses_history_order_and_preserves_output() {
    let state =
        json!({"kind":"record","fields":{"result":{"type":{"kind":"integer"},"required":true}}});
    let output = json!({"kind":"succeed","name":"selected","output":state,
        "bindings":[{"target":["result"],"value":{"source":"state","path":["result"]}}]});
    let mut writer = sequence(
        "z_path",
        vec![promoted_integer_step("writer", "result"), output],
    );
    writer["state"] = state.clone();
    let mut par = parallel(
        json!({"kind":"all"}),
        vec![
            sequence(
                "a_path",
                vec![step("later", 1), fail("later_stop", "later")],
            ),
            writer,
        ],
    );
    par["state"] = state.clone();
    let mut root = sequence("run", vec![par, fail("after", "ran_past_terminal")]);
    root["state"] = state;
    let graph = verified(root, json!({})).await;
    for selected_first in [true, false] {
        let history = [
            settled(
                SettledSpec::new(1, 1, "later")
                    .dispatched(1)
                    .position(if selected_first { 6 } else { 4 }),
                success(1),
            ),
            settled(
                SettledSpec::new(2, 2, "writer")
                    .dispatched(2)
                    .position(if selected_first { 4 } else { 6 }),
                success(42),
            ),
        ];
        let reduced = replay(&graph, &json!({"result":0}), &history);
        let expected = if selected_first {
            TerminalProjection::Succeeded {
                output: json!({"result":42}),
            }
        } else {
            TerminalProjection::Failed {
                reason: "later".parse().unwrap(),
                context: None,
            }
        };
        assert_eq!(reduced.terminal, Some(expected));
    }
}

#[tokio::test]
async fn immediate_terminal_leaves_queued_siblings_unstarted() {
    for join in joins() {
        let graph = verified(
            sequence(
                "run",
                vec![
                    parallel(
                        join,
                        vec![fail("stop", "review_rejected"), verifier("docs", 1)],
                    ),
                    succeed("after"),
                ],
            ),
            json!({}),
        )
        .await;
        let reduced = replay(&graph, &json!({}), &[]);
        assert!(
            matches!(reduced.terminal, Some(TerminalProjection::Failed { reason, .. })
            if reason.as_str() == "review_rejected")
        );
        assert!(
            !reduced
                .decisions
                .iter()
                .any(|decision| matches!(decision, Decision::Dispatch { .. }))
        );
    }
}

#[tokio::test]
async fn failure_does_not_charge_unstarted_sibling_loops() {
    terminal_does_not_charge_unstarted_sibling_loops(false).await;
}

#[tokio::test]
async fn success_does_not_charge_unstarted_sibling_loops() {
    terminal_does_not_charge_unstarted_sibling_loops(true).await;
}

async fn terminal_does_not_charge_unstarted_sibling_loops(succeeds: bool) {
    let state = required_array_record(&[("empty", "null")]);
    for iterations in [32_768, 32_769] {
        for started in [false, true] {
            let unstarted = |name: &str| {
                json!({"kind":"loop","name":name,"state":state,"maxIterations":iterations,
                    "promotedStatePaths":[],"body":{"kind":"map","name":format!("{name}_empty"),
                    "state":state,"maxItems":1,"over":{"source":"state","path":["empty"]},
                    "body":step(&format!("{name}_unused"),1),"promotedStatePaths":[]}})
            };
            let terminal = if succeeds {
                succeed("stop")
            } else {
                fail("stop", "review_rejected")
            };
            let gate = if started {
                let mut gate = sequence("gate_path", vec![verifier("gate", 1), terminal]);
                gate["state"] = state.clone();
                gate
            } else {
                terminal
            };
            let mut par = parallel(
                json!({"kind":"all"}),
                vec![
                    gate,
                    unstarted("one"),
                    unstarted("two"),
                    verifier("docs", 1),
                ],
            );
            par["state"] = state.clone();
            let mut root = sequence("run", vec![par, fail("after", "ran_past_terminal")]);
            root["state"] = state.clone();
            let graph = verified(root, json!({})).await;
            let history = if started {
                vec![
                    settled(
                        SettledSpec::new(1, 1, "gate").dispatched(1).position(3),
                        verdict("rejected"),
                    ),
                    active(2, 2, "docs", 2),
                ]
            } else {
                vec![]
            };
            let input = json!({"empty":[]});
            let reduction_input = ReductionInput {
                initial_input: &input,
                executions: &history,
                next_execution: history.len() as u64 + 1,
                next_node_instance: history.len() as u64 + 1,
            };
            let reducer = FullV1Reducer::native_v2(&graph);
            let reduced = reducer.reduce(reduction_input.clone()).unwrap();
            assert_eq!(reduced, reducer.reduce(reduction_input).unwrap());
            let expected = if succeeds {
                TerminalProjection::Succeeded {
                    output: Value::Null,
                }
            } else {
                TerminalProjection::Failed {
                    reason: "review_rejected".parse().unwrap(),
                    context: None,
                }
            };
            assert_eq!(reduced.terminal, Some(expected));
            assert!(
                !reduced
                    .decisions
                    .iter()
                    .any(|d| matches!(d, Decision::Dispatch { .. }))
            );
            assert_eq!(
                reduced.decisions.iter().any(|d| matches!(d,
                Decision::VoidLoser { execution, .. } if execution.get() == 2)),
                started
            );
            if started {
                let mut invalid = history.clone();
                invalid[1].input = json!({"unexpected":true});
                assert!(matches!(
                    FullV1Reducer::native_v2(&graph).reduce(ReductionInput {
                        initial_input: &input,
                        executions: &invalid,
                        next_execution: 3,
                        next_node_instance: 3,
                    }),
                    Err(ReducerError::InconsistentHistory)
                ));
            }
        }
    }
}

#[tokio::test]
async fn nested_parallel_terminals_propagate_through_maps_loops_and_parallel_groups() {
    for nesting in ["map", "loop", "par"] {
        let state = required_array_record(&[("items", "null")]);
        let mut gate = sequence(
            "gate_path",
            vec![verifier("gate", 1), fail("stop", "review_rejected")],
        );
        gate["state"] = state.clone();
        let mut body = parallel(json!({"kind":"all"}), vec![gate, verifier("docs", 1)]);
        body["state"] = state.clone();
        let outer = match nesting {
            "map" => json!({"kind":"map","name":"outer","state":state,"promotedStatePaths":[],
                "over":{"source":"state","path":["items"]},"maxItems":2,"body":body}),
            "loop" => json!({"kind":"loop","name":"outer","state":state,"promotedStatePaths":[],
                "maxIterations":2,"body":body}),
            _ => json!({"kind":"par","name":"outer","state":state,"promotedStatePaths":[],
                "join":{"kind":"all"},"branches":[body,step("outer_sibling", 1)]}),
        };
        let root = json!({"kind":"seq","name":"run","state":state,"promotedStatePaths":[],
            "children":[outer,succeed("after")]});
        let graph = verified(root, json!({})).await;
        let scope = if nesting == "map" { vec![0] } else { vec![] };
        let mut sibling = active(2, 2, "docs", 2);
        sibling.occurrence.map_indices = scope.clone();
        let history = [
            settled(
                SettledSpec::new(1, 1, "gate")
                    .dispatched(1)
                    .position(3)
                    .map_indices(scope),
                verdict("rejected"),
            ),
            sibling,
        ];
        let reduced = replay(&graph, &json!({"items":[null,null]}), &history);
        assert!(
            matches!(reduced.terminal, Some(TerminalProjection::Failed { reason, .. })
            if reason.as_str() == "review_rejected"),
            "{nesting}"
        );
        assert!(reduced.decisions.iter().any(|decision| matches!(decision,
            Decision::VoidLoser { execution, .. } if execution.get() == 2)));
        assert!(
            !reduced
                .decisions
                .iter()
                .any(|decision| matches!(decision, Decision::Dispatch { .. }))
        );
    }
}
