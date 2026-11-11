use super::*;
use super::scoped::{done, entry, seq};

fn repeated(children: Vec<Value>, until: Option<Value>) -> Value {
    let s = continuation::collection_state();
    let mut body = seq("body", children, &s);
    body["promotedStatePaths"] = json!([["items"]]);
    let mut repeated = json!({"kind":"loop","name":"twice","state":s,"maxIterations":2,"body":body,"promotedStatePaths":[]});
    if let Some(until) = until {
        repeated["until"] = until;
    }
    repeated
}

async fn graph(branch: Value, nested: bool) -> VerifiedGraph {
    race_in_scope(
        branch,
        continuation::opposite_verdicts(),
        continuation::collection_state(),
        nested,
    )
    .await
}

fn default_loop() -> Value {
    let (grow, target) = continuation::collection_nodes();
    repeated(vec![target, grow, step("tail", 1)], None)
}

pub(super) fn scopes(nested: bool) -> (Vec<u64>, Vec<u64>, Vec<u64>) {
    let scope = if nested { vec![0] } else { vec![] };
    let mut item = scope.clone();
    item.push(0);
    let mut other = scope.clone();
    other.push(1);
    (scope, item, other)
}

pub(super) fn assert_viability(result: &Reduction, possible: bool) {
    if possible {
        assert!(result.terminal.is_none(), "{result:#?}");
    } else {
        assert_no_satisfier(result);
    }
}

#[tokio::test]
async fn completed_collection_writes_keep_their_concrete_size_while_tails_wait() {
    for nested in [false, true] {
        for count in [1, 2] {
            let g = graph(default_loop(), nested).await;
            let input = json!({"items":[1],"next_items":vec![Value::Null;count]});
            let (scope, item, other) = scopes(nested);
            let mut history = vec![
                entry(1, "b", "rejected", scope.clone()),
                entry(2, "a", "rejected", item.clone()),
                done(3, "copy", item.clone()),
            ];
            if count == 2 {
                history.push(done(4, "copy", other.clone()));
            }
            assert_viability(&one_slot(&g, &input, &history), count == 2);
            history.push(done(history.len() as u64 + 1, "tail", scope.clone()));
            for label in ["accepted", "rejected"] {
                let mut full = history.clone();
                full.push(entry(full.len() as u64 + 1, "a", label, item.clone()));
                if count == 2 {
                    full.push(entry(full.len() as u64 + 1, "a", "rejected", other.clone()));
                }
                full.push(done(full.len() as u64 + 1, "copy", item.clone()));
                if count == 2 {
                    full.push(done(full.len() as u64 + 1, "copy", other.clone()));
                }
                full.push(done(full.len() as u64 + 1, "tail", scope.clone()));
                super::loop_outcomes::normalize_instances(&mut full);
                let result = one_slot(&g, &input, &full);
                if count == 2 && label == "accepted" {
                    assert!(matches!(
                        result.terminal,
                        Some(TerminalProjection::Succeeded { .. })
                    ));
                } else {
                    assert_no_satisfier(&result);
                }
            }
        }
    }
}

#[tokio::test]
async fn only_retained_writes_on_nonconverging_routes_feed_later_rounds() {
    for nested in [false, true] {
        for exits in [false, true] {
            let s = continuation::collection_state();
            let (grow, target) = continuation::collection_nodes();
            let mut writer = seq("writer", vec![grow, verifier("w", 1)], &s);
            writer["promotedStatePaths"] = json!([["items"]]);
            let when = json!({"kind":"any","guards":[{"kind":"all","guards":[signal("gate","accepted",false),signal("w","accepted",false)]},signal("c","accepted",false)]});
            let select = json!({"kind":"par","name":"select_write","state":s,"branches":[writer,verifier("c",1)],"join":{"kind":"first","when":when},"promotedStatePaths":[["items"]]});
            let until = signal("gate", if exits { "accepted" } else { "rejected" }, false);
            let g = graph(
                repeated(
                    vec![verifier("gate", 1), target, select, step("tail", 1)],
                    Some(until),
                ),
                nested,
            )
            .await;
            let input = json!({"items":[1],"next_items":[null,null]});
            let (scope, item, other) = scopes(nested);
            let mut history = vec![entry(1, "b", "rejected", scope.clone())];
            assert_viability(&one_slot(&g, &input, &history), !exits);
            history.extend([
                entry(2, "gate", "accepted", scope.clone()),
                entry(3, "a", "rejected", item.clone()),
                done(4, "copy", item.clone()),
                done(5, "copy", other.clone()),
                entry(6, "w", "accepted", scope.clone()),
                done(7, "tail", scope.clone()),
            ]);
            if !exits {
                history.extend([
                    entry(8, "gate", "rejected", scope.clone()),
                    entry(9, "a", "accepted", item),
                    entry(10, "a", "rejected", other),
                    entry(11, "c", "accepted", scope.clone()),
                    done(12, "tail", scope),
                ]);
            }
            super::loop_outcomes::normalize_instances(&mut history);
            let result = one_slot(&g, &input, &history);
            if exits {
                assert_no_satisfier(&result);
            } else {
                assert!(matches!(
                    result.terminal,
                    Some(TerminalProjection::Succeeded { .. })
                ));
            }
        }
    }
}

#[tokio::test]
async fn failed_items_cannot_supply_a_partial_collection_for_loop_carry() {
    for nested in [false, true] {
        for failed in [false, true] {
            let g = graph(default_loop(), nested).await;
            let input = json!({"items":[1],"next_items":[null,null]});
            let (scope, item, other) = scopes(nested);
            let mut first = done(3, "copy", item.clone());
            if failed {
                if let DurableExecutionState::Settled { outcome, .. } = &mut first.state {
                    *outcome = WorkerOutcome::declared_failure(
                        openengine_cluster_protocol::WorkerErrorCode::Crash,
                    );
                }
            }
            let mut history = vec![
                entry(1, "b", "rejected", scope.clone()),
                entry(2, "a", "rejected", item.clone()),
                first,
            ];
            assert_viability(&one_slot(&g, &input, &history), !failed);
            history.extend([
                done(4, "copy", other.clone()),
                done(5, "tail", scope.clone()),
                entry(6, "a", "accepted", item.clone()),
            ]);
            if !failed {
                history.push(entry(7, "a", "rejected", other.clone()));
            }
            history.push(done(history.len() as u64 + 1, "copy", item));
            history.push(done(history.len() as u64 + 1, "copy", other));
            history.push(done(history.len() as u64 + 1, "tail", scope));
            super::loop_outcomes::normalize_instances(&mut history);
            let result = one_slot(&g, &input, &history);
            if failed {
                assert_no_satisfier(&result);
            } else {
                assert!(matches!(
                    result.terminal,
                    Some(TerminalProjection::Succeeded { .. })
                ));
            }
        }
    }
}

#[tokio::test]
async fn hidden_completed_growth_does_not_change_the_next_round_collection() {
    let mut branch = default_loop();
    branch["body"]["promotedStatePaths"] = json!([]);
    let g = graph(branch, false).await;
    let history = vec![
        entry(1, "b", "rejected", vec![]),
        entry(2, "a", "rejected", vec![0]),
        done(3, "copy", vec![0]),
        done(4, "copy", vec![1]),
    ];
    assert_no_satisfier(&one_slot(
        &g,
        &json!({"items":[1],"next_items":[null,null]}),
        &history,
    ));
}

#[tokio::test]
async fn nested_pending_groups_preserve_concrete_promoted_collection_sizes() {
    for nested in [false, true] {
        for choice in [false, true] {
            for count in [1, 2] {
                let s = continuation::collection_state();
                let (grow, target) = continuation::collection_nodes();
                let mut pending = seq("pending", vec![grow, step("tail", 1)], &s);
                pending["promotedStatePaths"] = json!([["items"]]);
                if choice {
                    pending = json!({"kind":"choice","name":"chosen","state":s,"branches":[{"when":signal("a","rejected",true),"node":pending}],"otherwise":step("noop",1),"promotedStatePaths":[["items"]]});
                }
                let g = graph(repeated(vec![target, pending], None), nested).await;
                let (scope, item, other) = scopes(nested);
                let mut history = vec![
                    entry(1, "b", "rejected", scope),
                    entry(2, "a", "rejected", item.clone()),
                    done(3, "copy", item),
                ];
                if count == 2 {
                    history.push(done(4, "copy", other));
                }
                assert_viability(
                    &one_slot(
                        &g,
                        &json!({"items":[1],"next_items":vec![Value::Null;count]}),
                        &history,
                    ),
                    count == 2,
                );
            }
        }
    }
}

fn collection_completion(scope: &[u64], shape: (u64, usize, usize)) -> Vec<DurableExecution> {
    let (rounds, count, accepted) = shape;
    let mut history = vec![entry(1, "b", "rejected", scope.to_vec())];
    let mut items = 1;
    for round in 1..=rounds {
        for index in 0..items {
            let mut item = scope.to_vec();
            item.push(index as u64);
            let label = if round == rounds && index < accepted {
                "accepted"
            } else {
                "rejected"
            };
            history.push(entry(history.len() as u64 + 1, "a", label, item));
        }
        for index in 0..count {
            let mut item = scope.to_vec();
            item.push(index as u64);
            history.push(done(history.len() as u64 + 1, "copy", item));
        }
        history.push(done(history.len() as u64 + 1, "tail", scope.to_vec()));
        items = count;
    }
    super::loop_outcomes::normalize_instances(&mut history);
    history
}

fn pending_collection_check(g: &VerifiedGraph, scope: &[u64], expected: (usize, bool)) -> Value {
    let (count, possible) = expected;
    let mut item = scope.to_vec();
    item.push(0);
    let history = vec![
        entry(1, "b", "rejected", scope.to_vec()),
        entry(2, "a", "rejected", item),
    ];
    let input = json!({"items":[1],"next_items":vec![Value::Null;count]});
    assert_viability(&one_slot(g, &input, &history), possible);
    input
}

#[tokio::test]
async fn pending_collections_preserve_exact_sizes_across_remaining_rounds() {
    for nested in [false, true] {
        for rounds in [2, 3, 4] {
            for count in [0, 1, 2, 3] {
                let mut branch = default_loop();
                branch["maxIterations"] = json!(rounds);
                let g = graph(branch, nested).await;
                let (scope, _, _) = scopes(nested);
                let input = pending_collection_check(&g, &scope, (count, count >= 2));
                let complete = one_slot(
                    &g,
                    &input,
                    &collection_completion(&scope, (rounds, count, 1)),
                );
                if count >= 2 {
                    assert!(matches!(
                        complete.terminal,
                        Some(TerminalProjection::Succeeded { .. })
                    ));
                } else {
                    assert_no_satisfier(&complete);
                }
            }
        }
    }
}

#[tokio::test]
async fn finite_replacements_cannot_invent_larger_collections() {
    for nested in [false, true] {
        for count in [2, 3] {
            let mut accepted = signal("a", "accepted", true);
            accepted["count"] = json!(2);
            let guard = json!({"kind":"all","guards":[accepted,signal("a","rejected",true)]});
            let g = race_in_scope(
                default_loop(),
                guard,
                continuation::collection_state(),
                nested,
            )
            .await;
            let (scope, _, _) = scopes(nested);
            let input = pending_collection_check(&g, &scope, (count, count == 3));
            let full = one_slot(&g, &input, &collection_completion(&scope, (2, count, 2)));
            if count == 3 {
                assert!(matches!(
                    full.terminal,
                    Some(TerminalProjection::Succeeded { .. })
                ));
            } else {
                assert_no_satisfier(&full);
            }
        }
    }
}

#[tokio::test]
async fn convergence_can_keep_the_current_collection_before_a_future_replacement() {
    for nested in [false, true] {
        let (grow, target) = continuation::collection_nodes();
        let branch = repeated(
            vec![target, grow, verifier("gate", 1), step("tail", 1)],
            Some(signal("gate", "accepted", false)),
        );
        let mut two = signal("a", "accepted", true);
        two["count"] = json!(2);
        let errors = json!({"kind":"k_of_map","count":1,"value":{"name":"a","source":"error","field":null},"labels":["crash","timeout","malformed","refusal"]});
        let guard = json!({"kind":"all","guards":[signal("a","accepted",true),{"kind":"not","guard":two},{"kind":"not","guard":signal("a","rejected",true)},{"kind":"not","guard":errors}]});
        let g = race_in_scope(branch, guard, continuation::collection_state(), nested).await;
        let (scope, item, other) = scopes(nested);
        let input = json!({"items":[1],"next_items":[null,null]});
        let mut history = vec![
            entry(1, "b", "rejected", scope.clone()),
            entry(2, "a", "accepted", item.clone()),
            done(3, "copy", item),
            done(4, "copy", other),
        ];
        let pending = one_slot(&g, &input, &history);
        history.extend([
            entry(5, "gate", "accepted", scope.clone()),
            done(6, "tail", scope),
        ]);
        assert!(matches!(
            one_slot(&g, &input, &history).terminal,
            Some(TerminalProjection::Succeeded { .. })
        ));
        assert!(
            pending.terminal.is_none(),
            "the current round can still converge: {pending:#?}"
        );
    }
}

#[tokio::test]
async fn self_copies_keep_finite_sizes_in_every_remaining_round() {
    for nested in [false, true] {
        for rounds in [2, 3, 4, 8] {
            let mut branch = default_loop();
            branch["maxIterations"] = json!(rounds);
            branch["body"]["children"][1]["over"]["path"] = json!(["items"]);
            let g = graph(branch, nested).await;
            let (scope, _, _) = scopes(nested);
            for count in [1, 2] {
                let input = json!({"items":vec![1;count],"next_items":[null]});
                let history = vec![entry(1, "b", "rejected", scope.clone())];
                assert_viability(&one_slot(&g, &input, &history), count == 2);
            }
        }
    }
}

fn copy_chain(hops: usize, rounds: usize) -> (Value, Value, Value) {
    let names = (0..=hops).map(|i| format!("items_{i}")).collect::<Vec<_>>();
    let mut schema = continuation::collection_state();
    let mut input = json!({"items":[1],"next_items":[null]});
    for (i, name) in names.iter().enumerate() {
        schema["fields"][name] =
            json!({"type":{"kind":"array","items":{"kind":"integer"}},"required":true});
        input[name] = json!(vec![1; if i == hops { 2 } else { 1 }]);
    }
    let (_, mut target) = continuation::collection_nodes();
    target["state"] = schema.clone();
    target["over"]["path"] = json!([names[0]]);
    let mut children = vec![target];
    for i in 0..hops {
        let name = format!("copy_{i}");
        let mut copy = step(&name, 1);
        copy["writeBindings"] =
            json!([{"value":{"node":name,"channel":"out","path":["value"]},"target":[names[i]]}]);
        children.push(json!({"kind":"map","name":format!("grow_{i}"),"state":schema,"body":copy,"over":{"source":"state","path":[names[i+1]]},"maxItems":1,"promotedStatePaths":[[names[i]]]}));
    }
    children.push(step("tail", 1));
    let mut body = seq("body", children, &schema);
    body["promotedStatePaths"] = json!(
        names[..hops]
            .iter()
            .map(|name| vec![name])
            .collect::<Vec<_>>()
    );
    let repeated = json!({"kind":"loop","name":"repeat","state":schema,"body":body,"maxIterations":rounds,"promotedStatePaths":[]});
    (repeated, schema, input)
}

#[tokio::test]
async fn ordered_copy_chains_require_enough_nonfinal_rounds_to_reach_the_selector() {
    for (nested, queued) in [(false, false), (false, true), (true, false), (true, true)] {
        for hops in [2, 3, 4] {
            for rounds in [hops, hops + 1, hops + 2] {
                let (mut branch, schema, input) = copy_chain(hops, rounds);
                if queued {
                    branch = seq("prefix", vec![step("wait", 1), branch], &schema);
                }
                let g =
                    race_in_scope(branch, continuation::opposite_verdicts(), schema, nested).await;
                let (scope, _, _) = scopes(nested);
                let history = vec![entry(1, "b", "rejected", scope)];
                assert_viability(&one_slot(&g, &input, &history), rounds > hops);
            }
        }
    }
}

#[tokio::test]
async fn queued_forced_convergence_keeps_the_first_round_winner() {
    for nested in [false, true] {
        for growing in [false, true] {
            let s = continuation::collection_state();
            let (grow, target) = continuation::collection_nodes();
            let mut children = vec![verifier("exit", 1), target];
            if growing {
                children.push(grow);
            }
            children.push(step("tail", 1));
            let until = json!({"kind":"any","guards":[signal("exit","accepted",false),signal("exit","rejected",false),{"kind":"in","value":{"name":"exit","source":"error","field":null},"labels":["crash","timeout","malformed","refusal"]}]});
            let mut branch = repeated(children, Some(until));
            branch["maxIterations"] = json!(3);
            let left = seq("left", vec![step("gate", 1), branch], &s);
            let g = race_in_scope(left, signal("a", "accepted", true), s, nested).await;
            let (scope, item, _) = scopes(nested);
            let input = json!({"items":[1],"next_items":[null]});
            let mut history = vec![entry(1, "b", "rejected", scope.clone())];
            assert_viability(&one_slot(&g, &input, &history), true);
            history.extend([
                done(2, "gate", scope.clone()),
                entry(3, "exit", "accepted", scope.clone()),
                entry(4, "a", "accepted", item.clone()),
            ]);
            if growing {
                history.push(done(5, "copy", item));
            }
            history.push(done(history.len() as u64 + 1, "tail", scope));
            assert!(matches!(
                one_slot(&g, &input, &history).terminal,
                Some(TerminalProjection::Succeeded { .. })
            ));
        }
    }
}
