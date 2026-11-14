use openengine_cluster_protocol::{GraphNode, NodeRuntimeBinding, PayloadType};
use super::{NativeV2AdmissionError, validation::ExecutableDeclaration};
use crate::native_v2_wait;

pub(super) fn validate_position(
    node: &GraphNode,
    concurrent: bool,
) -> Result<(), NativeV2AdmissionError> {
    if let GraphNode::Verifier(wait) = node
        && wait.worker.as_str() == native_v2_wait::WAIT_WORKER_REF
        && (concurrent || wait.timeout_ms.is_some())
    {
        return Err(NativeV2AdmissionError::InvalidRuntimeBinding {
            node: wait.name.clone(),
            detail: "wait forbids timeoutMs and placement under par or map".to_owned(),
        });
    }
    let concurrent = concurrent || matches!(node, GraphNode::Par(_) | GraphNode::Map(_));
    for child in openengine_cluster_server::graph_verifier::graph_node_children(node) {
        validate_position(child, concurrent)?;
    }
    Ok(())
}

pub(super) fn validate_binding(
    node: &ExecutableDeclaration,
    binding: &NodeRuntimeBinding,
) -> Result<(), NativeV2AdmissionError> {
    let wait = node.worker.as_str() == native_v2_wait::WAIT_WORKER_REF;
    if !wait && !matches!(binding, NodeRuntimeBinding::Request {}) {
        return Ok(());
    }
    let valid = wait
        && matches!(binding, NodeRuntimeBinding::Request {})
        && node.instructions.is_none()
        && node.attempts == 1
        && native_v2_wait::input_schema().ok().as_ref() == Some(&node.contract.input)
        && node.contract.output == PayloadType::Null
        && node.contract.verifier.as_ref().is_some_and(|contract| {
            native_v2_wait::signal_labels().ok().as_ref() == Some(&contract.signals)
                && native_v2_wait::diagnostic_schema().ok().as_ref() == Some(&contract.diagnostic)
        });
    if valid {
        Ok(())
    } else {
        Err(NativeV2AdmissionError::InvalidRuntimeBinding {
        node: node.name.clone(), detail: "request requires the fixed wait verifier contract, no instructions, and attempts: 1".to_owned(),
    })
    }
}
