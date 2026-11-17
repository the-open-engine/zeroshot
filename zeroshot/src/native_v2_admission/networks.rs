use super::*;

pub(super) fn validate_graph_bindings(
    graph: &GraphSpec,
    runtime: &RuntimePlan,
    context: NativeV2AdmissionContext<'_>,
) -> Result<Vec<ExecutableDeclaration>, NativeV2AdmissionError> {
    let declarations = executable_declarations(&graph.root);
    validate_executable_bindings(&declarations, runtime, context)?;
    validate_network_bindings(graph, runtime)?;
    Ok(declarations)
}

fn validate_network_bindings(
    graph: &GraphSpec,
    runtime: &RuntimePlan,
) -> Result<(), NativeV2AdmissionError> {
    for (network, declaration) in &graph.networks {
        for node in declaration.members.union(&declaration.publishers) {
            if !matches!(
                runtime.nodes().get(node),
                Some(NodeRuntimeBinding::Agent { .. })
            ) {
                return Err(NativeV2AdmissionError::InvalidRuntimeBinding {
                    node: node.clone(),
                    detail: format!("network {network} participants require an agent binding"),
                });
            }
        }
    }
    Ok(())
}
