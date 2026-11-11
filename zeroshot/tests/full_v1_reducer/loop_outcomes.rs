use super::*;

use super::scoped::{done, entry, seq};

async fn conditional_carry(nested: bool, prefix: bool, grow_on_exit: bool) {
    let s = continuation::collection_state();
    let repeated = growing_loop(
        2,
        Some(signal("gate", "accepted", false)),
        if grow_on_exit { "accepted" } else { "rejected" },
    );
    let left = if prefix {
        seq("left", vec![step("prefix", 1), repeated], &s)
    } else {
        repeated
    };
    let g = growing_race(left, nested).await;
    let (input, scope) = carry_input(nested);
    let h = vec![entry(1, "b", "rejected", scope.clone())];
    let partial = one_slot(&g, &input, &h);
    if grow_on_exit {
        assert_no_satisfier(&partial);
    } else {
        assert!(partial.terminal.is_none());
    }

    for (succeeds, history) in carry_histories(&scope, prefix, grow_on_exit) {
        let reduced = one_slot(&g, &input, &history);
        if succeeds {
            assert!(
                matches!(reduced.terminal, Some(TerminalProjection::Succeeded { .. })),
                "{reduced:#?}"
            );
        } else {
            assert_no_satisfier(&reduced);
        }
    }
}

// Complete histories distinguish growth after the final map from growth feeding a later round.
fn carry_histories(
    scope: &[u64],
    prefix: bool,
    grow_on_exit: bool,
) -> Vec<(bool, Vec<DurableExecution>)> {
    let mut leaf = scope.to_vec();
    leaf.push(0);
    let mut second_item = scope.to_vec();
    second_item.push(1);
    let mut histories = vec![];
    for first in ["accepted", "rejected"] {
        for second in ["accepted", "rejected"] {
            let mut full = vec![entry(1, "b", "rejected", scope.to_vec())];
            let mut id = 2;
            if prefix {
                full.push(done(id, "prefix", scope.to_vec()));
                id += 1;
            }
            full.push(entry(id, "gate", first, scope.to_vec()));
            id += 1;
            full.push(entry(id, "a", "rejected", leaf.clone()));
            id += 1;
            let grows = |label| (label == "accepted") == grow_on_exit;
            if grows(first) {
                full.push(done(id, "copy", leaf.clone()));
                id += 1;
                full.push(done(id, "copy", second_item.clone()));
                id += 1;
            } else {
                full.push(done(id, "noop", scope.to_vec()));
                id += 1;
            }
            if first == "rejected" {
                full.push(entry(id, "gate", second, scope.to_vec()));
                id += 1;
                full.push(entry(id, "a", "accepted", leaf.clone()));
                id += 1;
                if grows(first) {
                    full.push(entry(id, "a", "rejected", second_item.clone()));
                    id += 1;
                }
                if grows(second) {
                    full.push(done(id, "copy", leaf.clone()));
                    id += 1;
                    full.push(done(id, "copy", second_item.clone()));
                } else {
                    full.push(done(id, "noop", scope.to_vec()));
                }
            }
            normalize_instances(&mut full);
            histories.push((!grow_on_exit && first == "rejected", full));
        }
    }
    histories
}

#[tokio::test]
async fn only_joint_nonconverging_writes_feed_a_later_round() {
    for nested in [false, true] {
        for prefix in [false, true] {
            for grow_on_exit in [false, true] {
                conditional_carry(nested, prefix, grow_on_exit).await;
            }
        }
    }
}

#[tokio::test]
async fn final_loop_controls_match_exit_predicates_before_item_aggregation() {
    for aggregate in [false, true] {
        for nested in [false, true] {
            for compatible in [false, true] {
                let s = state();
                let body = if aggregate {
                    seq(
                        "iteration",
                        vec![verifier("gate", 1), mapped(verifier("a", 1), 1)],
                        &s,
                    )
                } else {
                    verifier("a", 1)
                };
                let producer = if aggregate { "gate" } else { "a" };
                let until = signal(producer, "accepted", false);
                let repeated = json!({"kind":"loop","name":"twice","state":s,"maxIterations":2,"until":until,"body":body,"promotedStatePaths":[]});
                let control = json!({"kind":"in","value":{"name":"twice","source":"group","field":"terminated"},"labels":[if compatible {"converged"} else {"exhausted"}]});
                let mut guards = vec![signal(producer, "accepted", false), control];
                if aggregate {
                    guards.push(signal("a", "accepted", true));
                }
                let g =
                    race_in_scope(repeated, json!({"kind":"all","guards":guards}), s, nested).await;
                let input = json!({"items":[null]});
                let scope = if nested { vec![0] } else { vec![] };
                let mut full = vec![entry(1, "b", "rejected", scope.clone())];
                let partial = one_slot(&g, &input, &full);
                full.push(entry(2, producer, "accepted", scope.clone()));
                if aggregate {
                    let mut item = scope;
                    item.push(0);
                    full.push(entry(3, "a", "accepted", item));
                }
                let complete = one_slot(&g, &input, &full);
                if compatible {
                    assert!(partial.terminal.is_none());
                    assert!(matches!(
                        complete.terminal,
                        Some(TerminalProjection::Succeeded { .. })
                    ));
                } else {
                    assert_no_satisfier(&partial);
                    assert_no_satisfier(&complete);
                }
            }
        }
    }
}

#[tokio::test]
async fn separate_map_items_can_have_different_legal_loop_terminations() {
    let repeated = json!({"kind":"loop","name":"twice","state":state(),"maxIterations":2,"until":signal("a","accepted",false),"body":verifier("a",1),"promotedStatePaths":[]});
    let condition = json!({"kind":"all","guards":[signal("a","accepted",true),{"kind":"k_of_map","count":1,"value":{"name":"twice","source":"group","field":"terminated"},"labels":["exhausted"]}]});
    let g = checked_race(vec![mapped(repeated, 1), verifier("b", 1)], condition).await;
    let input = json!({"items":[null,null]});
    let mut history = vec![entry(1, "b", "rejected", vec![])];
    assert!(one_slot(&g, &input, &history).terminal.is_none());
    history.extend([
        entry(2, "a", "accepted", vec![0]),
        entry(3, "a", "rejected", vec![1]),
        entry(4, "a", "rejected", vec![1]),
    ]);
    history[3].node_instance = history[2].node_instance;
    assert!(matches!(
        one_slot(&g, &input, &history).terminal,
        Some(TerminalProjection::Succeeded { .. })
    ));
}

pub(super) fn normalize_instances(history: &mut [DurableExecution]) {
    let mut instances = std::collections::BTreeMap::new();
    for execution in history {
        execution.node_instance = *instances
            .entry(execution.occurrence.clone())
            .or_insert(execution.node_instance);
    }
}

#[tokio::test]
async fn observed_routes_feed_only_rounds_that_can_still_precede_the_final_map() {
    for nested in [false, true] {
        for (rounds, observed) in [(2, 1), (3, 1), (3, 2), (4, 2)] {
            observed_carry(nested, rounds, observed).await;
        }
    }
}

async fn observed_carry(nested: bool, rounds: u64, observed: u64) {
    let repeated = growing_loop(rounds, None, "accepted");
    let graph = growing_race(repeated, nested).await;
    let (input, scope) = carry_input(nested);
    let mut item = scope.clone();
    item.push(0);
    let mut other = scope.clone();
    other.push(1);
    let mut history = vec![entry(1, "b", "rejected", scope.clone())];
    assert!(one_slot(&graph, &input, &history).terminal.is_none());
    for round in 1..=observed {
        history.push(entry(
            history.len() as u64 + 1,
            "gate",
            "rejected",
            scope.clone(),
        ));
        if round < observed {
            finish_rejected_round(&mut history, &scope, &item);
        }
    }
    normalize_instances(&mut history);
    let partial = one_slot(&graph, &input, &history);
    let future_carry = rounds - observed > 1;
    if future_carry {
        assert!(partial.terminal.is_none());
    } else {
        assert_no_satisfier(&partial);
    }
    finish_rejected_round(&mut history, &scope, &item);
    if future_carry {
        history.push(entry(
            history.len() as u64 + 1,
            "gate",
            "accepted",
            scope.clone(),
        ));
        history.push(entry(
            history.len() as u64 + 1,
            "a",
            "rejected",
            item.clone(),
        ));
        history.push(done(history.len() as u64 + 1, "copy", item.clone()));
        history.push(done(history.len() as u64 + 1, "copy", other.clone()));
    }
    history.push(entry(
        history.len() as u64 + 1,
        "gate",
        "rejected",
        scope.clone(),
    ));
    history.push(entry(history.len() as u64 + 1, "a", "accepted", item));
    if future_carry {
        history.push(entry(history.len() as u64 + 1, "a", "rejected", other));
    }
    history.push(done(history.len() as u64 + 1, "noop", scope));
    normalize_instances(&mut history);
    let completed = one_slot(&graph, &input, &history);
    if future_carry {
        assert!(matches!(
            completed.terminal,
            Some(TerminalProjection::Succeeded { .. })
        ));
    } else {
        assert_no_satisfier(&completed);
    }
}

fn growing_loop(rounds: u64, until: Option<Value>, grow_label: &str) -> Value {
    let s = continuation::collection_state();
    let (grow, target) = continuation::collection_nodes();
    let choose = json!({"kind":"choice","name":"choose_growth","state":s,"branches":[{"when":signal("gate",grow_label,false),"node":grow}],"otherwise":step("noop",1),"promotedStatePaths":[["items"]]});
    let mut body = seq("body", vec![verifier("gate", 1), target, choose], &s);
    body["promotedStatePaths"] = json!([["items"]]);
    let mut repeated = json!({"kind":"loop","name":"twice","state":s,"maxIterations":rounds,"body":body,"promotedStatePaths":[]});
    if let Some(until) = until {
        repeated["until"] = until;
    }
    repeated
}

fn finish_rejected_round(history: &mut Vec<DurableExecution>, scope: &[u64], item: &[u64]) {
    history.push(entry(
        history.len() as u64 + 1,
        "a",
        "rejected",
        item.to_vec(),
    ));
    history.push(done(history.len() as u64 + 1, "noop", scope.to_vec()));
}

async fn growing_race(branch: Value, nested: bool) -> VerifiedGraph {
    race_in_scope(
        branch,
        continuation::opposite_verdicts(),
        continuation::collection_state(),
        nested,
    )
    .await
}

fn carry_input(nested: bool) -> (Value, Vec<u64>) {
    (
        json!({"items":[1],"next_items":[null,null]}),
        if nested { vec![0] } else { vec![] },
    )
}
