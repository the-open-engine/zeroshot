use super::*;

fn copy(name: &str, source: &str, target: &str, schema: &Value) -> Value {
    let mut leaf = step(name, 1);
    leaf["writeBindings"] =
        json!([{"value":{"node":name,"channel":"out","path":["value"]},"target":[target]}]);
    json!({"kind":"map","name":format!("map_{name}"),"state":schema,"body":leaf,"over":{"source":"state","path":[source]},"maxItems":1,"promotedStatePaths":[[target]]})
}
fn swap_loop(rounds: u64, queued: bool) -> (Value, Value) {
    swap_loop_with_scope(rounds, queued, false)
}
fn swap_loop_with_scope(rounds: u64, queued: bool, descendant: bool) -> (Value, Value) {
    let schema = required_array_record(&[
        ("items", "integer"),
        ("other", "integer"),
        ("temp", "integer"),
    ]);
    let target = json!({"kind":"map","name":"target","state":schema,"body":verifier("a",1),"over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]});
    let names = if descendant {
        vec!["copy_x", "copy_y"]
    } else {
        vec!["save", "copy_x", "copy_y"]
    };
    let errors = names.into_iter().map(|name|json!({"kind":"k_of_map","count":1,"value":{"name":name,"source":"error","field":null},"labels":["crash","timeout","malformed","refusal"]})).collect::<Vec<_>>();
    let checked = json!({"kind":"choice","name":"check_copies","state":schema,"branches":[{"when":{"kind":"any","guards":errors},"node":{"kind":"fail","name":"copy_failed","reason":"copy_failed"}}],"otherwise":step("copy_ok",1),"promotedStatePaths":[]});
    let mut children = vec![
        target,
        copy("save", "items", "temp", &schema),
        copy("copy_x", "other", "items", &schema),
        copy("copy_y", "temp", "other", &schema),
        checked,
        verifier("exit", 1),
        step("tail", 1),
    ];
    if descendant {
        children.splice(1..4, [json!({"kind":"par","name":"swaps","state":schema,"branches":[copy("copy_x","other","items",&schema),copy("copy_y","items","other",&schema)],"join":{"kind":"all"},"promotedStatePaths":[["items"],["other"]]})]);
    }
    let mut body = seq("body", children, &schema);
    body["promotedStatePaths"] = json!([["items"], ["other"], ["temp"]]);
    let repeated = json!({"kind":"loop","name":"repeat","state":schema,"body":body,"maxIterations":rounds,"until":signal("exit","accepted",false),"promotedStatePaths":[]});
    let left = if queued {
        seq("left", vec![step("prefix", 1), repeated], &schema)
    } else {
        repeated
    };
    let left = if descendant {
        json!({"kind":"map","name":"racer_map","state":schema,"body":left,"over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]})
    } else {
        left
    };
    (left, schema)
}
fn winning_history(scope: &[u64], queued: bool, descendant: bool) -> Vec<DurableExecution> {
    let mut history = vec![entry(1, "b", "rejected", scope.to_vec())];
    let mut scope = scope.to_vec();
    if descendant {
        scope.push(0);
    }
    if queued {
        history.push(done(2, "prefix", scope.to_vec()));
    }
    for round in 0..2 {
        append_round(&mut history, round, &scope, descendant);
    }
    normalize_instances(&mut history);
    history
}

#[tokio::test]
async fn intermediate_convergence_preserves_real_winners_at_every_prefix() {
    for (nested, queued) in [(false, false), (true, false), (false, true), (true, true)] {
        for rounds in [2, 3, 4, 5] {
            let (left, schema) = swap_loop(rounds, queued);
            let graph = race_in_scope(left, opposite_verdicts(), schema, nested).await;
            let (input, history) = inputs_and_history(nested, queued, false);
            assert!(matches!(
                one_slot(&graph, &input, &history).terminal,
                Some(TerminalProjection::Succeeded { .. })
            ));
            assert_pending_prefixes(
                &graph,
                &input,
                &history,
                &format!("nested={nested} queued={queued} rounds={rounds}"),
            );
        }
    }
}

#[tokio::test]
async fn descendant_intermediate_exits_preserve_winners_at_every_prefix() {
    for (nested, queued, rounds) in [
        (false, false, 3),
        (false, true, 3),
        (true, false, 3),
        (true, true, 3),
        (false, false, 2),
        (false, false, 4),
        (false, false, 5),
    ] {
        let (left, schema) = swap_loop_with_scope(rounds, queued, true);
        let inside = root_with_join(
            vec![left, verifier("b", 1)],
            json!({"kind":"first","when":either(opposite_verdicts())}),
            &schema,
        );
        let root = if nested {
            seq(
                "outer_run",
                vec![
                    json!({"kind":"map","name":"outer","state":schema,"body":inside,"over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]}),
                    succeed("outer_done"),
                ],
                &schema,
            )
        } else {
            inside
        };
        let graph = reducer_test_support::verified_graph(root, schema, true).await;
        let (input, history) = inputs_and_history(nested, queued, true);
        let completed = FullV1Reducer::native_v2(&graph)
            .with_dispatch_limit(1)
            .reduce_with_trace(ReductionInput {
                initial_input: &input,
                executions: &history,
                next_node_instance: 100,
                next_execution: 100,
            })
            .assert_value();
        assert!(matches!(
            completed.reduction.terminal,
            Some(TerminalProjection::Succeeded { .. })
        ));
        assert!(
            completed
                .trace
                .iter()
                .any(|trace| trace.node.as_str() == "race"
                    && trace.detail.as_deref() == Some("satisfied")),
            "complete history must actually satisfy the race"
        );
        assert_pending_prefixes(
            &graph,
            &input,
            &history,
            &format!("nested={nested} queued={queued} rounds={rounds}"),
        );
    }
}

fn assert_pending_prefixes(
    graph: &VerifiedGraph,
    input: &Value,
    history: &[DurableExecution],
    case: &str,
) {
    for count in 1..history.len() {
        let reduced = one_slot(graph, input, &history[..count]);
        assert!(
            reduced.terminal.is_none(),
            "{case} prefix={count}: {reduced:#?}"
        );
        let dispatches = reduced
            .decisions
            .iter()
            .filter_map(|decision| {
                if let Decision::Dispatch {
                    occurrence,
                    iteration,
                    ..
                } = decision
                {
                    Some((occurrence, iteration))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert!(
            !dispatches.is_empty(),
            "{case} prefix={count}: pending work must dispatch"
        );
        for (occurrence, iteration) in dispatches {
            assert!(
                history[count..]
                    .iter()
                    .any(|entry| entry.occurrence == *occurrence),
                "{case} prefix={count}: unexpected {occurrence:?}"
            );
            let depth = enclosing_loop_scope(&graph.compiled_ir.root, &occurrence.node, (0, None))
                .expect("authored occurrence");
            let expected = depth.map_or(0, |depth| {
                1 + history[..count]
                    .iter()
                    .filter(|entry| {
                        entry.occurrence.node.as_str() == "tail"
                            && entry.occurrence.map_indices == occurrence.map_indices[..depth]
                    })
                    .count() as u64
            });
            assert_eq!(
                *iteration, expected,
                "{case} prefix={count}: wrong loop round for {occurrence:?}"
            );
        }
    }
}

fn enclosing_loop_scope(
    node: &openengine_cluster_protocol::GraphNode,
    target: &openengine_cluster_protocol::NodeName,
    scope: (usize, Option<usize>),
) -> Option<Option<usize>> {
    if node.name() == target {
        return Some(scope.1);
    }
    let (mut depth, mut owner) = scope;
    if matches!(node, openengine_cluster_protocol::GraphNode::Map(_)) {
        depth += 1;
    }
    if matches!(node, openengine_cluster_protocol::GraphNode::Loop(_)) {
        owner = Some(depth);
    }
    openengine_cluster_server::graph_verifier::graph_node_children(node)
        .into_iter()
        .find_map(|child| enclosing_loop_scope(child, target, (depth, owner)))
}

fn append_round(history: &mut Vec<DurableExecution>, round: u64, scope: &[u64], descendant: bool) {
    let (items, other) = if round == 0 { (1, 2) } else { (2, 1) };
    for index in 0..items {
        let mut item = scope.to_vec();
        item.push(index);
        let label = if round == 1 && index == 0 {
            "accepted"
        } else {
            "rejected"
        };
        history.push(entry(history.len() as u64 + 1, "a", label, item));
    }
    let copies = if descendant {
        vec![("copy_x", other), ("copy_y", items)]
    } else {
        vec![("save", items), ("copy_x", other), ("copy_y", items)]
    };
    for (name, count) in copies {
        for index in 0..count {
            let mut item = scope.to_vec();
            item.push(index);
            history.push(done(history.len() as u64 + 1, name, item));
        }
    }
    history.push(done(history.len() as u64 + 1, "copy_ok", scope.to_vec()));
    history.push(entry(
        history.len() as u64 + 1,
        "exit",
        if round == 0 { "rejected" } else { "accepted" },
        scope.to_vec(),
    ));
    history.push(done(history.len() as u64 + 1, "tail", scope.to_vec()));
}
fn inputs_and_history(
    nested: bool,
    queued: bool,
    descendant: bool,
) -> (Value, Vec<DurableExecution>) {
    let scope = if nested { vec![0] } else { vec![] };
    (
        json!({"items":[1],"other":[1,1],"temp":[1]}),
        winning_history(&scope, queued, descendant),
    )
}
