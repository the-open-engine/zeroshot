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
                    verdict("rejected"),
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
                verdict("accepted"),
            ));
            history.push(settled(
                SettledSpec::new(5, 3, "round_tail").position(6),
                success(2),
            ));
            let complete = one_slot(&graph, &input, &history);
            assert_eq!(
                complete.terminal,
                Some(TerminalProjection::Succeeded {
                    output: Value::Null
                })
            );
        }
    }
}

#[tokio::test]
async fn skipped_choice_producer_cannot_keep_scalar_or_map_tail_alive() {
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
            let choice = gate_choice(producer, step("fallback", 1));
            let mut left = sequence("left", vec![choice, step("irrelevant_tail", 1)]);
            left["state"] = state();
            let race = json!({"kind":"par","name":"race","state":state(),"branches":[left,verifier("b",1)],
            "join":{"kind":"first","when":either(condition)},"promotedStatePaths":[]});
            let graph = reducer_test_support::verified_graph(
                root(vec![verifier("gate", 1), race, no_satisfier()]),
                state(),
                true,
            )
            .await;
            let mut history = skipped_history();
            history.push(settled(
                SettledSpec::new(3, 3, "b").position(4),
                verdict("rejected"),
            ));
            assert_no_satisfier(&one_slot(&graph, &json!({"items":[null]}), &history));
        }
    }
}

#[tokio::test]
async fn future_outcomes_preserve_signal_labels_and_signal_error_exclusivity() {
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
            let contradiction =
                json!({"kind":"all","guards":[signal("a","accepted",aggregate),second]});
            let producer = if aggregate {
                mapped(verifier("a", 1), 1)
            } else {
                verifier("a", 1)
            };
            let graph = checked_race(vec![producer, verifier("b", 1)], either(contradiction)).await;
            let history = [settled(
                SettledSpec::new(1, 1, "b").position(2),
                verdict("rejected"),
            )];
            assert_no_satisfier(&one_slot(&graph, &json!({"items":[null]}), &history));
            if aggregate {
                // Two distinct items can legitimately emit different labels or signal/error.
                let still_possible = one_slot(&graph, &json!({"items":[null,null]}), &history);
                assert!(still_possible.terminal.is_none());
                assert!(still_possible.decisions.iter().any(|d|matches!(d,Decision::Dispatch {occurrence,..} if occurrence.node.as_str()=="a")));
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
async fn pending_map_viability_counts_settled_items_and_future_aggregate_availability() {
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
async fn nested_item_races_use_selected_flow_and_replaceable_loop_facts() {
    for scenario in ["loop", "skipped", "contradictory"] {
        let mut condition = either(signal("a", "accepted", false));
        let (branch, mut history) = match scenario {
            "loop" => (
                json!({"kind":"loop","name":"twice","state":state(),"maxIterations":2,
                    "body":sequence("round",vec![verifier("a",1),step("tail",1)]),"promotedStatePaths":[]}),
                vec![settled(
                    SettledSpec::new(1, 1, "a").position(2),
                    verdict("rejected"),
                )],
            ),
            "skipped" => {
                let left = gated_branch(step("fallback", 1), "tail");
                (left, skipped_history())
            }
            _ => {
                condition = either(
                    json!({"kind":"all","guards":[signal("a","accepted",false),signal("a","rejected",false)]}),
                );
                (verifier("a", 1), vec![])
            }
        };
        history.push(settled(
            SettledSpec::new(3, 3, "b").position(4),
            verdict("rejected"),
        ));
        for execution in &mut history {
            execution.occurrence.map_indices = vec![0];
        }
        let mut body = root_with_join(
            vec![branch, verifier("b", 1)],
            json!({"kind":"first","when":condition}),
            &state(),
        );
        body["children"][1] = no_satisfier();
        let nested = json!({"kind":"map","name":"outer_items","state":state(),"maxItems":1,
            "over":{"source":"state","path":["items"]},"body":body,"promotedStatePaths":[]});
        let outer = json!({"kind":"seq","name":"enclosing_run","state":state(),
            "children":[nested,succeed("outer_done")],"promotedStatePaths":[]});
        let spec: GraphSpec = serde_json::from_value(json!({
            "profile":"openengine.graph.full/v1","initialInput":state(),
            "policy":{"policy":"policy.test@1","default":"deny"},"root":outer
        }))
        .assert_value();
        let verified = ProductionGraphVerifier::new(TestWorkers { rich_outputs: true })
            .verify(&spec)
            .await;
        assert!(verified.is_ok(), "nested {scenario}: {verified:?}");
        let graph = verified.assert_value();
        let reduced = one_slot(&graph, &json!({"items":[null]}), &history);
        if scenario == "loop" {
            assert!(reduced.terminal.is_none());
            assert!(reduced.decisions.iter().any(|d|matches!(d,Decision::Dispatch {occurrence,..} if occurrence.node.as_str()=="tail" && occurrence.map_indices==[0])));
        } else {
            assert_no_satisfier(&reduced);
        }
    }
}

fn skipped_history() -> Vec<DurableExecution> {
    vec![
        settled(
            SettledSpec::new(1, 1, "gate").position(2),
            verdict("rejected"),
        ),
        settled(SettledSpec::new(2, 2, "fallback").position(3), success(1)),
    ]
}

#[path = "residual.rs"]
mod residual;

fn gate_choice(producer: Value, otherwise: Value) -> Value {
    json!({"kind":"choice","name":"route","state":state(),
        "branches":[{"when":signal("gate","accepted",false),"node":producer}],
        "otherwise":otherwise,"promotedStatePaths":[]})
}

fn gated_branch(otherwise: Value, tail: &str) -> Value {
    let choice = gate_choice(verifier("a", 1), otherwise);
    let mut left = sequence("left", vec![verifier("gate", 1), choice, step(tail, 1)]);
    left["state"] = state();
    left
}

mod continuation;

mod nested;

async fn race_in_scope(
    left: Value,
    condition: Value,
    schema: Value,
    nested: bool,
) -> VerifiedGraph {
    let mut definition = race_definition(left, condition, &schema);
    if nested {
        let outer = json!({"kind":"map","name":"outer_items","state":schema,"body":definition,"over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]});
        definition = json!({"kind":"seq","name":"outer_run","state":schema,"children":[outer,succeed("outer_done")],"promotedStatePaths":[]});
    }
    reducer_test_support::verified_graph(definition, schema, true).await
}

mod scoped;

mod loop_outcomes;

#[path = "loop_writes.rs"]
mod loop_writes;

#[path = "signal_fields.rs"]
mod signal_fields;

#[path = "write_order.rs"]
mod write_order;

fn race_definition(left: Value, condition: Value, schema: &Value) -> Value {
    let mut definition = root_with_join(
        vec![left, verifier("b", 1)],
        json!({"kind":"first","when":either(condition)}),
        schema,
    );
    let mut outcome = no_satisfier();
    outcome["state"] = schema.clone();
    definition["children"][1] = outcome;
    definition
}

#[path = "loop_exits.rs"]
mod loop_exits;
