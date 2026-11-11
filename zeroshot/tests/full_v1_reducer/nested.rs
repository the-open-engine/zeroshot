use super::*;

fn double_map(body: Value) -> Value {
    let mut inner = mapped(body, 1);
    inner["name"] = json!("inner_items");
    mapped(inner, 1)
}
fn rejecting_b(scope: Vec<u64>) -> DurableExecution {
    settled(
        SettledSpec::new(1, 1, "b").position(2).map_indices(scope),
        verdict("rejected"),
    )
}
fn nested_a(id: u64, scope: Vec<u64>, label: &str) -> DurableExecution {
    settled(
        SettledSpec::new(id, id, "a")
            .position(id * 2)
            .map_indices(scope),
        verdict(label),
    )
}

#[tokio::test]
async fn nested_aggregate_availability_and_cardinality_follow_the_full_map_chain() {
    for size in [1, 2] {
        let mut condition = signal("a", "accepted", true);
        condition["count"] = json!(if size == 1 { 1 } else { 3 });
        let graph = race_in_scope(double_map(verifier("a", 1)), condition, state(), false).await;
        let input = json!({"items":vec![Value::Null;size]});
        let mut history = vec![rejecting_b(vec![])];
        let pending = one_slot(&graph, &input, &history);
        assert!(pending.terminal.is_none());
        assert!(pending.decisions.iter().any(|d|matches!(d,Decision::Dispatch{occurrence,..} if occurrence.node.as_str()=="a" && occurrence.map_indices==vec![0,0])));
        for outer in 0..size {
            for inner in 0..size {
                history.push(nested_a(
                    history.len() as u64 + 1,
                    vec![outer as u64, inner as u64],
                    "accepted",
                ));
            }
        }
        assert!(matches!(
            one_slot(&graph, &input, &history).terminal,
            Some(TerminalProjection::Succeeded { .. })
        ));
    }
}

#[tokio::test]
async fn nested_fixed_contributions_prune_tails_and_empty_outcome_witnesses() {
    for enclosed in [false, true] {
        let mut body = sequence("body", vec![verifier("a", 1), step("tail", 1)]);
        body["state"] = state();
        let graph = race_in_scope(
            double_map(body),
            signal("a", "accepted", true),
            state(),
            enclosed,
        )
        .await;
        let outer = if enclosed { vec![0] } else { vec![] };
        let mut indices = outer.clone();
        indices.extend([0, 0]);
        assert_no_satisfier(&one_slot(
            &graph,
            &json!({"items":[null]}),
            &[rejecting_b(outer), nested_a(2, indices, "rejected")],
        ));
    }
    let condition = super::continuation::excludes_every_outcome();
    let graph = race_in_scope(double_map(verifier("a", 1)), condition, state(), false).await;
    assert_no_satisfier(&one_slot(
        &graph,
        &json!({"items":[null]}),
        &[rejecting_b(vec![])],
    ));
    assert!(matches!(
        one_slot(&graph, &json!({"items":[]}), &[rejecting_b(vec![])]).terminal,
        Some(TerminalProjection::Succeeded { .. })
    ));
}

#[tokio::test]
async fn nested_terminal_maps_exclude_body_and_later_item_producers() {
    for later in [false, true] {
        let mut body = sequence(
            "inner_body",
            vec![
                verifier("a", 1),
                json!({"kind":"fail","name":"end","reason":"terminal"}),
            ],
        );
        body["state"] = state();
        let mut inner = mapped(body, 1);
        inner["name"] = json!("inner_items");
        let body = if later {
            json!({"kind":"seq","name":"outer_body","state":state(),"children":[inner,verifier("c",1)],"promotedStatePaths":[]})
        } else {
            inner
        };
        let graph = race_in_scope(
            mapped(body, 1),
            signal(if later { "c" } else { "a" }, "accepted", true),
            state(),
            false,
        )
        .await;
        assert_no_satisfier(&one_slot(
            &graph,
            &json!({"items":[null]}),
            &[rejecting_b(vec![])],
        ));
    }
}

#[tokio::test]
async fn completion_dependencies_keep_other_maps_joint_scoped_counts() {
    for terminates in [false, true] {
        let finish = json!({"kind":"choice","name":"finish","state":state(),"branches":[{"when":signal("a","accepted",true),"node":if terminates {json!({"kind":"fail","name":"end","reason":"terminal"})}else{step("tail",1)}}],"otherwise":if terminates {step("other_tail",1)}else{json!({"kind":"fail","name":"end","reason":"terminal"})},"promotedStatePaths":[]});
        let body = json!({"kind":"seq","name":"second_body","state":state(),"children":[verifier("c",1),finish],"promotedStatePaths":[]});
        let mut second = mapped(body, 1);
        second["name"] = json!("second_map");
        let left = json!({"kind":"seq","name":"left","state":state(),"children":[mapped(verifier("a",1),1),second],"promotedStatePaths":[]});
        let graph = race_in_scope(left, signal("c", "accepted", true), state(), false).await;
        let result = one_slot(
            &graph,
            &json!({"items":[null]}),
            &[
                nested_a(1, vec![0], "accepted"),
                settled(SettledSpec::new(2, 2, "b").position(4), verdict("rejected")),
            ],
        );
        if terminates {
            assert_no_satisfier(&result);
        } else {
            assert!(result.terminal.is_none());
            assert!(result.decisions.iter().any(
                |d| matches!(d,Decision::Dispatch{occurrence,..} if occurrence.node.as_str()=="c")
            ));
        }
    }
}

#[tokio::test]
async fn impossible_join_terminal_replay_retains_late_pending_prefix_evidence() {
    for aggregate in [false, true] {
        for reversed in [false, true] {
            let producer = if aggregate {
                mapped(verifier("a", 1), 1)
            } else {
                verifier("a", 1)
            };
            let left = json!({"kind":"seq","name":"left","state":state(),"children":[producer,step("tail",1)],"promotedStatePaths":[]});
            let graph =
                race_in_scope(left, signal("a", "accepted", aggregate), state(), true).await;
            let mut indices = vec![0];
            if aggregate {
                indices.push(0);
            }
            let (a, b) = if reversed { (2, 1) } else { (1, 2) };
            let history = [
                nested_a(a, indices, "rejected"),
                settled(
                    SettledSpec::new(b, b, "b")
                        .position(b * 2)
                        .map_indices(vec![0]),
                    verdict("rejected"),
                ),
            ];
            assert_no_satisfier(&one_slot(&graph, &json!({"items":[null]}), &history));
        }
    }
}

#[tokio::test]
async fn conditional_collection_growth_keeps_its_choice_guard_in_each_scope() {
    for nested in [false, true] {
        for gate_label in ["accepted", "rejected"] {
            let (grow, target) = super::continuation::collection_nodes();
            let schema = super::continuation::collection_state();
            let route = json!({"kind":"choice","name":"route","state":schema,"branches":[{"when":signal("gate","accepted",false),"node":grow}],"otherwise":step("noop",1),"promotedStatePaths":[["items"]]});
            let left = json!({"kind":"seq","name":"left","state":schema,"children":[verifier("gate",1),route,target],"promotedStatePaths":[]});
            let condition = json!({"kind":"all","guards":[signal("gate",gate_label,false),signal("a","accepted",true),signal("a","rejected",true)]});
            let graph = race_in_scope(left, condition, schema, nested).await;
            let result = one_slot(
                &graph,
                &json!({"items":[1],"next_items":[null,null]}),
                &[rejecting_b(if nested { vec![0] } else { vec![] })],
            );
            if gate_label == "rejected" {
                assert_no_satisfier(&result);
            } else {
                assert!(result.terminal.is_none());
            }
        }
    }
}
