use super::*;
use super::scoped::{done, entry, seq};

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
    super::loop_outcomes::normalize_instances(&mut history);
    history
}

#[tokio::test]
async fn intermediate_convergence_preserves_real_winners_at_every_prefix() {
    for (nested, queued) in [(false, false), (true, false), (false, true), (true, true)] {
        for rounds in [2, 3, 4, 5] {
            let (left, schema) = swap_loop(rounds, queued);
            let graph =
                race_in_scope(left, continuation::opposite_verdicts(), schema, nested).await;
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
async fn intermediate_exits_require_their_own_convergence_and_count_witness() {
    for (nested, queued) in [(false, false), (false, true)] {
        for incompatible in ["exhausted", "exit_rejected", "three_items"] {
            let (left, schema) = swap_loop(3, queued);
            let mut guard = continuation::opposite_verdicts();
            match incompatible {
                "exhausted" => guard["guards"].as_array_mut().unwrap().push(json!({"kind":"in","value":{"name":"repeat","source":"group","field":"terminated"},"labels":["exhausted"]})),
                "exit_rejected" => guard["guards"].as_array_mut().unwrap().push(signal("exit","rejected",false)),
                _ => guard["guards"][0]["count"] = json!(2),
            }
            eprintln!(
                "admission control nested={nested} queued={queued} incompatible={incompatible}"
            );
            let graph = race_in_scope(left, guard, schema, nested).await;
            let scope = if nested { vec![0] } else { vec![] };
            let result = one_slot(
                &graph,
                &json!({"items":[1],"other":[1,1],"temp":[1]}),
                &[entry(1, "b", "rejected", scope)],
            );
            assert!(
                result.terminal.is_some(),
                "nested={nested} queued={queued} incompatible={incompatible}: {result:#?}"
            );
            assert_no_satisfier(&result);
        }
    }
}

#[tokio::test]
async fn current_round_exit_keeps_observed_rejections_fixed() {
    for nested in [false, true] {
        let (left, schema) = swap_loop(2, false);
        let graph = race_in_scope(left, continuation::opposite_verdicts(), schema, nested).await;
        let scope = if nested { vec![0] } else { vec![] };
        let mut first = scope.clone();
        first.push(0);
        let mut second = scope.clone();
        second.push(1);
        let history = vec![
            entry(1, "b", "rejected", scope),
            entry(2, "a", "rejected", first),
            entry(3, "a", "rejected", second),
        ];
        assert_no_satisfier(&one_slot(
            &graph,
            &json!({"items":[1,1],"other":[1],"temp":[1]}),
            &history,
        ));
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
            json!({"kind":"first","when":either(continuation::opposite_verdicts())}),
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
    }
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
