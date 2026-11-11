use super::*;

pub(super) async fn graph_for(left: Value, condition: Value, nested: bool) -> VerifiedGraph {
    super::race_in_scope(left, condition, state(), nested).await
}

fn mapped_history(labels: &[&str], tails_complete: bool, nested: bool) -> Vec<DurableExecution> {
    let outer = if nested { vec![0] } else { vec![] };
    let mut history = Vec::new();
    for (index, label) in labels.iter().enumerate() {
        let mut indices = outer.clone();
        indices.push(index as u64);
        let id = index as u64 * 2 + 1;
        history.push(settled(
            SettledSpec::new(id, id, "a")
                .position(id * 2)
                .map_indices(indices.clone()),
            verdict(label),
        ));
        if tails_complete {
            history.push(settled(
                SettledSpec::new(id + 1, id + 1, "tail")
                    .position(id * 2 + 2)
                    .map_indices(indices),
                success(1),
            ));
        }
    }
    let id = labels.len() as u64 * 2 + 1;
    history.push(settled(
        SettledSpec::new(id, id, "b")
            .position(id * 2)
            .map_indices(outer),
        verdict("rejected"),
    ));
    history
}

#[tokio::test]
async fn fixed_map_verdicts_bound_remaining_acceptances_and_prune_item_tails() {
    for nested in [false, true] {
        let body = sequence("item_body", vec![verifier("a", 1), step("tail", 1)]);
        let graph = graph_for(
            mapped(body.clone(), 1),
            signal("a", "accepted", true),
            nested,
        )
        .await;
        assert_no_satisfier(&one_slot(
            &graph,
            &json!({"items":[null]}),
            &mapped_history(&["rejected"], false, nested),
        ));
        let mut two = signal("a", "accepted", true);
        two["count"] = json!(2);
        let graph = graph_for(
            mapped(body, 1),
            json!({"kind":"all","guards":[two,signal("a","rejected",true)]}),
            nested,
        )
        .await;
        assert_no_satisfier(&one_slot(
            &graph,
            &json!({"items":[null,null,null]}),
            &mapped_history(&["rejected", "rejected"], true, nested),
        ));
        let possible = one_slot(
            &graph,
            &json!({"items":[null,null,null]}),
            &mapped_history(&["accepted", "rejected"], true, nested),
        );
        assert!(
            possible.terminal.is_none(),
            "a fixed acceptance plus the last future item can still qualify"
        );
        assert!(possible.decisions.iter().any(|d|matches!(d,Decision::Dispatch {occurrence,..} if occurrence.node.as_str()=="a" && occurrence.map_indices.last()==Some(&2))));
    }
}

#[tokio::test]
async fn future_choice_routes_share_one_joint_presence_assignment() {
    for aggregate in [false, true] {
        for nested in [false, true] {
            let mut left = gated_branch(verifier("c", 1), "tail");
            if aggregate {
                left = mapped(left, 1);
            }
            check_joint_alternatives(left, aggregate, nested, "gate").await;
        }
    }
}

#[tokio::test]
async fn future_subset_join_retains_only_a_legal_branch_set_per_item() {
    for aggregate in [false, true] {
        let mut left = json!({"kind":"par","name":"inner","state":state(),"branches":[verifier("a",1),verifier("c",1)],"join":{"kind":"any"},"promotedStatePaths":[]});
        if aggregate {
            left = mapped(left, 1);
        }
        check_joint_alternatives(left, aggregate, false, "a").await;
    }
}

#[tokio::test]
async fn immutable_revisited_map_keeps_its_single_final_occurrence() {
    for nested in [false, true] {
        let mut body = sequence(
            "round_body",
            vec![mapped(verifier("a", 1), 1), step("tail", 1)],
        );
        body["state"] = state();
        let repeated = json!({"kind":"loop","name":"twice","state":state(),"body":body,"maxIterations":2,"promotedStatePaths":[]});
        let both = json!({"kind":"all","guards":[signal("a","accepted",true),signal("a","rejected",true)]});
        let graph = graph_for(repeated, both, nested).await;
        assert_no_satisfier(&one_slot(
            &graph,
            &json!({"items":[null]}),
            &mapped_history(&["rejected"], false, nested),
        ));
    }
}

#[tokio::test]
async fn promoted_future_collection_writes_allow_revisited_map_cardinality_to_change() {
    for visible in [false, true] {
        let s = super::continuation::collection_state();
        let (grow, first) = super::continuation::collection_nodes();
        let mut body = json!({"kind":"seq","name":"round","state":s,"children":[first,grow],"promotedStatePaths":[["items"]]});
        body["promotedStatePaths"] = if visible {
            json!([["items"]])
        } else {
            json!([])
        };
        let repeated = json!({"kind":"loop","name":"twice","state":s,"body":body,"maxIterations":2,"promotedStatePaths":[]});
        let cond = json!({"kind":"all","guards":[signal("a","accepted",true),signal("a","rejected",true)]});
        let mut definition = root_with_join(
            vec![repeated, verifier("b", 1)],
            json!({"kind":"first","when":either(cond)}),
            &s,
        );
        let mut outcome = no_satisfier();
        outcome["state"] = s.clone();
        definition["children"][1] = outcome;
        let graph = reducer_test_support::verified_graph(definition, s, true).await;
        let result = one_slot(
            &graph,
            &json!({"items":[1],"next_items":[null,null]}),
            &mapped_history(&["rejected"], false, false),
        );
        if !visible {
            assert_no_satisfier(&result);
        } else {
            assert!(
                result.terminal.is_none(),
                "a future promoted collection can contain both labels in round two"
            );
            assert!(result.decisions.iter().any(
        |d| matches!(d,Decision::Dispatch {occurrence,..} if occurrence.node.as_str()=="copy")
    ));
        }
    }
}

async fn check_joint_alternatives(left: Value, aggregate: bool, nested: bool, next: &str) {
    let both = json!({"kind":"all","guards":[signal("a","accepted",aggregate),signal("c","accepted",aggregate)]});
    let graph = graph_for(left, both, nested).await;
    let history = mapped_history(&[], false, nested);
    assert_no_satisfier(&one_slot(&graph, &json!({"items":[null]}), &history));
    if aggregate {
        let possible = one_slot(&graph, &json!({"items":[null,null]}), &history);
        assert!(
            possible.terminal.is_none(),
            "different items can retain different outcomes"
        );
        assert!(possible.decisions.iter().any(
            |d| matches!(d,Decision::Dispatch {occurrence,..} if occurrence.node.as_str()==next)
        ));
    }
}

#[tokio::test]
async fn settled_subset_joins_do_not_reintroduce_voided_producers() {
    for aggregate in [false, true] {
        let inner = json!({"kind":"par","name":"inner","state":state(),
            "branches":[gated_branch(verifier("unused",1),"tail"),verifier("c",1)],
            "join":{"kind":"any"},"promotedStatePaths":[]});
        let mut left = sequence("after_inner", vec![inner, step("irrelevant_tail", 1)]);
        left["state"] = state();
        if aggregate {
            left = mapped(left, 1);
        }
        let graph = graph_for(left, signal("a", "accepted", aggregate), false).await;
        let history = vec![
            settled(
                SettledSpec::new(1, 1, "c")
                    .position(2)
                    .map_indices(if aggregate { vec![0] } else { vec![] }),
                verdict("rejected"),
            ),
            settled(SettledSpec::new(2, 2, "b").position(3), verdict("rejected")),
        ];
        assert_no_satisfier(&one_slot(&graph, &json!({"items":[null]}), &history));
    }
}

#[tokio::test]
async fn selected_outer_item_stays_immutable_through_state_writes_and_loop_revisits() {
    let s = json!({"kind":"record","fields":{"records":{"type":reducer_test_support::records_array_type(),"required":true}}});
    for revisited in [false, true] {
        let mapped = json!({"kind":"map","name":"children","state":s,"body":verifier("a",1),
            "over":{"source":"item","path":["children"]},"maxItems":1,"promotedStatePaths":[]});
        let mut replace = reducer_test_support::step_node(
            "replace",
            1,
            json!({"kind":"record","fields":{
            "value":{"type":s["fields"]["records"]["type"],"required":true}}}),
        );
        replace["worker"] = json!("worker.record-array@1");
        replace["writeBindings"] = json!([{"value":{"node":"replace","channel":"out","path":["value"]},"target":["records"]}]);
        let mut body = json!({"kind":"seq","name":"round","state":s,"children":[mapped,replace],"promotedStatePaths":[["records"]]});
        let left = if revisited {
            json!({"kind":"loop","name":"twice","state":s,"body":body,"maxIterations":2,"promotedStatePaths":[]})
        } else {
            body["children"]
                .as_array_mut()
                .assert_value()
                .insert(0, verifier("gate", 1));
            body
        };
        let both = json!({"kind":"all","guards":[signal("a","accepted",true),signal("a","rejected",true)]});
        let mut race = root_with_join(
            vec![left, verifier("b", 1)],
            json!({"kind":"first","when":either(both)}),
            &s,
        );
        let mut outcome = no_satisfier();
        outcome["state"] = s.clone();
        race["children"][1] = outcome;
        let outer = json!({"kind":"map","name":"outer_records","state":s,"body":race,
            "over":{"source":"state","path":["records"]},"maxItems":1,"promotedStatePaths":[]});
        let root = json!({"kind":"seq","name":"outer_run","state":s,"children":[outer,succeed("outer_done")],"promotedStatePaths":[]});
        let graph = reducer_test_support::verified_graph(root, s.clone(), true).await;
        let mut history = vec![settled(
            SettledSpec::new(2, 2, "b").position(4).map_indices(vec![0]),
            verdict("rejected"),
        )];
        if revisited {
            history.insert(
                0,
                settled(
                    SettledSpec::new(1, 1, "a")
                        .position(2)
                        .map_indices(vec![0, 0]),
                    verdict("rejected"),
                ),
            );
        }
        assert_no_satisfier(&one_slot(
            &graph,
            &json!({"records":[{"children":[null]}]}),
            &history,
        ));
    }
}
