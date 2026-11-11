use super::*;
use openengine_cluster_protocol::FailureContext;

fn rounds(name: &str, count: u64, state: &Value) -> Value {
    let check = format!("{name}_assessor");
    json!({"kind":"loop","name":name,"state":state,"promotedStatePaths":[],
    "maxIterations":count,"body":verifier(&check, 1),"until":{
        "kind":"in","value":{"name":check,"source":"signal","field":"verdict"},"labels":["accepted"]
    }})
}

fn failed_branch(name: &str, body: Value, state: &Value) -> Value {
    json!({"kind":"seq","name":format!("{name}_branch"),"state":state,"promotedStatePaths":[],"children":[
        body,
        {"kind":"choice","name":format!("{name}_route"),"state":state,"promotedStatePaths":[],
         "branches":[{"when":{"kind":"in","value":{"name":format!("{name}_assessor"),"source":"signal","field":"verdict"},"labels":["rejected"]},
                      "node":{"kind":"fail","name":format!("{name}_halt"),"reason":"unfinished"}}],
         "otherwise":step(&format!("{name}_continue"),1)}
    ]})
}

fn append_rounds(history: &mut Vec<DurableExecution>, name: &str, indices: Vec<u64>, count: u64) {
    let check = format!("{name}_assessor");
    let instance = history.len() as u64 + 1;
    for _ in 0..count {
        let id = history.len() as u64 + 1;
        history.push(settled(
            SettledSpec::new(id, instance, &check)
                .map_indices(indices.clone())
                .position(id * 2),
            verdict("rejected"),
        ));
    }
}

fn evidence(
    graph: &VerifiedGraph,
    initial: &Value,
    history: &[DurableExecution],
) -> FailureContext {
    let input = ReductionInput {
        initial_input: initial,
        executions: history,
        next_execution: history.len() as u64 + 1,
        next_node_instance: history.len() as u64 + 1,
    };
    let reducer = FullV1Reducer::native_v2(graph);
    let plain = reducer.reduce(input.clone()).assert_value();
    let traced = reducer.reduce_with_trace(input).assert_value();
    assert_eq!(plain, traced.reduction);
    assert!(
        plain
            .decisions
            .iter()
            .all(|decision| !matches!(decision, Decision::Dispatch { .. }))
    );
    let TerminalProjection::Failed { context, .. } = plain.terminal.assert_value() else {
        panic!("expected authored failure")
    };
    context.map(|context| *context).unwrap_or_default()
}

fn stopped_parallel(branches: Vec<Value>, state: &Value) -> Value {
    json!({"kind":"seq","name":"root","state":state,"promotedStatePaths":[],"children":[
        {"kind":"par","name":"outer","state":state,"promotedStatePaths":[],"join":{"kind":"all"},"branches":branches},
        {"kind":"fail","name":"halt","reason":"needs_followup"}
    ]})
}

#[tokio::test]
async fn canonical_loop_evidence_excludes_settled_inner_race_losers() {
    let state = json!({"kind":"record","fields":{}});
    let mut winner = failed_branch("winner", rounds("winner", 1, &state), &state);
    winner["children"][1]["branches"][0]["when"]["labels"] = json!(["accepted"]);
    let inner = json!({"kind":"par","name":"race","state":state,"promotedStatePaths":[],"join":{"kind":"any"},"branches":[
        failed_branch("loser", rounds("loser", 2, &state), &state),
        winner
    ]});
    let graph = super::trace::observed_graph(stopped_parallel(
        vec![
            inner,
            failed_branch("other", rounds("other", 2, &state), &state),
        ],
        &state,
    ))
    .await;
    let mut history = Vec::new();
    append_rounds(&mut history, "loser", vec![], 2);
    append_rounds(&mut history, "winner", vec![], 1);
    history.push(settled(
        SettledSpec::new(4, 4, "winner_continue").position(8),
        success(1),
    ));
    append_rounds(&mut history, "other", vec![], 2);
    let context = evidence(&graph, &json!({}), &history);
    let mut names = context
        .exhausted_loops
        .iter()
        .map(|entry| (entry.node.as_str(), entry.completed_iterations.get()))
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, [("other", 2), ("winner", 1)]);
    assert!(!context.incomplete);
}

#[tokio::test]
async fn canonical_loop_evidence_excludes_map_losers_and_their_overflow() {
    let state = json!({"kind":"record","fields":{"items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}});
    let mapped = json!({"kind":"map","name":"mapped","state":state,"promotedStatePaths":[],
        "over":{"source":"state","path":["items"]},"maxItems":6,
        "body":failed_branch("mapped_rounds", rounds("mapped_rounds", 2, &state), &state)});
    let graph = super::trace::observed_graph(stopped_parallel(
        vec![
            mapped,
            failed_branch("other", rounds("other", 2, &state), &state),
        ],
        &state,
    ))
    .await;
    let mut history = Vec::new();
    for round in 0..2 {
        for index in 0..6 {
            let id = history.len() as u64 + 1;
            history.push(settled(
                SettledSpec::new(id, index + 1, "mapped_rounds_assessor")
                    .map_indices(vec![index])
                    .position(round * 12 + index + 7)
                    .dispatched(round * 12 + index + 1),
                verdict("rejected"),
            ));
        }
    }
    history.push(settled(
        SettledSpec::new(13, 7, "other_assessor")
            .position(9)
            .dispatched(0),
        verdict("rejected"),
    ));
    history.push(settled(
        SettledSpec::new(14, 7, "other_assessor")
            .position(25)
            .dispatched(10),
        verdict("rejected"),
    ));
    history.sort_by_key(|execution| execution.dispatch_position);
    for (index, execution) in history.iter_mut().enumerate() {
        execution.execution = ExecutionId::new(index as u64 + 1).unwrap();
    }
    let context = evidence(
        &graph,
        &json!({"items":[null,null,null,null,null,null]}),
        &history,
    );
    assert_eq!(context.exhausted_loops.len(), 1);
    let selected = context
        .exhausted_loops
        .iter()
        .find(|entry| entry.node.as_str() == "mapped_rounds")
        .assert_value();
    assert_eq!(selected.map_indices, [0]);
    assert_eq!(selected.completed_iterations.get(), 2);
    assert!(
        !context.incomplete,
        "discarded probes cannot consume the public evidence bound"
    );
}
