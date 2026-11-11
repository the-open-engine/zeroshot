use super::*;

pub(super) fn collection_state() -> Value {
    json!({"kind":"record","fields":{
        "items":{"type":{"kind":"array","items":{"kind":"integer"}},"required":true},
        "next_items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}})
}

pub(super) fn collection_nodes() -> (Value, Value) {
    let s = collection_state();
    let mut copy = step("copy", 1);
    copy["writeBindings"] =
        json!([{"value":{"node":"copy","channel":"out","path":["value"]},"target":["items"]}]);
    let grow = json!({"kind":"map","name":"grow","state":s,"body":copy,"over":{"source":"state","path":["next_items"]},"maxItems":1,"promotedStatePaths":[["items"]]});
    let target = json!({"kind":"map","name":"items_map","state":s,"body":verifier("a",1),"over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]});
    (grow, target)
}

async fn collection_race(left: Value, condition: Value) -> VerifiedGraph {
    super::race_in_scope(left, condition, collection_state(), false).await
}

pub(super) fn opposite_verdicts() -> Value {
    json!({"kind":"all","guards":[signal("a","accepted",true),signal("a","rejected",true)]})
}

fn rejected_b(id: u64) -> DurableExecution {
    settled(
        SettledSpec::new(id, id, "b").position(id * 2),
        verdict("rejected"),
    )
}

#[tokio::test]
async fn future_selector_uses_completed_prefix_without_promoting_private_branch_state() {
    let (grow, target) = collection_nodes();
    let left = json!({"kind":"seq","name":"left","state":collection_state(),"children":[grow,verifier("gate",1),target],"promotedStatePaths":[]});
    let graph = collection_race(left, opposite_verdicts()).await;
    let input = json!({"items":[1],"next_items":[null,null]});
    let mut history = vec![
        settled(
            SettledSpec::new(1, 1, "copy")
                .position(2)
                .map_indices(vec![0]),
            success(4),
        ),
        settled(
            SettledSpec::new(2, 2, "copy")
                .position(4)
                .map_indices(vec![1]),
            success(5),
        ),
        rejected_b(3),
    ];
    let result = one_slot(&graph, &input, &history);
    assert_gate_pending(&result);
    history.extend([
        settled(
            SettledSpec::new(4, 4, "gate").position(8),
            verdict("accepted"),
        ),
        settled(
            SettledSpec::new(5, 5, "a")
                .position(10)
                .map_indices(vec![0]),
            verdict("accepted"),
        ),
        settled(
            SettledSpec::new(6, 6, "a")
                .position(12)
                .map_indices(vec![1]),
            verdict("rejected"),
        ),
    ]);
    assert!(matches!(
        one_slot(&graph, &input, &history).terminal,
        Some(TerminalProjection::Succeeded { .. })
    ));
}

#[tokio::test]
async fn collection_dependencies_respect_local_order_promotions_and_loop_carry() {
    for placement in [
        "before",
        "hidden",
        "after",
        "inside_loop",
        "carried",
        "not_carried",
    ] {
        let (grow, target) = collection_nodes();
        let s = collection_state();
        let children = match placement {
            "hidden" => vec![
                verifier("gate", 1),
                json!({"kind":"seq","name":"hidden","state":s,"children":[grow],"promotedStatePaths":[]}),
                target,
            ],
            "after" | "carried" | "not_carried" => vec![verifier("gate", 1), target, grow],
            _ => vec![verifier("gate", 1), grow, target],
        };
        let mut left = json!({"kind":"seq","name":"body","state":s,"children":children,"promotedStatePaths":[]});
        if placement == "carried" {
            left["promotedStatePaths"] = json!([["items"]]);
        }
        if ["inside_loop", "carried", "not_carried"].contains(&placement) {
            left = json!({"kind":"loop","name":"twice","state":s,"body":left,"maxIterations":2,"promotedStatePaths":[]});
        }
        let graph = collection_race(left, opposite_verdicts()).await;
        let result = one_slot(
            &graph,
            &json!({"items":[1],"next_items":[null,null]}),
            &[rejected_b(1)],
        );
        if ["before", "inside_loop", "carried"].contains(&placement) {
            assert_gate_pending(&result);
        } else {
            assert_no_satisfier(&result);
        }
    }
}

#[tokio::test]
async fn stable_exit_controls_prevent_scalar_and_mapped_loop_replacement() {
    // Empty maps cannot guarantee an item signal: mapped cases use a scalar exit producer.
    for (aggregate, nested, separate_gate) in [
        (false, false, false),
        (false, true, false),
        (false, false, true),
        (false, true, true),
        (true, false, true),
        (true, true, true),
    ] {
        let producer = if aggregate {
            mapped(verifier("a", 1), 1)
        } else {
            verifier("a", 1)
        };
        let mut children = vec![producer, step("tail", 1)];
        let until = if separate_gate {
            children.insert(0, verifier("gate", 1));
            signal("gate", "accepted", false)
        } else {
            signal("a", "rejected", aggregate)
        };
        let body = json!({"kind":"seq","name":"body","state":state(),"children":children,"promotedStatePaths":[]});
        let left = json!({"kind":"loop","name":"twice","state":state(),"body":body,"maxIterations":2,"until":until,"promotedStatePaths":[]});
        let graph = super::residual::graph_for(
            left,
            signal(
                "a",
                if separate_gate {
                    "rejected"
                } else {
                    "accepted"
                },
                aggregate,
            ),
            nested,
        )
        .await;
        let scope = if nested { vec![0] } else { vec![] };
        let mut a_scope = scope.clone();
        if aggregate {
            a_scope.push(0);
        }
        let mut history = vec![
            settled(
                SettledSpec::new(2, 2, "a").position(4).map_indices(a_scope),
                verdict(if separate_gate {
                    "accepted"
                } else {
                    "rejected"
                }),
            ),
            settled(
                SettledSpec::new(3, 3, "b")
                    .position(6)
                    .map_indices(scope.clone()),
                verdict("rejected"),
            ),
        ];
        if separate_gate {
            history.insert(
                0,
                settled(
                    SettledSpec::new(1, 1, "gate")
                        .position(2)
                        .map_indices(scope.clone()),
                    verdict("accepted"),
                ),
            );
        }
        assert_no_satisfier(&one_slot(&graph, &json!({"items":[null]}), &history));
        history.push(settled(
            SettledSpec::new(4, 4, "tail")
                .position(8)
                .map_indices(scope),
            success(1),
        ));
        assert_no_satisfier(&one_slot(&graph, &json!({"items":[null]}), &history));
    }
}

#[tokio::test]
async fn exact_nonempty_maps_cannot_omit_every_legal_signal_and_error() {
    let condition = excludes_every_outcome();
    let graph = super::residual::graph_for(mapped(verifier("a", 1), 1), condition, false).await;
    assert!(matches!(
        one_slot(&graph, &json!({"items":[]}), &[rejected_b(1)]).terminal,
        Some(TerminalProjection::Succeeded { .. })
    ));
    for count in [1, 2, 33, 1025, 65536] {
        assert_no_satisfier(&one_slot(
            &graph,
            &json!({"items":vec![Value::Null;count]}),
            &[rejected_b(1)],
        ));
    }
}

#[tokio::test]
async fn map_normal_completion_requires_each_selected_body_to_finish() {
    for nested in [false, true] {
        for conditional in [false, true] {
            let end = if conditional {
                json!({"kind":"choice","name":"finish","state":state(),"branches":[{"when":signal("a","accepted",false),"node":{"kind":"fail","name":"end","reason":"terminal"}}],"otherwise":step("tail",1),"promotedStatePaths":[]})
            } else {
                json!({"kind":"fail","name":"end","reason":"terminal"})
            };
            let mut body = sequence("body", vec![verifier("a", 1), end]);
            body["state"] = state();
            let graph =
                super::residual::graph_for(mapped(body, 1), signal("a", "accepted", true), nested)
                    .await;
            let scope = if nested { vec![0] } else { vec![] };
            assert_no_satisfier(&one_slot(
                &graph,
                &json!({"items":[null]}),
                &[settled(
                    SettledSpec::new(1, 1, "b").position(2).map_indices(scope),
                    verdict("rejected"),
                )],
            ));
        }
    }
}

fn assert_gate_pending(result: &Reduction) {
    assert!(
        result.terminal.is_none(),
        "a possible collection must remain viable: {result:#?}"
    );
    assert!(result.decisions.iter().any(
        |d| matches!(d,Decision::Dispatch{occurrence,..} if occurrence.node.as_str()=="gate")
    ));
}

pub(super) fn excludes_every_outcome() -> Value {
    let errors = json!({"kind":"k_of_map","count":1,"value":{"name":"a","source":"error","field":null},"labels":["crash","timeout","malformed","refusal"]});
    json!({"kind":"all","guards":[{"kind":"not","guard":signal("a","accepted",true)},{"kind":"not","guard":signal("a","rejected",true)},{"kind":"not","guard":errors}]})
}
