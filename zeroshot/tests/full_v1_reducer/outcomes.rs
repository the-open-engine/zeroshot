use super::*;

fn signal(name: &str, label: &str, aggregate: bool) -> Value {
    if aggregate {
        json!({"kind":"k_of_map","count":1,"value":{"name":name,"source":"signal","field":"verdict"},"labels":[label]})
    } else {
        json!({"kind":"in","value":{"name":name,"source":"signal","field":"verdict"},"labels":[label]})
    }
}

fn either(a: Value) -> Value {
    json!({"kind":"any","guards":[a,signal("b","accepted",false)]})
}

fn no_satisfier() -> Value {
    json!({"kind":"choice","name":"outcome","state":state(),"branches":[{
        "when":{"kind":"in","value":{"name":"race","source":"group","field":"raced"},"labels":["no_satisfier"]},
        "node":{"kind":"fail","name":"impossible","reason":"no_satisfier"}}],
        "otherwise":succeed("done"),"promotedStatePaths":[]})
}

async fn checked_race(branches: Vec<Value>, guard: Value) -> VerifiedGraph {
    let mut definition = root_with_join(branches, json!({"kind":"first","when":guard}), &state());
    definition["children"][1] = no_satisfier();
    reducer_test_support::verified_graph(definition, state(), true).await
}

fn assert_no_satisfier(reduced: &Reduction) {
    assert_complete(reduced);
    assert!(
        matches!(&reduced.terminal,Some(TerminalProjection::Failed { reason,.. }) if reason.as_str()=="no_satisfier")
    );
}

#[tokio::test]
async fn unfinished_loop_keeps_scalar_and_mapped_verdicts_replaceable() {
    for aggregate in [false, true] {
        for nested in [false, true] {
            for first_label in ["accepted", "rejected"] {
                let producer = if aggregate {
                    mapped(verifier("a", 1), 1)
                } else {
                    verifier("a", 1)
                };
                let mut body = sequence("round_body", vec![producer, step("round_tail", 1)]);
                body["state"] = state();
                let mut repeated = json!({"kind":"loop","name":"twice","state":state(),"body":body,
                "maxIterations":2,"promotedStatePaths":[]});
                if nested {
                    repeated = json!({"kind":"loop","name":"outer_loop","state":state(),"body":repeated,
                    "maxIterations":1,"promotedStatePaths":[]});
                }
                let graph = checked_race(
                    vec![repeated, verifier("b", 1)],
                    either(signal("a", "accepted", aggregate)),
                )
                .await;
                let indices = if aggregate { vec![0] } else { vec![] };
                let mut history = vec![
                    settled(
                        SettledSpec::new(1, 1, "a")
                            .map_indices(indices.clone())
                            .position(2),
                        verdict(first_label),
                    ),
                    settled(SettledSpec::new(2, 2, "b").position(3), verdict("rejected")),
                ];
                let input = json!({"items":[null]});
                let first = one_slot(&graph, &input, &history);
                assert!(
                    first.terminal.is_none(),
                    "round one cannot freeze the loop's final verdict"
                );
                assert!(first.decisions.iter().any(|d|matches!(d,Decision::Dispatch {occurrence,..} if occurrence.node.as_str()=="round_tail")));
                history.push(settled(
                    SettledSpec::new(3, 3, "round_tail").position(4),
                    success(1),
                ));
                let next = one_slot(&graph, &input, &history);
                assert!(next.terminal.is_none());
                assert!(next.decisions.iter().any(|d|matches!(d,Decision::Dispatch {occurrence,iteration,..} if occurrence.node.as_str()=="a" && occurrence.map_indices==indices && *iteration==2)));
                history.push(settled(
                    SettledSpec::new(4, 1, "a").map_indices(indices).position(5),
                    verdict(if first_label == "accepted" {
                        "rejected"
                    } else {
                        "accepted"
                    }),
                ));
                history.push(settled(
                    SettledSpec::new(5, 3, "round_tail").position(6),
                    success(2),
                ));
                let complete = one_slot(&graph, &input, &history);
                if first_label == "accepted" {
                    assert_no_satisfier(&complete);
                } else {
                    assert_eq!(
                        complete.terminal,
                        Some(TerminalProjection::Succeeded {
                            output: Value::Null
                        })
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn joins_can_discard_unstarted_oversized_maps_before_work_admission() {
    for join in [
        json!({"kind":"any"}),
        json!({"kind":"quorum","count":1}),
        json!({"kind":"first","when":either(signal("a","accepted",true))}),
    ] {
        let graph = reducer_test_support::verified_graph(
            root_with_join(
                vec![mapped(verifier("a", 1), 1), verifier("b", 1)],
                join,
                &state(),
            ),
            state(),
            true,
        )
        .await;
        let input = json!({"items":vec![Value::Null;65537]});
        let history = [settled(
            SettledSpec::new(1, 1, "b").position(2),
            verdict("accepted"),
        )];
        assert_complete(&one_slot(&graph, &input, &history));
        // With no winner yet, the racer may start, but speculative total size is not actual work.
        let pending = one_slot(&graph, &input, &[]);
        assert!(pending.terminal.is_none());
        assert_eq!(
            pending
                .decisions
                .iter()
                .filter(|d| matches!(d, Decision::Dispatch { .. }))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn pending_maps_finish_all_item_bodies_before_first_eligibility() {
    let both =
        json!({"kind":"all","guards":[signal("a","accepted",true),signal("a","rejected",true)]});
    let graph = checked_race(
        vec![mapped(verifier("a", 1), 1), verifier("b", 1)],
        either(both),
    )
    .await;
    let history = [
        settled(
            SettledSpec::new(1, 1, "a").map_indices(vec![0]).position(2),
            verdict("accepted"),
        ),
        settled(SettledSpec::new(2, 2, "b").position(3), verdict("rejected")),
    ];
    let next = one_slot(&graph, &json!({"items":[null,null]}), &history);
    assert!(
        next.terminal.is_none(),
        "a settled accepted item plus a future rejected item can win"
    );
    assert!(next.decisions.iter().any(|d|matches!(d,Decision::Dispatch {occurrence,..} if occurrence.node.as_str()=="a" && occurrence.map_indices==[1])));

    let mut body = sequence("item_body", vec![verifier("a", 1), step("item_tail", 1)]);
    body["state"] = state();
    let graph = checked_race(
        vec![mapped(body, 1), verifier("b", 1)],
        either(signal("a", "accepted", true)),
    )
    .await;
    let next = one_slot(&graph, &json!({"items":[null]}), &history);
    assert!(
        next.terminal.is_none(),
        "aggregate will become available when the pending body completes"
    );
    assert!(next.decisions.iter().any(
        |d| matches!(d,Decision::Dispatch {occurrence,..} if occurrence.node.as_str()=="item_tail")
    ));
}

#[tokio::test]
async fn missing_skipped_and_foreign_facts_stay_unknown_after_pending_tail_finishes() {
    for aggregate in [false, true] {
        for negate in [false, true] {
            let a = signal("a", "accepted", aggregate);
            let condition = if negate {
                json!({"kind":"not","guard":a})
            } else {
                a
            };
            let producer = if aggregate {
                mapped(verifier("a", 1), 1)
            } else {
                verifier("a", 1)
            };
            let choice = json!({"kind":"choice","name":"route","state":state(),
                "branches":[{"when":signal("gate","accepted",false),"node":producer}],
                "otherwise":step("fallback",1),"promotedStatePaths":[]});
            let mut left = sequence("left", vec![choice, step("tail", 1)]);
            left["state"] = state();
            let race = json!({"kind":"par","name":"race","state":state(),"branches":[left,verifier("b",1)],
                "join":{"kind":"first","when":either(condition)},"promotedStatePaths":[]});
            let graph = reducer_test_support::verified_graph(
                root(vec![verifier("gate", 1), race, no_satisfier()]),
                state(),
                true,
            )
            .await;
            let mut history = vec![
                entry(1, "gate", "rejected", vec![]),
                done(2, "fallback", vec![]),
                entry(3, "b", "rejected", vec![]),
            ];
            let input = json!({"items":[null]});
            let pending = one_slot(&graph, &input, &history);
            assert_pending_dispatch(&pending, "tail");
            history.push(done(4, "tail", vec![]));
            assert_no_satisfier(&one_slot(&graph, &input, &history));
        }
    }
}

#[tokio::test]
async fn actual_signal_and_error_outcomes_resolve_after_candidate_completion() {
    for aggregate in [false, true] {
        for error in [false, true] {
            let second = if error {
                if aggregate {
                    json!({"kind":"k_of_map","count":1,"value":{"name":"a","source":"error","field":null},"labels":["crash"]})
                } else {
                    json!({"kind":"in","value":{"name":"a","source":"error","field":null},"labels":["crash"]})
                }
            } else {
                signal("a", "rejected", aggregate)
            };
            let guard =
                either(json!({"kind":"all","guards":[signal("a","accepted",aggregate),second]}));
            let producer = if aggregate {
                mapped(verifier("a", 1), 1)
            } else {
                verifier("a", 1)
            };
            let graph = checked_race(vec![producer, verifier("b", 1)], guard).await;
            let input = json!({"items":[null]});
            let mut history = vec![entry(1, "b", "rejected", vec![])];
            let pending = one_slot(&graph, &input, &history);
            assert_pending_dispatch(&pending, "a");
            history.push(entry(
                2,
                "a",
                "accepted",
                if aggregate { vec![0] } else { vec![] },
            ));
            assert_no_satisfier(&one_slot(&graph, &input, &history));
            if aggregate {
                let mut next = entry(3, "a", "rejected", vec![1]);
                if error {
                    next.state = DurableExecutionState::Settled {
                        position: HistoryPosition::new(7).assert_value(),
                        outcome: WorkerOutcome::Error {
                            code: openengine_cluster_protocol::WorkerErrorCode::Crash,
                            reason:
                                openengine_cluster_protocol::WorkerFailureReason::DeclaredFailure,
                        },
                    };
                }
                history.push(next);
                assert!(matches!(
                    one_slot(&graph, &json!({"items":[null,null]}), &history).terminal,
                    Some(TerminalProjection::Succeeded { .. })
                ));
            }
        }
    }
}

#[tokio::test]
async fn completed_candidate_can_use_pre_parallel_scalar_and_aggregate_facts() {
    for aggregate in [false, true] {
        let prior = if aggregate {
            mapped(verifier("gate", 1), 1)
        } else {
            verifier("gate", 1)
        };
        let guard = json!({"kind":"all","guards":[signal("gate","accepted",aggregate),either(signal("a","accepted",false))]});
        let race = json!({"kind":"par","name":"race","state":state(),"branches":[verifier("a",1),verifier("b",1)],"join":{"kind":"first","when":guard},"promotedStatePaths":[]});
        let graph = reducer_test_support::verified_graph(
            root(vec![prior, race, no_satisfier()]),
            state(),
            true,
        )
        .await;
        let history = vec![
            entry(
                1,
                "gate",
                "accepted",
                if aggregate { vec![0] } else { vec![] },
            ),
            entry(2, "a", "accepted", vec![]),
        ];
        assert!(matches!(
            one_slot(&graph, &json!({"items":[null]}), &history).terminal,
            Some(TerminalProjection::Succeeded { .. })
        ));
    }
}

fn entry(id: u64, name: &str, label: &str, indices: Vec<u64>) -> DurableExecution {
    settled(
        SettledSpec::new(id, id, name)
            .position(id * 2)
            .map_indices(indices),
        verdict(label),
    )
}
fn done(id: u64, name: &str, indices: Vec<u64>) -> DurableExecution {
    settled(
        SettledSpec::new(id, id, name)
            .position(id * 2)
            .map_indices(indices),
        success(1),
    )
}
fn seq(name: &str, children: Vec<Value>, schema: &Value) -> Value {
    json!({"kind":"seq","name":name,"state":schema,"children":children,"promotedStatePaths":[]})
}
fn opposite_verdicts() -> Value {
    json!({"kind":"all","guards":[signal("a","accepted",true),signal("a","rejected",true)]})
}
fn normalize_instances(history: &mut [DurableExecution]) {
    let mut instances = std::collections::BTreeMap::new();
    for execution in history {
        execution.node_instance = *instances
            .entry(execution.occurrence.clone())
            .or_insert(execution.node_instance);
    }
}
async fn race_in_scope(
    left: Value,
    condition: Value,
    schema: Value,
    nested: bool,
) -> VerifiedGraph {
    let mut definition = race_definition(left, condition, &schema);
    if nested {
        let outer = json!({"kind":"map","name":"outer_items","state":schema,"body":definition,"over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]});
        definition = seq("outer_run", vec![outer, succeed("outer_done")], &schema);
    }
    reducer_test_support::verified_graph(definition, schema, true).await
}
#[path = "loop_exits.rs"]
mod loop_exits;
#[path = "signal_fields.rs"]
mod signal_fields;

fn race_definition(left: Value, condition: Value, schema: &Value) -> Value {
    let mut outcome = no_satisfier();
    outcome["state"] = schema.clone();
    let mut definition = root_with_join(
        vec![left, verifier("b", 1)],
        json!({"kind":"first","when":either(condition)}),
        schema,
    );
    definition["children"][1] = outcome;
    definition
}
#[path = "write_order.rs"]
mod write_order;

#[tokio::test]
async fn impossible_joins_preserve_prior_worker_error_and_guarded_output() {
    let state =
        json!({"kind":"record","fields":{"result":{"type":{"kind":"integer"},"required":false}}});
    for join in [
        json!({"kind":"all"}),
        json!({"kind":"any"}),
        json!({"kind":"quorum","count":2}),
    ] {
        let mut branches =
            vec![json!({"kind":"fail","name":"branch_failed","reason":"branch_failed"})];
        if join["kind"] != "any" {
            branches.push(verifier("pending", 1));
        }
        let root = json!({"kind":"seq","name":"run","state":state,"children":[
            promoted_integer_step("writer","result"),
            {"kind":"par","name":"parallel","state":state,"branches":branches,"join":join,"promotedStatePaths":[]},
            {"kind":"choice","name":"check_writer","state":state,"branches":[{
                "when":{"kind":"in","value":{"name":"writer","source":"error","field":null},"labels":["timeout","crash","malformed","refusal"]},
                "node":{"kind":"fail","name":"writer_failed","reason":"writer_failed"}}],
                "otherwise":{"kind":"succeed","name":"read","output":{"kind":"record","fields":{"result":{"type":{"kind":"integer"},"required":true}}},
                    "bindings":[{"target":["result"],"value":{"source":"state","path":["result"]}}]},"promotedStatePaths":[]}
        ],"promotedStatePaths":[]});
        let graph = reducer_test_support::verified_graph(root, state.clone(), true).await;
        for failed in [true, false] {
            let outcome = if failed {
                WorkerOutcome::declared_failure(openengine_cluster_protocol::WorkerErrorCode::Crash)
            } else {
                success(7)
            };
            let history = [settled(
                SettledSpec::new(1, 1, "writer").position(2),
                outcome,
            )];
            let reduced = one_slot(&graph, &json!({}), &history);
            let expected = if failed {
                TerminalProjection::Failed {
                    context: None,
                    reason: "writer_failed".parse().assert_value(),
                }
            } else {
                TerminalProjection::Succeeded {
                    output: json!({"result":7}),
                }
            };
            assert_eq!(reduced.terminal, Some(expected));
            assert!(
                !reduced
                    .decisions
                    .iter()
                    .any(|decision| matches!(decision, Decision::Dispatch { .. }))
            );
        }
    }
}
