use super::*;
use crate::native_v2_delivery::contract::{
    delivery_diagnostic_schema, delivery_result_schema, delivery_signal_labels,
};
use crate::native_v2_delivery::{DeliveryMode, DELIVERY_SIGNAL_FIELD};
use openengine_cluster_protocol::RuntimePlan;
use openengine_cluster_testkit::assertions::AssertValue;

#[test]
fn delivery_choices_use_native_contracts_and_preserve_legacy_identities() {
    let choices = workers().assert_value();
    for (index, mode) in [
        (1, DeliveryMode::Push),
        (2, DeliveryMode::PullRequestV2),
        (3, DeliveryMode::MergeV3),
    ] {
        let choice = &choices[index];
        assert_eq!(choice["node"]["kind"], "verifier");
        assert!(choice["node"]["instructions"].is_null());
        assert_eq!(
            choice["node"]["output"],
            json!(delivery_result_schema(mode).assert_value())
        );
        assert_eq!(
            choice["node"]["diagnostic"],
            json!(delivery_diagnostic_schema().assert_value())
        );
        assert_eq!(
            choice["node"]["signals"][DELIVERY_SIGNAL_FIELD],
            json!(delivery_signal_labels(mode).assert_value())
        );
        let binding = if mode == DeliveryMode::Push {
            json!({"kind":"git_delivery"})
        } else {
            json!({"kind":"git_delivery","connections":{"github":["GH_TOKEN"]}})
        };
        assert_eq!(choice["runtimeBinding"], binding);
        assert_eq!(
            choice["node"]["input"]["fields"]["issueNumber"]["required"],
            false
        );
    }
    assert_eq!(
        choices[3]["node"]["worker"],
        GIT_DELIVERY_MERGE_V3_WORKER_REF
    );
    assert!(
        choices[3]["workerRefs"]
            .as_array()
            .assert_value()
            .contains(&json!(GIT_DELIVERY_MERGE_WORKER_REF))
    );
    assert!(
        choices[0]["workerRefs"]
            .as_array()
            .assert_value()
            .is_empty()
    );
}

#[tokio::test]
async fn template_variants_keep_exact_factory_graphs_and_admit_with_factory_bindings() {
    let templates = templates().assert_value();
    assert_eq!(templates.len(), 7);
    for template in templates {
        let kind =
            BuiltinGraphTemplate::parse(template["name"].as_str().assert_value()).assert_value();
        let delivery = match template["delivery"].as_str().assert_value() {
            "pull_request" => TemplateDelivery::PullRequest,
            "push" => TemplateDelivery::Push,
            "merge" => TemplateDelivery::Merge,
            _ => TemplateDelivery::None,
        };
        assert_eq!(
            template["graph"],
            json!(kind.materialize(delivery).assert_value())
        );
        let graph: GraphSpec = serde_json::from_value(template["graph"].clone()).assert_value();
        let mut bindings = template["runtimeBindings"]
            .as_object()
            .assert_value()
            .clone();
        let mut pending = vec![&graph.root];
        while let Some(node) = pending.pop() {
            if matches!(node, GraphNode::Step(_) | GraphNode::Verifier(_)) {
                let binding = bindings
                    .entry(node.name().to_string())
                    .or_insert_with(|| json!({"kind":"agent","model":""}));
                if binding["kind"] == "agent" {
                    binding["model"] = json!("opaque-model");
                }
            }
            pending.extend(graph_node_children(node));
        }
        let runtime: RuntimePlan = serde_json::from_value(
            json!({"harness":"codex","provider":"openai","size":"small","nodes":bindings}),
        )
        .assert_value();
        super::super::validate_profile(&graph, &runtime)
            .await
            .assert_value();
    }
}

#[test]
fn software_templates_keep_one_worker_session_and_fresh_reviewers() {
    for template in templates()
        .assert_value()
        .iter()
        .filter(|value| value["name"] == "software-change")
    {
        let bindings = &template["runtimeBindings"];
        assert_eq!(bindings["worker"]["sessionScope"], "node_instance");
        assert!(bindings["acceptance"].is_null());
        assert!(bindings["code"].is_null());
    }
}

#[test]
fn research_editor_bindings_keep_four_sessions_and_a_fresh_reviewer() {
    let templates = templates().assert_value();
    for mode in ["none", "push"] {
        let template = templates
            .iter()
            .find(|t| t["name"] == "auto-research" && t["delivery"] == mode)
            .assert_value();
        let bindings = &template["runtimeBindings"];
        for role in ["explorer", "synthesizer", "challenger", "researcher"] {
            assert_eq!(bindings[role]["sessionScope"], "node_instance");
        }
        assert!(
            bindings.get("reviewer").is_none(),
            "reviewer uses the protocol's execution default"
        );
        assert_eq!(
            bindings.get("checkpoint_delivery").is_some(),
            mode == "push"
        );
    }
}
