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
        // The selected instance supplies delivery connections at submission.
        assert_eq!(choice["runtimeBinding"], json!({"kind":"git_delivery"}));
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
    assert_eq!(templates.len(), 14);
    for template in templates {
        let kind =
            BuiltinGraphTemplate::parse(template["name"].as_str().assert_value()).assert_value();
        let delivery = match template["delivery"].as_str().assert_value() {
            "pull_request" => TemplateDelivery::PullRequest,
            "push" => TemplateDelivery::Push,
            "merge" => TemplateDelivery::Merge,
            "comment" => TemplateDelivery::Comment,
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
fn first_template_choices_use_their_default_delivery_bindings() {
    let choices = templates().assert_value();
    for (template, kind) in [
        (BuiltinGraphTemplate::IssueReview, "comment_delivery"),
        (BuiltinGraphTemplate::PrReview, "comment_delivery"),
        (BuiltinGraphTemplate::CodeReview, "comment_delivery"),
        (BuiltinGraphTemplate::SoftwareChange, "git_delivery"),
    ] {
        let choice = choices
            .iter()
            .find(|choice| choice["name"] == template.name())
            .assert_value();
        assert_eq!(choice["delivery"], template.default_delivery().name());
        assert_eq!(choice["runtimeBindings"]["deliver"]["kind"], kind);
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

#[tokio::test]
async fn decision_worker_uses_the_native_contract_and_keeps_the_model_caller_owned() {
    let choice = workers()
        .unwrap()
        .into_iter()
        .find(|value| value["runtimeKind"] == "decision")
        .unwrap();
    assert_eq!(choice["runtimeBinding"]["model"], "");
    let mut node = choice["node"].clone();
    node["name"] = json!("triage");
    let graph = crate::native_v2_candidate::test_support::full_graph(vec![
        node,
        crate::native_v2_candidate::test_support::success_node(),
    ]);
    let mut binding = choice["runtimeBinding"].clone();
    binding["model"] = json!("caller/model");
    let runtime = serde_json::from_value(
        json!({"harness":"codex","provider":"openai","size":"small","nodes":{"triage":binding}}),
    )
    .unwrap();
    super::super::validate_profile(&graph, &runtime)
        .await
        .unwrap();
}

#[tokio::test]
async fn complete_template_defaults_admit_and_expose_derived_access() {
    for template in templates().assert_value() {
        let graph: GraphSpec = serde_json::from_value(template["graph"].clone()).assert_value();
        let runtime: RuntimePlan =
            serde_json::from_value(template["runtime"].clone()).assert_value();
        assert!(runtime.workspace_checkpoints);
        assert!(!template["description"].as_str().assert_value().is_empty());
        super::super::validate_profile(&graph, &runtime)
            .await
            .assert_value();
        let setup = super::super::profile_setup(json!({"runtime":runtime})).assert_value();
        assert_eq!(template["requirements"], setup["requirements"]);
    }
}

#[test]
fn code_review_preserves_the_supplied_agent_plan_and_native_delivery() {
    let template = templates()
        .assert_value()
        .into_iter()
        .find(|template| template["name"] == "code-review")
        .assert_value();
    let supplied: RuntimePlan = serde_json::from_str(include_str!(
        "../../../../docs/assets/code-review/runtime.json"
    ))
    .assert_value();
    let mut supplied = serde_json::to_value(supplied).assert_value();
    supplied["nodes"]["deliver"] = json!({"kind":"comment_delivery"});
    assert_eq!(template["runtime"]["nodes"], supplied["nodes"]);
    assert_eq!(template["runtime"]["harness"], supplied["harness"]);
    assert_eq!(template["runtime"]["provider"], supplied["provider"]);
    assert_eq!(
        template["requirements"]["openai"],
        json!(["OPENAI_API_KEY"])
    );
    assert_eq!(
        template["requirements"]["anthropic"],
        json!(["ANTHROPIC_API_KEY"])
    );
    assert!(template["requirements"].get("github").is_none());
    assert!(template["runtime"].get("size").is_none());
    assert!(template["runtime"].get("workspaceCheckpoints").is_none());
}

#[test]
fn new_builtin_examples_have_native_descriptions_and_generic_requirements() {
    let templates = templates().assert_value();
    for name in ["documentation-task", "cross-provider-task", "service-task"] {
        assert!(templates.iter().any(|template| template["name"] == name));
    }
    let service = templates
        .iter()
        .find(|template| template["name"] == "service-task")
        .assert_value();
    assert_eq!(
        service["requirements"]["project_service"],
        json!(["PROJECT_SERVICE_API_KEY", "PROJECT_SERVICE_ENDPOINT"])
    );
    let mixed = templates
        .iter()
        .find(|template| template["name"] == "cross-provider-task")
        .assert_value();
    assert!(mixed["requirements"].get("anthropic").is_some());
    assert!(mixed["requirements"].get("openai").is_some());
}
