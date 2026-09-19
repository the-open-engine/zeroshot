//! Editor metadata for implementations supported by the native host. Contracts and delivery
//! bindings are materialized by the existing template factories, not authored by the browser.
use openengine_cluster_protocol::{GraphNode, GraphSpec};
use openengine_cluster_server::graph_verifier::graph_node_children;
use serde_json::{json, Value};

use crate::native_v2_contract::{
    GIT_DELIVERY_MERGE_V2_WORKER_REF, GIT_DELIVERY_MERGE_V3_WORKER_REF,
    GIT_DELIVERY_MERGE_WORKER_REF, GIT_DELIVERY_PR_V2_WORKER_REF, GIT_DELIVERY_PR_WORKER_REF,
    GIT_DELIVERY_PUSH_WORKER_REF,
};

use super::{ApiError, BuiltinGraphTemplate, TemplateDelivery};

pub(super) fn templates() -> Result<Vec<Value>, ApiError> {
    let mut result = Vec::new();
    for template in BuiltinGraphTemplate::all() {
        let deliveries = if *template == BuiltinGraphTemplate::SoftwareChange {
            &[
                TemplateDelivery::None,
                TemplateDelivery::Push,
                TemplateDelivery::PullRequest,
                TemplateDelivery::Merge,
            ][..]
        } else {
            &[TemplateDelivery::None][..]
        };
        for delivery in deliveries {
            let graph = materialize(*template, *delivery)?;
            let mut bindings = serde_json::Map::new();
            if let Some((name, binding)) = template
                .delivery_runtime_binding(*delivery)
                .map_err(internal)?
            {
                bindings.insert(
                    name.to_string(),
                    serde_json::to_value(binding).map_err(internal)?,
                );
            }
            result.push(json!({
                "id":format!("{}:{}", template.name(), delivery.name()),
                "name":template.name(),
                "delivery":delivery.name(),
                "label":template_label(*template, *delivery),
                "graph":graph,
                "runtimeBindings":bindings,
            }));
        }
    }
    Ok(result)
}

pub(super) fn workers() -> Result<Vec<Value>, ApiError> {
    let mut result = vec![json!({
        "id":"agent",
        "label":"Agent",
        "runtimeKind":"agent",
        "workerRefs":[],
        "runtimeBinding":{"kind":"agent","model":""},
    })];
    for (id, label, delivery, refs) in [
        (
            "git_delivery_push",
            "Git delivery · push",
            TemplateDelivery::Push,
            vec![GIT_DELIVERY_PUSH_WORKER_REF],
        ),
        (
            "git_delivery_pr",
            "Git delivery · pull request",
            TemplateDelivery::PullRequest,
            vec![GIT_DELIVERY_PR_V2_WORKER_REF, GIT_DELIVERY_PR_WORKER_REF],
        ),
        (
            "git_delivery_merge",
            "Git delivery · merge",
            TemplateDelivery::Merge,
            vec![
                GIT_DELIVERY_MERGE_V3_WORKER_REF,
                GIT_DELIVERY_MERGE_V2_WORKER_REF,
                GIT_DELIVERY_MERGE_WORKER_REF,
            ],
        ),
    ] {
        let template = BuiltinGraphTemplate::SoftwareChange;
        let graph = materialize(template, delivery)?;
        let (name, binding) = template
            .delivery_runtime_binding(delivery)
            .map_err(internal)?
            .ok_or_else(|| ApiError::internal("Missing delivery binding.".into()))?;
        result.push(json!({
            "id":id,
            "label":label,
            "runtimeKind":"git_delivery",
            "workerRefs":refs,
            "node":named_node(&graph, name.as_str())?,
            "runtimeBinding":binding,
        }));
    }
    Ok(result)
}

fn materialize(
    template: BuiltinGraphTemplate,
    delivery: TemplateDelivery,
) -> Result<GraphSpec, ApiError> {
    template.materialize(delivery).map_err(internal)
}

fn named_node<'a>(graph: &'a GraphSpec, name: &str) -> Result<&'a GraphNode, ApiError> {
    let mut pending = vec![&graph.root];
    while let Some(node) = pending.pop() {
        if node.name().as_str() == name {
            return Ok(node);
        }
        pending.extend(graph_node_children(node));
    }
    Err(ApiError::internal("Missing template worker.".into()))
}

fn template_label(template: BuiltinGraphTemplate, delivery: TemplateDelivery) -> &'static str {
    match (template, delivery) {
        (BuiltinGraphTemplate::SingleWorker, _) => "Single worker",
        (BuiltinGraphTemplate::SoftwareChange, TemplateDelivery::None) => "Software change",
        (BuiltinGraphTemplate::SoftwareChange, TemplateDelivery::Push) => "Software change · push",
        (BuiltinGraphTemplate::SoftwareChange, TemplateDelivery::PullRequest) => {
            "Software change · pull request"
        }
        (BuiltinGraphTemplate::SoftwareChange, TemplateDelivery::Merge) => {
            "Software change · merge"
        }
    }
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(error.to_string())
}

#[cfg(test)]
mod tests {
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
            assert_eq!(
                choice["runtimeBinding"],
                json!({"kind":"git_delivery","connections":{"github":["GH_TOKEN"]}})
            );
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
        assert_eq!(templates.len(), 5);
        for template in templates {
            let kind = BuiltinGraphTemplate::parse(template["name"].as_str().assert_value())
                .assert_value();
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
                    bindings
                        .entry(node.name().to_string())
                        .or_insert_with(|| json!({"kind":"agent","model":"opaque-model"}));
                }
                pending.extend(graph_node_children(node));
            }
            let runtime: RuntimePlan = serde_json::from_value(
                json!({"harness":"codex","provider":"openai","size":"small","nodes":bindings}),
            )
            .assert_value();
            super::super::admit(&graph, &runtime).await.assert_value();
        }
    }
}
