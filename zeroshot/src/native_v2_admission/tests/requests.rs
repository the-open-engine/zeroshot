use super::*;
fn ask() -> Value {
    json!({"kind":"verifier","name":"approval","worker":crate::native_v2_request::ASK_WORKER_REF,
        "input":crate::native_v2_request::input_schema().assert_value(),"output":{"kind":"record","fields":{}},
        "signals":{"decision":["approved","rejected"]},"diagnostic":crate::native_v2_wait::diagnostic_schema().assert_value(),
        "inputBindings":[{"target":["text"],"value":{"source":"state","path":["text"]}}],"writeBindings":[],"attempts":1})
}
async fn check(node: Value, checkpoints: bool) -> Result<(), NativeV2AdmissionError> {
    check_nodes(vec![node, succeed("done")], checkpoints).await
}
async fn check_nodes(nodes: Vec<Value>, checkpoints: bool) -> Result<(), NativeV2AdmissionError> {
    let mut value = serde_json::to_value(graph(nodes)).assert_value();
    let text = json!({"required":true,"type":{"kind":"string"}});
    value["initialInput"]["fields"]["text"] = text.clone();
    value["root"]["state"]["fields"]["text"] = text;
    let graph: GraphSpec = serde_json::from_value(value).assert_value();
    let bindings = executable_declarations(&graph.root)
        .into_iter()
        .map(|node| (node.name, NodeRuntimeBinding::Request {}))
        .collect();
    let mut request = submission(graph.clone(), bindings);
    request.initial_input = json!({"items":[],"text":"Approve?"});
    request.runtime.workspace_checkpoints = checkpoints;
    let profile = NativeV2Admission
        .validate_profile(&graph, &request.runtime, DeliveryPolicy::Optional)
        .await;
    let admitted = NativeV2Admission.admit(request).await;
    assert_eq!(
        profile.is_ok(),
        admitted.is_ok(),
        "profile: {profile:?}; admission: {admitted:?}"
    );
    profile
}

#[tokio::test]
async fn multiple_requests_and_choice_placement_are_accepted() {
    let mut second = ask();
    second["name"] = json!("second");
    let state =
        json!({"kind":"record","fields":{"text":{"required":true,"type":{"kind":"string"}}}});
    let choice = json!({"kind":"choice","name":"choose","state":state,"branches":[{
        "when":{"kind":"in","value":{"name":"approval","source":"signal","field":"decision"},"labels":["approved"]},
        "node":second}],"otherwise":succeed("skip"),"promotedStatePaths":[]});
    check_nodes(vec![ask(), choice, succeed("done")], true)
        .await
        .assert_value();
}
#[tokio::test]
async fn request_contract_at_profile_and_run_admission() {
    let mut valid = ask();
    valid["timeoutMs"] = json!(crate::native_v2_request::MAX_DEADLINE_MILLIS);
    check(valid, true).await.assert_value();
    for (field, value) in [
        (
            "timeoutMs",
            json!(crate::native_v2_request::MAX_DEADLINE_MILLIS + 1),
        ),
        ("instructions", json!("ask")),
        ("attempts", json!(2)),
        ("input", json!({"kind":"null"})),
        (
            "input",
            json!({"kind":"record","fields":{"text":{"required":false,"type":{"kind":"string"}}}}),
        ),
        (
            "input",
            json!({"kind":"record","fields":{"text":{"required":true,"type":{"kind":"integer"}}}}),
        ),
        (
            "input",
            json!({"kind":"record","fields":{"text":{"required":true,"type":{"kind":"string"}},"extra":{"required":false,"type":{"kind":"string"}}}}),
        ),
        ("output", json!({"kind":"null"})),
        (
            "output",
            json!({"kind":"record","fields":{"nested":{"required":false,"type":{"kind":"record","fields":{}}}}}),
        ),
        (
            "output",
            json!({"kind":"record","fields":{"items":{"required":false,"type":{"kind":"array","items":{"kind":"string"}}}}}),
        ),
        ("kind", json!("step")),
    ] {
        let mut node = ask();
        node[field] = value;
        if field == "kind" {
            node.as_object_mut().assert_value().remove("signals");
            node.as_object_mut().assert_value().remove("diagnostic");
        }
        let error = check(node, true).await.unwrap_err();
        assert!(error.to_string().contains("approval"), "{error}");
    }
    assert!(
        check(ask(), false)
            .await
            .unwrap_err()
            .to_string()
            .contains("approval")
    );
    let graph = graph(vec![ask(), succeed("done")]);
    let runtime = submission(
        graph.clone(),
        BTreeMap::from([(named("approval"), binding("model", None))]),
    )
    .runtime;
    assert!(
        NativeV2Admission
            .validate_profile(&graph, &runtime, DeliveryPolicy::Optional)
            .await
            .unwrap_err()
            .to_string()
            .contains("approval")
    );
    let runtime = submission(
        graph.clone(),
        BTreeMap::from([(named("approval"), NodeRuntimeBinding::Request {})]),
    )
    .runtime;
    for delivery in [StackDelivery::Merge, StackDelivery::StackedPullRequests] {
        assert!(
            NativeV2Admission
                .validate_stack_profile(&graph, &runtime, delivery)
                .await
                .unwrap_err()
                .to_string()
                .contains("approval")
        );
    }
}
#[tokio::test]
async fn request_under_parallel_or_map_is_rejected_at_both_boundaries() {
    let inner = json!({"kind":"seq","name":"inner","state":{"kind":"null"},"children":[ask()],"promotedStatePaths":[]});
    for group in [
        json!({"kind":"par","name":"parallel","state":{"kind":"null"},"branches":[inner.clone()],"join":{"kind":"all"},"promotedStatePaths":[]}),
        json!({"kind":"map","name":"mapped","state":{"kind":"null"},"body":inner,"over":{"source":"state","path":["items"]},"maxItems":10,"promotedStatePaths":[]}),
    ] {
        let error = check(group, true).await.unwrap_err();
        assert!(error.to_string().contains("approval"), "{error}");
    }
}
