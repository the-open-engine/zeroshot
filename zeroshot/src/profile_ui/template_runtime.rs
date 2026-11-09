//! Explicit built-in runtime defaults; provider access still belongs to the native registry.

use openengine_cluster_protocol::{GraphNode, GraphSpec, RuntimePlan};
use openengine_cluster_server::graph_verifier::graph_node_children;
use serde_json::{json, Map, Value};

use super::{BuiltinGraphTemplate, TemplateDelivery, WorkspaceError};

pub(super) fn for_template(
    template: BuiltinGraphTemplate,
    delivery: TemplateDelivery,
    graph: &GraphSpec,
) -> Result<RuntimePlan, WorkspaceError> {
    let mut runtime = match template {
        BuiltinGraphTemplate::CodeReview => serde_json::from_str(include_str!(
            "../../../docs/assets/code-review/runtime.json"
        )),
        BuiltinGraphTemplate::PrReview => {
            serde_json::from_str(include_str!("../../../docs/assets/pr-review/runtime.json"))
        }
        _ => Ok(json!({"harness":"codex", "provider":"openai", "nodes":{}})),
    }
    .map_err(internal)?;
    let object = runtime
        .as_object_mut()
        .ok_or_else(|| WorkspaceError::internal("A built-in runtime must be an object.".into()))?;
    // Example CLI files may select local machine/snapshot policy. Catalog defaults are portable.
    object.remove("size");
    object.remove("workspaceCheckpoints");
    let nodes = object["nodes"].as_object_mut().ok_or_else(|| {
        WorkspaceError::internal("A built-in runtime must contain node bindings.".into())
    })?;
    fill_bindings(&graph.root, template, nodes);
    if let Some((name, binding)) = template
        .delivery_runtime_binding(delivery)
        .map_err(internal)?
    {
        // Catalog delivery follows source selection; the docs file is a GitHub-specific example.
        nodes.insert(
            name.to_string(),
            serde_json::to_value(binding).map_err(internal)?,
        );
    }
    if template == BuiltinGraphTemplate::ServiceTask {
        nodes["worker"]["connections"] = json!({
            "project_service":["PROJECT_SERVICE_ENDPOINT", "PROJECT_SERVICE_API_KEY"]
        });
    }
    serde_json::from_value(runtime).map_err(internal)
}

fn fill_bindings(node: &GraphNode, template: BuiltinGraphTemplate, nodes: &mut Map<String, Value>) {
    match node {
        GraphNode::Step(_) | GraphNode::Verifier(_) => {
            nodes.entry(node.name().to_string()).or_insert_with(|| {
                let mut binding = json!({
                    "kind":"agent", "model":"gpt-6.1-sol", "effort":"high",
                    "sessionScope":template.default_session_scope(node.name().as_str()),
                });
                if template == BuiltinGraphTemplate::CrossProviderTask
                    && node.name().as_str() != "worker"
                {
                    binding["lane"] = json!({"harness":"claude", "provider":"anthropic"});
                    binding["model"] = json!("claude-opus-5-5");
                }
                binding
            });
        }
        _ => {
            for child in graph_node_children(node) {
                fill_bindings(child, template, nodes);
            }
        }
    }
}

pub(super) fn description(template: BuiltinGraphTemplate) -> &'static str {
    match template {
        BuiltinGraphTemplate::SingleWorker => "Give one agent a focused task.",
        BuiltinGraphTemplate::SoftwareChange => "Turn a task into a reviewed software change.",
        BuiltinGraphTemplate::AutoResearch => {
            "Explore and test approaches to improve your project."
        }
        BuiltinGraphTemplate::IssueReview => "Find gaps and clarify what an issue needs.",
        BuiltinGraphTemplate::PrReview => "Reproduce changed behavior and check a pull request.",
        BuiltinGraphTemplate::CodeReview => {
            "Get a detailed code review from independent specialists."
        }
        BuiltinGraphTemplate::DocumentationTask => {
            "Update documentation and check its examples against the code."
        }
        BuiltinGraphTemplate::CrossProviderTask => {
            "Build a change and cross-check it with independent reviewers."
        }
        BuiltinGraphTemplate::ServiceTask => {
            "Complete an integration task with access to your project service."
        }
    }
}

fn internal(error: impl std::fmt::Display) -> WorkspaceError {
    WorkspaceError::internal(error.to_string())
}
