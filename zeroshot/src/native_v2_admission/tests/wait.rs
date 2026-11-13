use super::*;

fn wait() -> serde_json::Value {
    json!({"kind":"verifier","name":"wait","worker":crate::native_v2_wait::WAIT_WORKER_REF,
        "input":crate::native_v2_wait::input_schema().assert_value(),"output":{"kind":"null"},
        "signals":crate::native_v2_wait::signal_labels().assert_value(),
        "diagnostic":crate::native_v2_wait::diagnostic_schema().assert_value(),
        "inputBindings":[],"writeBindings":[],"attempts":1})
}

#[tokio::test]
async fn wait_requires_exact_host_contract_and_binding() {
    let bindings = BTreeMap::from([(named("wait"), NodeRuntimeBinding::Request {})]);
    let valid = wait();
    let valid_graph = graph(vec![valid, succeed("done")]);
    NativeV2Admission
        .validate_profile(
            &valid_graph,
            &submission(valid_graph.clone(), bindings.clone()).runtime,
            DeliveryPolicy::Optional,
        )
        .await
        .assert_value();
    for (field, value) in [
        ("instructions", json!("wait")),
        ("timeoutMs", json!(1000)),
        ("attempts", json!(2)),
        ("output", json!({"kind":"string"})),
        ("worker", json!("agent.other@1")),
        ("kind", json!("step")),
    ] {
        let mut node = wait();
        node[field] = value;
        if field == "kind" {
            node.as_object_mut().assert_value().remove("signals");
            node.as_object_mut().assert_value().remove("diagnostic");
        }

        let graph = graph(vec![node, succeed("done")]);
        let request = submission(graph.clone(), bindings.clone());
        let error = NativeV2Admission
            .validate_profile(&graph, &request.runtime, DeliveryPolicy::Optional)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("wait"), "{error}");
    }
    let request = submission(
        valid_graph.clone(),
        BTreeMap::from([(named("wait"), binding("caller-model", None))]),
    );
    assert!(
        NativeV2Admission
            .validate_profile(&valid_graph, &request.runtime, DeliveryPolicy::Optional)
            .await
            .is_err()
    );
    for delivery in [StackDelivery::Merge, StackDelivery::StackedPullRequests] {
        let runtime = submission(valid_graph.clone(), bindings.clone()).runtime;
        let error = NativeV2Admission
            .validate_stack_profile(&valid_graph, &runtime, delivery)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("wait"));
    }
}

#[tokio::test]
async fn wait_under_parallel_or_map_is_rejected_even_below_a_sequence() {
    let bindings = BTreeMap::from([(named("wait"), NodeRuntimeBinding::Request {})]);
    let inner = json!({"kind":"seq","name":"inner","state":{"kind":"null"},"children":[wait()],"promotedStatePaths":[]});
    for group in [
        json!({"kind":"par","name":"parallel","state":{"kind":"null"},"branches":[inner.clone()],"join":{"kind":"all"},"promotedStatePaths":[]}),
        json!({"kind":"map","name":"mapped","state":{"kind":"null"},"body":inner,
            "over":{"source":"state","path":["items"]},"maxItems":10,"promotedStatePaths":[]}),
    ] {
        let graph = graph(vec![group, succeed("done")]);
        let runtime = submission(graph.clone(), bindings.clone()).runtime;
        let error = NativeV2Admission
            .validate_profile(&graph, &runtime, DeliveryPolicy::Optional)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("wait"), "{error}");
    }
}
