use super::*;

async fn nested_scope_graph() -> VerifiedGraph {
    let state = json!({"kind":"record","fields":{"items":{"required":true,"type":{
        "kind":"array","items":{"kind":"record","fields":{"kids":{"required":true,
            "type":{"kind":"array","items":{"kind":"null"}}}}}
    }}}});
    let route = json!({"kind":"choice","name":"route","state":state,"promotedStatePaths":[],
        "branches":[{"when":{"kind":"in","value":{"source":"signal","name":"a","field":"verdict"},
            "labels":["accepted"]},"node":verifier("b",1)}],"otherwise":verifier("c",1)});
    let outer = json!({"kind":"map","name":"outer","state":state,"promotedStatePaths":[],
        "over":{"source":"state","path":["items"]},"maxItems":2,
        "body":{"kind":"seq","name":"body","state":state,"promotedStatePaths":[],
            "children":[verifier("a",1),
                {"kind":"map","name":"inner","state":state,"promotedStatePaths":[],
                    "over":{"source":"item","path":["kids"]},"maxItems":2,"body":route}]}});
    let after = json!({"kind":"choice","name":"after","state":state,"promotedStatePaths":[],
        "branches":[{"when":{"kind":"all","guards":[
            {"kind":"k_of_map","count":1,"value":{"source":"signal","name":"b","field":"verdict"},"labels":["accepted"]},
            {"kind":"k_of_map","count":1,"value":{"source":"signal","name":"c","field":"verdict"},"labels":["accepted"]}
        ]},"node":succeed("yes")}],"otherwise":{"kind":"fail","name":"no","reason":"no_matching_signals"}});
    let mut root = sequence("root", vec![outer, after]);
    root["state"] = state;
    let source: GraphSpec = serde_json::from_value(json!({
        "profile":"openengine.graph.full/v1","initialInput":root["state"],
        "policy":{"policy":"policy.test@1","default":"deny"},"root":root
    }))
    .assert_value();
    let graph = ProductionGraphVerifier::new(TestWorkers { rich_outputs: true })
        .verify(&source)
        .await
        .unwrap_or_else(|error| panic!("graph verification failed: {error:?}"));
    assert_eq!(graph.compiled_ir.root, source.root);
    graph
}

fn nested_history(items: &[(&str, &[&str])]) -> (Value, Vec<DurableExecution>) {
    let mut history = Vec::new();
    let mut inputs = Vec::new();
    for (outer, (gate, children)) in items.iter().enumerate() {
        inputs.push(json!({"kids":vec![Value::Null; children.len()]}));
        let id = history.len() as u64 + 1;
        history.push(settled(
            SettledSpec::new(id, id, "a")
                .map_indices(vec![outer as u64])
                .position(id * 2),
            verdict(gate),
        ));
        for (inner, label) in children.iter().enumerate() {
            let id = history.len() as u64 + 1;
            history.push(settled(
                SettledSpec::new(id, id, if *gate == "accepted" { "b" } else { "c" })
                    .map_indices(vec![outer as u64, inner as u64])
                    .position(id * 2),
                verdict(label),
            ));
        }
    }
    (json!({"items":inputs}), history)
}

fn assert_nested_replay(graph: &VerifiedGraph, items: &[(&str, &[&str])], succeeds: bool) {
    let (initial, history) = nested_history(items);
    let expected = if succeeds {
        TerminalProjection::Succeeded {
            output: Value::Null,
        }
    } else {
        TerminalProjection::Failed {
            context: None,
            reason: "no_matching_signals".parse().assert_value(),
        }
    };
    for reducer in [FullV1Reducer::new(graph), FullV1Reducer::native_v2(graph)] {
        // Replay a second ledger storage order without changing dispatch/settlement chronology.
        for history in [history.clone(), history.iter().rev().cloned().collect()] {
            let input = ReductionInput {
                initial_input: &initial,
                executions: &history,
                next_execution: history.len() as u64 + 1,
                next_node_instance: history.len() as u64 + 1,
            };
            let plain = reducer.reduce(input.clone()).assert_value();
            let traced = reducer.reduce_with_trace(input).assert_value();
            assert_eq!(plain, traced.reduction);
            assert_eq!(plain.terminal, Some(expected.clone()), "{items:?}");
            assert!(!plain.decisions.iter().any(|decision| matches!(
                decision,
                Decision::Dispatch { .. } | Decision::VoidLoser { .. }
            )));
        }
    }
}

#[tokio::test]
async fn nested_map_implicit_dependencies_replay_mixed_outer_items() {
    let graph = nested_scope_graph().await;
    for gates in [["accepted", "rejected"], ["rejected", "accepted"]] {
        assert_nested_replay(
            &graph,
            &[(gates[0], &["accepted"]), (gates[1], &["accepted"])],
            true,
        );
    }
    assert_nested_replay(
        &graph,
        &[
            ("accepted", &["rejected", "accepted"]),
            ("rejected", &["accepted", "rejected"]),
        ],
        true,
    );
    assert_nested_replay(
        &graph,
        &[
            ("accepted", &["accepted", "accepted"]),
            ("rejected", &["rejected", "rejected"]),
        ],
        false,
    );
}

#[tokio::test]
async fn nested_map_implicit_dependencies_replay_uniform_and_empty_items() {
    let graph = nested_scope_graph().await;
    for gate in ["accepted", "rejected"] {
        assert_nested_replay(
            &graph,
            &[(gate, &["accepted"]), (gate, &["accepted"])],
            false,
        );
    }
    assert_nested_replay(&graph, &[], false);
    assert_nested_replay(
        &graph,
        &[("accepted", &[]), ("rejected", &["accepted"])],
        false,
    );
    assert_nested_replay(&graph, &[("accepted", &[]), ("rejected", &[])], false);
}
