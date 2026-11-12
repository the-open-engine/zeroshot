use super::*;

fn fixed_loop(name: &str, count: u64, body: Value) -> Value {
    json!({"kind":"loop","name":name,"state":{"kind":"record","fields":{}},
        "body":body,"maxIterations":count,"promotedStatePaths":[]})
}

fn replay(graph: &VerifiedGraph, input: &Value, history: &[DurableExecution]) -> Reduction {
    // Round-trip the unchanged durable history, and evaluate with a fresh reducer on every visit.
    let restored: Vec<DurableExecution> =
        serde_json::from_value(serde_json::to_value(history).assert_value()).assert_value();
    FullV1Reducer::native_v2(graph)
        .reduce(ReductionInput {
            initial_input: input,
            executions: &restored,
            next_node_instance: history
                .iter()
                .map(|e| e.node_instance.get())
                .max()
                .unwrap_or(0)
                + 1,
            next_execution: history.len() as u64 + 1,
        })
        .assert_value()
}

#[tokio::test]
async fn dispatch_metadata_tracks_leaf_loops_retries_resets_and_exit_during_replay() {
    let graph = verified(
        sequence(
            "root",
            vec![
                step("before", 1),
                fixed_loop(
                    "outer",
                    3,
                    sequence(
                        "outer_body",
                        vec![
                            step("outer_work", 1),
                            fixed_loop("inner", 2, step("inner_work", 2)),
                            step("outer_tail", 1),
                        ],
                    ),
                ),
                step("after", 1),
                succeed("done"),
            ],
        ),
        json!({}),
    )
    .await;
    let mut history = Vec::new();
    let mut observed = Vec::new();
    let mut instances = std::collections::BTreeMap::new();
    let mut retried = false;
    for _ in 0..32 {
        let reduction = replay(&graph, &json!({}), &history);
        if reduction.terminal.is_some() {
            break;
        }
        let decision = reduction
            .decisions
            .into_iter()
            .find(|d| matches!(d, Decision::Dispatch { .. }))
            .assert_value();
        let Decision::Dispatch {
            execution,
            node_instance,
            occurrence,
            attempt,
            iteration,
            input,
            ..
        } = decision
        else {
            unreachable!()
        };
        let node = occurrence.node.as_str().to_owned();
        if let Some(previous) = instances.insert(node.clone(), node_instance) {
            assert_eq!(previous, node_instance);
        }
        observed.push((node.clone(), attempt.get(), iteration));
        let outcome = if node == "inner_work" && !retried {
            retried = true;
            WorkerOutcome::declared_failure(openengine_cluster_protocol::WorkerErrorCode::Crash)
        } else {
            success(1)
        };
        history.push(DurableExecution {
            delivery_state: None,
            dispatch_position: HistoryPosition::new(execution.get() * 2).assert_value(),
            execution,
            node_instance,
            occurrence,
            attempt,
            input,
            state: DurableExecutionState::Settled {
                position: HistoryPosition::new(execution.get() * 2 + 1).assert_value(),
                outcome,
            },
        });
    }
    assert!(replay(&graph, &json!({}), &history).terminal.is_some());
    let mut expected = vec![("before".to_owned(), 1, 0)];
    for outer in 1..=3 {
        expected.push(("outer_work".to_owned(), 1, outer));
        expected.push(("inner_work".to_owned(), 1, 1));
        if outer == 1 {
            expected.push(("inner_work".to_owned(), 2, 1));
        }
        expected.push(("inner_work".to_owned(), 1, 2));
        expected.push(("outer_tail".to_owned(), 1, outer));
    }
    expected.push(("after".to_owned(), 1, 0));
    assert_eq!(observed, expected);
}

#[tokio::test]
async fn dispatch_metadata_uses_inner_loop_inside_map_and_parallel_boundaries() {
    let state = required_array_record(&[("items", "null")]);
    let mapped = json!({"kind":"map","name":"mapped","state":state.clone(),
        "over":{"source":"state","path":["items"]},"maxItems":2,"promotedStatePaths":[],
        "body": fixed_loop("inner", 2, step("mapped_work", 1))});
    let parallel = json!({"kind":"par","name":"parallel","state":state.clone(),
        "branches":[mapped,step("peer",1)],"join":{"kind":"all"},"promotedStatePaths":[]});
    let mut root = sequence("root", vec![parallel, step("after", 1), succeed("done")]);
    root["state"] = state;
    let graph = verified(root, json!({})).await;
    let mut history = Vec::new();
    let mut seen = std::collections::BTreeMap::<Vec<u64>, Vec<(u64, u64)>>::new();
    for _ in 0..8 {
        let reduction = replay(&graph, &json!({"items":[null,null]}), &history);
        if reduction.terminal.is_some() {
            break;
        }
        // The checkpoint boundary is outside both loops, so it cannot supply the leaf iteration.
        assert!(
            reduction
                .boundary
                .as_ref()
                .assert_value()
                .loop_iterations
                .is_empty()
        );
        for decision in reduction.decisions {
            let Decision::Dispatch {
                execution,
                node_instance,
                occurrence,
                attempt,
                iteration,
                input,
                ..
            } = decision
            else {
                continue;
            };
            if occurrence.node.as_str() == "mapped_work" {
                seen.entry(occurrence.map_indices.clone())
                    .or_default()
                    .push((node_instance.get(), iteration));
                assert_eq!(attempt.get(), 1);
            } else {
                assert_eq!(iteration, 0);
            }
            history.push(DurableExecution {
                delivery_state: None,
                dispatch_position: HistoryPosition::new(execution.get() * 2).assert_value(),
                execution,
                node_instance,
                occurrence,
                attempt,
                input,
                state: DurableExecutionState::Settled {
                    position: HistoryPosition::new(execution.get() * 2 + 1).assert_value(),
                    outcome: success(1),
                },
            });
        }
    }
    assert!(
        replay(&graph, &json!({"items":[null,null]}), &history)
            .terminal
            .is_some()
    );
    assert_eq!(seen.len(), 2);
    for values in seen.values() {
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].0, values[1].0);
        assert_eq!((values[0].1, values[1].1), (1, 2));
    }
    assert_ne!(seen[&vec![0]][0].0, seen[&vec![1]][0].0);
}
