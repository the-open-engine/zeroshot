use super::*;
use zeroshot_engine::full_v1_reducer::{StructuralTrace, StructuralTraceState};

pub(super) async fn observed_graph(root: Value) -> VerifiedGraph {
    let graph: GraphSpec = serde_json::from_value(json!({"profile":"openengine.graph.full/v1","initialInput":root["state"],"policy":{"policy":"policy.test@1","default":"deny"},"root":root})).assert_value();
    let result = ProductionGraphVerifier::new(TestWorkers { rich_outputs: true })
        .verify(&graph)
        .await;
    assert!(result.is_ok(), "trace fixture must be admitted: {result:?}");
    result.assert_value()
}

fn trace(
    graph: &VerifiedGraph,
    initial: &Value,
    history: &[DurableExecution],
) -> Vec<StructuralTrace> {
    let input = ReductionInput {
        initial_input: initial,
        executions: history,
        next_node_instance: history
            .iter()
            .map(|entry| entry.node_instance.get())
            .max()
            .unwrap_or(0)
            + 1,
        next_execution: history
            .iter()
            .map(|entry| entry.execution.get())
            .max()
            .unwrap_or(0)
            + 1,
    };
    let reducer = FullV1Reducer::native_v2(graph);
    let plain = reducer.reduce(input.clone()).assert_value();
    let observed = reducer.reduce_with_trace(input).assert_value();
    assert_eq!(
        plain.canonical_decision_bytes().assert_value(),
        observed.reduction.canonical_decision_bytes().assert_value()
    );
    assert_eq!(plain, observed.reduction);
    observed.trace
}

fn accepted(name: &str) -> Value {
    json!({"kind":"in","value":{"name":name,"source":"signal","field":"verdict"},"labels":["accepted"]})
}

fn route(name: &str, condition: Value, yes: Value, no: Value) -> Value {
    json!({"kind":"choice","name":name,"state":{"kind":"record","fields":{}},"branches":[{"when":condition,"node":yes}],"otherwise":no,"promotedStatePaths":[]})
}

#[tokio::test]
async fn accepted_parallel_reviews_expose_terminal_only_choice_at_the_completed_prefix() {
    let graph = observed_graph(sequence("root", vec![
        json!({"kind":"par","name":"reviews","state":{"kind":"record","fields":{}},"branches":[verifier("acceptance",1),verifier("code",1)],"promotedStatePaths":[],"join":{"kind":"all"}}),
        route("review_result", json!({"kind":"all","guards":[accepted("acceptance"),accepted("code")]}), succeed("done"), step("repair",1)),
        succeed("repaired"),
    ])).await;
    let mut history = vec![
        active(1, 1, "acceptance", 1),
        settled(
            SettledSpec::new(2, 2, "code").position(4),
            verdict("accepted"),
        ),
    ];
    let before = trace(&graph, &json!({}), &history);
    assert!(!before.iter().any(|record| record.node.as_str() == "review_result" || record.node.as_str() == "done"));
    history[0].state = DurableExecutionState::Settled {
        position: HistoryPosition::new(5).assert_value(),
        outcome: verdict("accepted"),
    };
    let after = trace(&graph, &json!({}), &history);
    let choice = after
        .iter()
        .find(|record| record.node.as_str() == "review_result")
        .assert_value();
    assert_eq!(
        choice.branch.as_ref().map(|name| name.as_str()),
        Some("done")
    );
    assert_eq!(choice.state, StructuralTraceState::Completed);
    assert!(after.iter().any(|record| record.node.as_str() == "done"
        && record.state == StructuralTraceState::Succeeded
        && record.output == Some(Value::Null)));
    assert!(!after.iter().any(|record| record.node.as_str() == "repair"));
}

#[tokio::test]
async fn loop_paths_distinguish_structural_visits_and_retries_stay_in_the_same_visit() {
    let graph = verified(sequence("root", vec![
        json!({"kind":"loop","name":"rounds","state":{"kind":"record","fields":{}},"maxIterations":3,"promotedStatePaths":[],"body":sequence("iteration", vec![verifier("check",2),route("result",accepted("check"),succeed("done"),step("repair",1))])}),
        succeed("exhausted"),
    ]), json!({})).await;
    let crash = settled(
        SettledSpec::new(1, 1, "check").position(2),
        WorkerOutcome::declared_failure(openengine_cluster_protocol::WorkerErrorCode::Crash),
    );
    let retry = settled(
        SettledSpec::new(2, 1, "check").attempt(2).position(4),
        verdict("rejected"),
    );
    let repair = settled(SettledSpec::new(3, 2, "repair").position(6), success(1));
    let accepted = settled(
        SettledSpec::new(4, 1, "check").position(8),
        verdict("accepted"),
    );
    let before = trace(&graph, &json!({}), std::slice::from_ref(&crash));
    assert_eq!(
        before
            .iter()
            .filter(|record| record.node.as_str() == "iteration")
            .count(),
        1
    );
    let observed = trace(&graph, &json!({}), &[crash, retry, repair, accepted]);
    let choices = observed
        .iter()
        .filter(|record| record.node.as_str() == "result")
        .collect::<Vec<_>>();
    assert_eq!(choices.len(), 2);
    assert_eq!(choices[0].loop_iterations, vec![1]);
    assert_eq!(choices[1].loop_iterations, vec![2]);
    assert_eq!(
        choices[0].branch.as_ref().map(|name| name.as_str()),
        Some("repair")
    );
    assert_eq!(
        choices[1].branch.as_ref().map(|name| name.as_str()),
        Some("done")
    );
}

#[tokio::test]
async fn race_probes_do_not_leak_a_losing_terminal_path_or_duplicate_winners() {
    let graph = verified(sequence("root", vec![
        json!({"kind":"par","name":"race","state":{"kind":"record","fields":{}},"branches":[sequence("left_branch",vec![step("left",1),succeed("left_end")]),sequence("right_branch",vec![step("right",1)])],"promotedStatePaths":[],"join":{"kind":"any"}}),
        succeed("done"),
    ]), json!({})).await;
    for loser_started in [false, true] {
        let mut history = vec![settled(
            SettledSpec::new(2, 2, "right").position(4),
            success(1),
        )];
        if loser_started {
            history.insert(0, active(1, 1, "left", 1));
        }
        let observed = trace(&graph, &json!({}), &history);
        assert!(
            !observed.iter().any(|record| record.node.as_str() == "left"
                && record.state == StructuralTraceState::Queued)
        );
        assert!(
            !observed
                .iter()
                .any(|record| ["left_branch", "left_end"].contains(&record.node.as_str()))
        );
        assert_eq!(
            observed
                .iter()
                .filter(|record| record.node.as_str() == "right_branch")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn terminal_parallel_branches_remain_visible_when_the_group_continues() {
    let graph = observed_graph(sequence("root", vec![
        verifier("check",1),
        json!({"kind":"par","name":"parallel","state":{"kind":"record","fields":{}},"branches":[route("left_route",accepted("check"),succeed("left_end"),step("left",1)),route("right_route",accepted("check"),json!({"kind":"fail","name":"right_end","reason":"declined"}),step("right",1))],"promotedStatePaths":[],"join":{"kind":"all"}}),
        succeed("done"),
    ])).await;
    let observed = trace(
        &graph,
        &json!({}),
        &[settled(
            SettledSpec::new(1, 1, "check").position(2),
            verdict("accepted"),
        )],
    );
    assert_eq!(
        observed
            .iter()
            .filter(|record| record.node.as_str() == "left_end"
                && record.state == StructuralTraceState::Succeeded)
            .count(),
        1
    );
    assert_eq!(
        observed
            .iter()
            .filter(|record| record.node.as_str() == "right_end"
                && record.state == StructuralTraceState::Failed)
            .count(),
        1
    );
    assert!(
        observed
            .iter()
            .any(|record| record.node.as_str() == "parallel"
                && record.detail.as_deref() == Some("quorum_unreachable"))
    );
}

#[tokio::test]
async fn map_terminal_selection_traces_only_the_winning_item_scope() {
    let state = json!({"kind":"record","fields":{"items":{"required":true,"type":{"kind":"array","items":{"kind":"null"}}}}});
    let body = sequence("item_body", vec![step("work", 1), succeed("item_done")]);
    let mapped = json!({"kind":"map","name":"mapped","state":state.clone(),"over":{"source":"state","path":["items"]},"maxItems":2,"body":body,"promotedStatePaths":[]});
    let mut root = sequence("root", vec![mapped, succeed("empty_done")]);
    root["state"] = state;
    let graph = verified(root, json!({})).await;
    let first = settled(
        SettledSpec::new(1, 1, "work")
            .map_indices(vec![0])
            .position(4),
        success(1),
    );
    let mut second = active(2, 2, "work", 2);
    second.occurrence.map_indices = vec![1];
    let observed = trace(&graph, &json!({"items":[null,null]}), &[first, second]);
    let terminal = observed
        .iter()
        .find(|record| record.node.as_str() == "item_done")
        .assert_value();
    assert_eq!(terminal.map_indices, vec![0]);
    assert!(!observed.iter().any(|record| record.map_indices == vec![1]));
}

#[tokio::test]
async fn queue_observation_is_bounded_independently_of_dispatch_and_keeps_item_loop_identity() {
    let state = required_array_record(&[("items", "null")]);
    let graph = reducer_test_support::verified_graph(json!({"kind":"seq","name":"run","state":state,"promotedStatePaths":[],"children":[
        {"kind":"map","name":"mapped","state":state,"over":{"source":"state","path":["items"]},"maxItems":3,"promotedStatePaths":[],
            "body":{"kind":"loop","name":"rounds","state":state,"maxIterations":2,"body":step("work",1),"promotedStatePaths":[]}},succeed("done")
    ]}), state,true).await;
    let observed = FullV1Reducer::native_v2(&graph)
        .with_dispatch_limit(0)
        .reduce_with_trace(ReductionInput {
            initial_input: &json!({"items":[null,null,null]}),
            executions: &[],
            next_execution: 1,
            next_node_instance: 1,
        })
        .unwrap();
    let queued = observed
        .trace
        .iter()
        .filter(|record| record.state == StructuralTraceState::Queued)
        .collect::<Vec<_>>();
    assert_eq!(queued.len(), 3);
    for (index, record) in queued.iter().enumerate() {
        assert_eq!(record.map_indices, vec![index as u64]);
        assert_eq!(record.loop_iterations, vec![1]);
    }
    assert!(
        !observed
            .reduction
            .decisions
            .iter()
            .any(|decision| matches!(decision, Decision::Dispatch { .. }))
    );
}

#[tokio::test]
async fn nested_completed_joins_reuse_discovery_and_replay_each_branch_once() {
    for join in [
        json!({"kind":"all"}),
        json!({"kind":"any"}),
        json!({"kind":"quorum","count":2}),
    ] {
        let depth = 24_u64;
        let mut nested = step("leaf_0", 1);
        for level in 1..=depth {
            nested = json!({"kind":"par","name":format!("nested_{level}"),
                "state":{"kind":"record","fields":{}},
                "branches":[nested,step(&format!("leaf_{level}"),1)],
                "join":join,"promotedStatePaths":[]});
        }
        let graph = observed_graph(sequence("root", vec![nested, succeed("done")])).await;
        let history = (0..=depth)
            .map(|index| {
                settled(
                    SettledSpec::new(index + 1, index + 1, &format!("leaf_{index}"))
                        .dispatched(index + 1)
                        .position(depth + index + 2),
                    success(1),
                )
            })
            .collect::<Vec<_>>();
        let observed = trace(&graph, &json!({}), &history);
        assert_eq!(observed.len(), depth as usize + 2);
        assert!(observed.iter().any(|record| record.node.as_str() == "done"
            && record.state == StructuralTraceState::Succeeded));
    }
}
