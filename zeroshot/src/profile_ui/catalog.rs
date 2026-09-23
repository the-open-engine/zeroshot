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

use super::{WorkspaceError as ApiError, BuiltinGraphTemplate, TemplateDelivery};

pub(super) fn templates() -> Result<Vec<Value>, ApiError> {
    let mut result = Vec::new();
    for template in BuiltinGraphTemplate::all() {
        let deliveries = match template {
            BuiltinGraphTemplate::SingleWorker => &[TemplateDelivery::None][..],
            BuiltinGraphTemplate::SoftwareChange => &[
                TemplateDelivery::None,
                TemplateDelivery::Push,
                TemplateDelivery::PullRequest,
                TemplateDelivery::Merge,
            ][..],
            BuiltinGraphTemplate::AutoResearch => {
                &[TemplateDelivery::None, TemplateDelivery::Push][..]
            }
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
        (BuiltinGraphTemplate::AutoResearch, TemplateDelivery::None) => "Auto research",
        (BuiltinGraphTemplate::AutoResearch, TemplateDelivery::Push) => "Auto research · push",
        (BuiltinGraphTemplate::AutoResearch, _) => "Auto research",
    }
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(error.to_string())
}

#[cfg(test)]
#[path = "catalog/tests.rs"]
mod tests;
