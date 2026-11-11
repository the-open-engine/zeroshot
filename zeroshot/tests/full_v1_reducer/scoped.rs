use super::*;

pub(super) fn entry(id: u64, name: &str, label: &str, indices: Vec<u64>) -> DurableExecution {
    settled(
        SettledSpec::new(id, id, name)
            .position(id * 2)
            .map_indices(indices),
        verdict(label),
    )
}
pub(super) fn done(id: u64, name: &str, indices: Vec<u64>) -> DurableExecution {
    settled(
        SettledSpec::new(id, id, name)
            .position(id * 2)
            .map_indices(indices),
        success(1),
    )
}
pub(super) fn seq(name: &str, children: Vec<Value>, schema: &Value) -> Value {
    json!({"kind":"seq","name":name,"state":schema,"children":children,"promotedStatePaths":[]})
}
fn map(name: &str, body: Value) -> Value {
    let mut definition = mapped(body, 1);
    definition["name"] = json!(name);
    definition
}
fn both(a: Value, b: Value) -> Value {
    json!({"kind":"all","guards":[a,b]})
}

#[tokio::test]
async fn stable_pre_race_aggregate_conditions_scalar_and_mapped_completion() {
    for aggregate in [false, true] {
        for label in ["accepted", "rejected"] {
            let s = state();
            let finish = json!({"kind":"choice","name":"finish","state":s,"branches":[{"when":signal("a","accepted",true),"node":step("tail",1)}],"otherwise":{"kind":"fail","name":"end","reason":"terminal"},"promotedStatePaths":[]});
            let mut left = seq("left", vec![verifier("c", 1), finish], &s);
            if aggregate {
                left = map("c_map", left);
            }
            let mut race = root_with_join(
                vec![left, verifier("b", 1)],
                json!({"kind":"first","when":either(signal("c","accepted",aggregate))}),
                &s,
            );
            race["children"][1] = no_satisfier();
            let graph = reducer_test_support::verified_graph(
                seq("before", vec![map("prior", verifier("a", 1)), race], &s),
                s,
                true,
            )
            .await;
            let input = json!({"items":[null]});
            let mut history = vec![
                entry(1, "a", label, vec![0]),
                entry(2, "b", "rejected", vec![]),
            ];
            let partial = one_slot(&graph, &input, &history);
            if label == "rejected" {
                assert_no_satisfier(&partial);
                continue;
            }
            assert!(partial.terminal.is_none());
            assert!(partial.decisions.iter().any(
                |d| matches!(d,Decision::Dispatch{occurrence,..} if occurrence.node.as_str()=="c")
            ));
            let scope = if aggregate { vec![0] } else { vec![] };
            history.push(entry(3, "c", "accepted", scope.clone()));
            history.push(done(4, "tail", scope));
            assert!(matches!(
                one_slot(&graph, &input, &history).terminal,
                Some(TerminalProjection::Succeeded { .. })
            ));
        }
    }
}

fn nested_choice(alternative: bool) -> Value {
    let s = state();
    let otherwise = if alternative {
        map("c_map", verifier("c", 1))
    } else {
        step("noop", 1)
    };
    let choice = json!({"kind":"choice","name":"choose","state":s,"branches":[{"when":signal("gate","accepted",false),"node":map("a_map",verifier("a",1))}],"otherwise":otherwise,"promotedStatePaths":[]});
    map(
        "outer",
        seq("outer_body", vec![verifier("gate", 1), choice], &s),
    )
}

#[tokio::test]
async fn nested_presence_is_selected_per_outer_item_in_admission_and_reduction() {
    let condition = both(signal("a", "accepted", true), signal("c", "accepted", true));
    let graph = checked_race(vec![nested_choice(true), verifier("b", 1)], condition).await;
    let b = entry(1, "b", "rejected", vec![]);
    let input = json!({"items":[null,null]});
    let pending = one_slot(&graph, &input, std::slice::from_ref(&b));
    assert!(pending.terminal.is_none());
    let dispatched = pending
        .decisions
        .iter()
        .find_map(|decision| {
            if let Decision::Dispatch { occurrence, .. } = decision {
                Some(occurrence)
            } else {
                None
            }
        })
        .expect("a viable nested item starts");
    assert_eq!(dispatched.node.as_str(), "gate");
    let full = [
        b.clone(),
        entry(2, "gate", "accepted", vec![0]),
        entry(3, "a", "accepted", vec![0, 0]),
        entry(4, "a", "accepted", vec![0, 1]),
        entry(5, "gate", "rejected", vec![1]),
        entry(6, "c", "accepted", vec![1, 0]),
        entry(7, "c", "accepted", vec![1, 1]),
    ];
    assert!(matches!(
        one_slot(&graph, &input, &full).terminal,
        Some(TerminalProjection::Succeeded { .. })
    ));
    // A single outer occurrence cannot take both choice arms.
    assert_no_satisfier(&one_slot(&graph, &json!({"items":[null]}), &[b]));
}

#[tokio::test]
async fn omitted_nested_maps_contribute_no_future_occurrences() {
    let mut three = signal("a", "accepted", true);
    three["count"] = json!(3);
    let condition = both(
        both(
            signal("gate", "accepted", true),
            signal("gate", "rejected", true),
        ),
        three,
    );
    let graph = checked_race(
        vec![nested_choice(false), verifier("b", 1)],
        either(condition),
    )
    .await;
    assert_no_satisfier(&one_slot(
        &graph,
        &json!({"items":[null,null]}),
        &[entry(1, "b", "rejected", vec![])],
    ));
}

#[tokio::test]
async fn queued_item_collections_preserve_exact_heterogeneous_cardinalities() {
    let s = item_record("items", item_record("children", json!({"kind":"null"})));
    for gate in [false, true] {
        for (sizes, required, viable) in [
            (vec![0, 0], 1, false),
            (vec![1, 0], 2, false),
            (vec![0, 1], 1, true),
        ] {
            let inner = json!({"kind":"map","name":"inner","state":s,"body":verifier("a",1),"over":{"source":"item","path":["children"]},"maxItems":1,"promotedStatePaths":[]});
            let mut children = vec![];
            if gate {
                children.push(step("gate", 1));
            }
            children.extend([inner, step("tail", 1)]);
            let left = json!({"kind":"map","name":"outer","state":s,"body":seq("body",children,&s),"over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]});
            let mut guard = signal("a", "accepted", true);
            guard["count"] = json!(required);
            let graph = race_in_scope(left, guard, s.clone(), false).await;
            let input = json!({"items":sizes.iter().map(|size|json!({"children":vec![Value::Null;*size]})).collect::<Vec<_>>()});
            let mut history = vec![entry(1, "b", "rejected", vec![])];
            let partial = one_slot(&graph, &input, &history);
            if !viable {
                assert_no_satisfier(&partial);
                continue;
            }
            assert!(partial.terminal.is_none());
            let mut id = 2;
            for outer in 0..2 {
                if gate {
                    history.push(done(id, "gate", vec![outer]));
                    id += 1;
                }
                if outer == 1 {
                    history.push(entry(id, "a", "accepted", vec![outer, 0]));
                    id += 1;
                }
                history.push(done(id, "tail", vec![outer]));
                id += 1;
                let result = one_slot(&graph, &input, &history);
                if outer == 0 {
                    assert!(result.terminal.is_none());
                } else {
                    assert!(matches!(
                        result.terminal,
                        Some(TerminalProjection::Succeeded { .. })
                    ));
                }
            }
        }
    }
}

#[tokio::test]
async fn pre_race_nested_aggregate_is_available_inside_its_unfinished_outer_map() {
    let s = state();
    let condition = both(
        signal("a", "accepted", true),
        signal("c", "accepted", false),
    );
    let mut race = root_with_join(
        vec![verifier("c", 1), verifier("b", 1)],
        json!({"kind":"first","when":condition}),
        &s,
    );
    race["name"] = json!("inner_race_run");
    let body = seq(
        "outer_body",
        vec![map("preceding", verifier("a", 1)), race],
        &s,
    );
    let graph = reducer_test_support::verified_graph(
        seq(
            "nested_run",
            vec![map("outer", body), succeed("done_outer")],
            &s,
        ),
        s,
        true,
    )
    .await;
    let history = [
        entry(1, "a", "accepted", vec![0, 0]),
        entry(2, "b", "rejected", vec![0]),
    ];
    let input = json!({"items":[null]});
    let pending = one_slot(&graph, &input, &history);
    assert!(pending.terminal.is_none());
    assert!(pending.decisions.iter().any(|decision|matches!(decision,Decision::Dispatch{occurrence,..} if occurrence.node.as_str()=="c" && occurrence.map_indices==[0])));
    let mut full = history.to_vec();
    full.push(entry(3, "c", "accepted", vec![0]));
    assert!(matches!(
        one_slot(&graph, &input, &full).terminal,
        Some(TerminalProjection::Succeeded { .. })
    ));
}

fn triple_item_map() -> (Value, Value) {
    let s = item_record(
        "items",
        item_record("children", item_record("leaves", json!({"kind":"null"}))),
    );
    let deep = json!({"kind":"map","name":"deep","state":s,"body":verifier("a",1),"over":{"source":"item","path":["leaves"]},"maxItems":1,"promotedStatePaths":[]});
    let inner = json!({"kind":"map","name":"inner","state":s,"body":deep,"over":{"source":"item","path":["children"]},"maxItems":1,"promotedStatePaths":[]});
    let mut outer = map("outer", seq("body", vec![inner, step("tail", 1)], &s));
    outer["state"] = s.clone();
    (s, outer)
}

#[tokio::test]
async fn queued_projection_keeps_settled_cardinalities_at_every_map_level() {
    for label in ["rejected", "accepted"] {
        let (schema, left) = triple_item_map();
        let mut condition = signal("a", "accepted", true);
        condition["count"] = json!(2);
        let graph = race_in_scope(left, condition, schema, false).await;
        let input =
            json!({"items":[{"children":[{"leaves":[null]}]},{"children":[{"leaves":[null]}]}]});
        let history = vec![
            entry(1, "b", "rejected", vec![]),
            entry(2, "a", label, vec![0, 0, 0]),
        ];
        let pending = one_slot(&graph, &input, &history);
        let mut full = history;
        full.extend([
            done(3, "tail", vec![0]),
            entry(4, "a", "accepted", vec![1, 0, 0]),
            done(5, "tail", vec![1]),
        ]);
        if label == "rejected" {
            assert_no_satisfier(&pending);
            assert_no_satisfier(&one_slot(&graph, &input, &full));
        } else {
            assert!(pending.terminal.is_none());
            assert_eq!(dispatches(&pending), vec![("tail", &[0_u64][..])]);
            assert_eq!(
                one_slot(&graph, &input, &full).terminal,
                Some(TerminalProjection::Succeeded {
                    output: Value::Null
                })
            );
        }
    }
}

fn legal_exit(exhaustive: bool) -> Value {
    let accepted = signal("gate", "accepted", false);
    if !exhaustive {
        return json!({"kind":"any","guards":[accepted.clone(),{"kind":"not","guard":accepted}]});
    }
    let error = json!({"kind":"in","value":{"name":"gate","source":"error","field":null},"labels":["crash","timeout","malformed","refusal"]});
    json!({"kind":"any","guards":[accepted,signal("gate","rejected",false),error]})
}

#[tokio::test]
async fn joint_exit_outcomes_gate_active_and_future_loop_carry() {
    for prefix in [false, true] {
        for nested in [false, true] {
            for exit in ["tautology", "exhaustive", "possible"] {
                let schema = super::continuation::collection_state();
                let (grow, target) = super::continuation::collection_nodes();
                let mut body = seq("body", vec![verifier("gate", 1), target, grow], &schema);
                body["promotedStatePaths"] = json!([["items"]]);
                let until = if exit == "possible" {
                    signal("gate", "accepted", false)
                } else {
                    legal_exit(exit == "exhaustive")
                };
                let repeated = json!({"kind":"loop","name":"twice","state":schema,"maxIterations":2,"until":until,"body":body,"promotedStatePaths":[]});
                let left = if prefix {
                    seq("left", vec![step("prefix", 1), repeated], &schema)
                } else {
                    repeated
                };
                let graph = race_in_scope(
                    left,
                    both(signal("a", "accepted", true), signal("a", "rejected", true)),
                    schema,
                    nested,
                )
                .await;
                let scope = if nested { vec![0] } else { vec![] };
                let result = one_slot(
                    &graph,
                    &json!({"items":[1],"next_items":[null,null]}),
                    &[entry(1, "b", "rejected", scope)],
                );
                if exit == "possible" {
                    assert!(result.terminal.is_none());
                    let admitted = dispatches(&result);
                    assert_eq!(admitted.len(), 1);
                    assert_eq!(admitted[0].0, if prefix { "prefix" } else { "gate" });
                } else {
                    assert_no_satisfier(&result);
                }
            }
        }
    }
}

fn dispatches(result: &Reduction) -> Vec<(&str, &[u64])> {
    result
        .decisions
        .iter()
        .filter_map(|decision| {
            if let Decision::Dispatch { occurrence, .. } = decision {
                Some((occurrence.node.as_str(), occurrence.map_indices.as_slice()))
            } else {
                None
            }
        })
        .collect()
}

fn item_record(field: &str, item: Value) -> Value {
    json!({"kind":"record","fields":{field:{"type":{"kind":"array","items":item},"required":true}}})
}
