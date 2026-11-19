//! Shared expansion of runtime declarations against an unchanged authored graph.
use std::collections::BTreeMap;
use openengine_cluster_protocol::{
    DeclaredConnections, GraphSpec, NodeName, NodeRuntimeBinding, PullRequestFeedback,
    RunResumeRuntime, RuntimePlan, SessionScope, UniformRuntimePlan,
};

pub(crate) fn resolve(config: RunResumeRuntime, graph: &GraphSpec) -> RuntimePlan {
    match config {
        RunResumeRuntime::Exact { runtime } => runtime,
        RunResumeRuntime::Uniform { runtime } => {
            expand_uniform(runtime, graph, PullRequestFeedback::Consider)
        }
    }
}

pub(crate) fn agent(uniform: &UniformRuntimePlan) -> NodeRuntimeBinding {
    NodeRuntimeBinding::Agent {
        lane: None,
        model: uniform.model.clone(),
        effort: uniform.effort,
        harness_config: uniform.harness_config.clone(),
        context_window: uniform.context_window,
        max_tokens: uniform.max_tokens,
        session_scope: uniform.session_scope.unwrap_or(SessionScope::Execution),
        connections: uniform
            .connections
            .clone()
            .unwrap_or_else(DeclaredConnections::empty),
    }
}

pub(crate) fn expand_uniform(
    uniform: UniformRuntimePlan,
    graph: &GraphSpec,
    feedback: PullRequestFeedback,
) -> RuntimePlan {
    let mut nodes = BTreeMap::new();
    for (name, delivery) in crate::native_v2_admission::executable_runtime_roles(&graph.root) {
        let binding = if is_wait_node(&graph.root, &name) {
            NodeRuntimeBinding::Request {}
        } else if is_comment_node(&graph.root, &name) {
            NodeRuntimeBinding::CommentDelivery {
                connections: DeclaredConnections::empty(),
            }
        } else if delivery {
            NodeRuntimeBinding::GitDelivery {
                connections: DeclaredConnections::empty(),
                pull_request_feedback: feedback,
            }
        } else {
            agent(&uniform)
        };
        nodes.insert(name, binding);
    }
    uniform.into_runtime_plan(nodes)
}

fn is_wait_node(root: &openengine_cluster_protocol::GraphNode, name: &NodeName) -> bool {
    if root.name() == name
        && let openengine_cluster_protocol::GraphNode::Verifier(node) = root
    {
        return [
            crate::native_v2_wait::WAIT_WORKER_REF,
            crate::native_v2_request::ASK_WORKER_REF,
        ]
        .contains(&node.worker.as_str());
    }
    openengine_cluster_server::graph_verifier::graph_node_children(root)
        .into_iter()
        .any(|child| is_wait_node(child, name))
}

fn is_comment_node(root: &openengine_cluster_protocol::GraphNode, name: &NodeName) -> bool {
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if node.name() == name
            && let openengine_cluster_protocol::GraphNode::Verifier(node) = node
        {
            return node.worker.as_str()
                == crate::native_v2_comment_delivery::COMMENT_DELIVERY_WORKER_REF;
        }
        pending.extend(openengine_cluster_server::graph_verifier::graph_node_children(node));
    }
    false
}

/// Materialize ordinary provider defaults before selecting fresh caller credentials.
pub(crate) fn materialize_access(
    config: &mut RunResumeRuntime,
    placement: crate::native_v2_candidate::ProviderAccessPlacement,
) -> Result<(), crate::harnesses::HarnessError> {
    match config {
        RunResumeRuntime::Exact { runtime } => {
            crate::native_v2_candidate::materialize_provider_access(runtime, placement)
        }
        RunResumeRuntime::Uniform { runtime } => {
            let mut binding = agent(runtime);
            crate::native_v2_candidate::materialize_provider_binding(
                &mut binding,
                openengine_cluster_protocol::RuntimeLane {
                    harness: runtime.harness.clone(),
                    provider: runtime.provider.clone(),
                },
                placement,
            )?;
            if let NodeRuntimeBinding::Agent { connections, .. } = binding {
                runtime.connections = Some(connections);
            }
            Ok(())
        }
    }
}

pub(crate) fn requirements(
    config: &RunResumeRuntime,
) -> openengine_cluster_protocol::RunConnectionRequirements {
    let requirements = match config {
        RunResumeRuntime::Exact { runtime } => runtime.connection_requirements(),
        RunResumeRuntime::Uniform { runtime } => runtime
            .connections
            .clone()
            .unwrap_or_else(DeclaredConnections::empty)
            .iter()
            .map(|(key, fields)| (key.clone(), fields.iter().cloned().collect()))
            .collect(),
    };
    requirements
        .into_iter()
        .map(|(key, fields)| (key, fields.into_iter().collect()))
        .collect()
}

pub(crate) fn merge_requirements(
    base: &openengine_cluster_protocol::RunConnectionRequirements,
    runtime: &RunResumeRuntime,
) -> openengine_cluster_protocol::RunConnectionRequirements {
    let mut merged: BTreeMap<_, std::collections::BTreeSet<_>> = base
        .iter()
        .map(|(key, fields)| (key.clone(), fields.iter().cloned().collect()))
        .collect();
    for (key, fields) in requirements(runtime) {
        merged.entry(key).or_default().extend(fields);
    }
    merged
        .into_iter()
        .map(|(key, fields)| (key, fields.into_iter().collect()))
        .collect()
}

/// Apply the same source-bound delivery defaults used by ordinary named submissions.
pub(crate) fn replacement_submission(
    admitted: &crate::native_v2_contract::AdmittedRun,
    config: RunResumeRuntime,
    submission_key: openengine_cluster_protocol::IdempotencyKey,
) -> Result<
    openengine_cluster_protocol::RunSubmission,
    crate::native_v2_admission::NativeV2AdmissionError,
> {
    let graph = GraphSpec {
        profile: admitted.graph.profile,
        initial_input: admitted.graph.initial_input.clone(),
        policy: admitted.graph.policy.clone(),
        root: admitted.graph.root.clone(),
        networks: admitted.graph.networks.clone(),
    };
    let runtime = resolve(config, &graph);
    let mut submission = openengine_cluster_protocol::RunSubmission {
        title: admitted.title.clone(),
        graph,
        initial_input: admitted.initial_input.clone(),
        runtime,
        environment: admitted.environment.clone(),
        source: admitted.source.clone(),
        context_sources: admitted.context_sources.clone(),
        submission_key,
    };
    crate::native_v2_admission::materialize_delivery_access(&mut submission)?;
    Ok(submission)
}
