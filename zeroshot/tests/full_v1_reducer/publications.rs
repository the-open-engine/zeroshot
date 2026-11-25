use super::*;

#[tokio::test]
async fn only_the_selected_outer_success_exposes_literal_and_typed_bound_paths() {
    let state =
        json!({"kind":"record","fields":{"file":{"type":{"kind":"filepath"},"required":true}}});
    let mut internal = succeed("internal");
    internal["artifacts"] = json!({"private":{"kind":"literal","path":"missing-internal"}});
    let profile = json!({"kind":"profile","name":"call","profile":"saved","initialInput":{"kind":"null"},"body":internal});
    let mut selected = succeed("selected");
    selected["artifacts"] = json!({"tree":{"kind":"literal","path":"report"},"file":{"kind":"binding","value":{"source":"state","path":["file"]}}});
    let mut unselected = succeed("unselected");
    unselected["artifacts"] = json!({"unselected":{"kind":"literal","path":"missing-unselected"}});
    let choice = json!({"kind":"choice","name":"choose","state":state,"promotedStatePaths":[],"branches":[{
        "when":{"kind":"in","value":{"name":"gate","source":"signal","field":"verdict"},"labels":["accepted"]},"node":selected}],"otherwise":unselected});
    let mut root = sequence("root", vec![profile, verifier("gate", 1), choice]);
    root["state"] = state;
    let graph = verified(root, json!({})).await;
    let history = [settled(
        SettledSpec::new(17, 4, "gate").position(2),
        verdict("accepted"),
    )];
    let input = json!({"file":"report/current"});
    for reducer in [FullV1Reducer::new(&graph), FullV1Reducer::native_v2(&graph)] {
        let request = || ReductionInput {
            initial_input: &input,
            executions: &history,
            next_node_instance: 5,
            next_execution: 18,
        };
        let plain = reducer.reduce(request()).assert_value();
        let traced = reducer
            .reduce_with_trace(request())
            .assert_value()
            .reduction;
        assert_eq!(plain, traced);
        assert_eq!(
            plain.artifacts,
            [("tree", "report"), ("file", "report/current")]
                .map(|(field, path)| (field.parse().assert_value(), path.to_owned()))
                .into_iter()
                .collect()
        );
        assert!(matches!(
            plain.terminal,
            Some(TerminalProjection::Succeeded {
                output: Value::Null
            })
        ));
    }
}
