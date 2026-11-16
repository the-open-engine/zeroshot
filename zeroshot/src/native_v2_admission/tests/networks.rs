use super::*;

fn network_graph() -> GraphSpec {
    let mut graph = graph(vec![null_step("work", "app.work@1"), succeed("done")]);
    graph.networks = serde_json::from_value(json!({"team":{"members":["work"]}})).assert_value();
    graph
}

#[tokio::test]
async fn network_admission_preserves_agent_members_and_default_deny() {
    let graph = network_graph();
    let expected = graph.networks.clone();
    let request = submission(
        graph,
        BTreeMap::from([(named("work"), binding("claude-sonnet-5", None))]),
    );
    NativeV2Admission
        .validate_profile(&request.graph, &request.runtime, DeliveryPolicy::Optional)
        .await
        .assert_value();
    let admitted = NativeV2Admission.admit(request).await.assert_value();
    assert_eq!(admitted.graph.networks, expected);
    assert!(
        admitted
            .graph
            .networks
            .values()
            .next()
            .assert_value()
            .publishers
            .is_empty()
    );
}

#[tokio::test]
async fn network_admission_rejects_nonagent_members_before_execution() {
    let mut graph = network_graph();
    if let GraphNode::Seq(root) = &mut graph.root {
        let GraphNode::Step(step) = &mut root.children.as_mut_slice()[0] else {
            panic!("step fixture")
        };
        step.instructions = None;
    }
    let script =
        serde_json::from_value(json!({"kind":"script","command":["python3"]})).assert_value();
    let request = submission(graph, BTreeMap::from([(named("work"), script)]));
    for result in [
        NativeV2Admission
            .validate_profile(&request.graph, &request.runtime, DeliveryPolicy::Optional)
            .await,
        NativeV2Admission
            .validate_intent(&submission_intent(&request), DeliveryPolicy::Optional)
            .await,
        NativeV2Admission.admit(request).await.map(|_| ()),
    ] {
        assert!(
            matches!(result, Err(NativeV2AdmissionError::InvalidRuntimeBinding { node, detail })
            if node == named("work") && detail.contains("require an agent binding"))
        );
    }
}
