use openengine_cluster_protocol::{ChoiceNode, FieldPath, GraphNode, LoopNode, PositiveInteger};
use serde_json::Value;

use super::NativeV2AdmissionError;

pub(super) fn bind_input_limits(
    node: &mut GraphNode,
    initial_input: &Value,
) -> Result<(), NativeV2AdmissionError> {
    match node {
        GraphNode::Seq(group) => bind_many(group.children.as_mut_slice(), initial_input),
        GraphNode::Choice(group) => bind_choice(group, initial_input),
        GraphNode::Par(group) => bind_many(group.branches.as_mut_slice(), initial_input),
        GraphNode::Loop(group) => bind_loop(group, initial_input),
        GraphNode::Map(group) => bind_input_limits(&mut group.body, initial_input),
        GraphNode::Step(_)
        | GraphNode::Verifier(_)
        | GraphNode::Succeed(_)
        | GraphNode::Fail(_) => Ok(()),
    }
}

fn bind_many(nodes: &mut [GraphNode], input: &Value) -> Result<(), NativeV2AdmissionError> {
    for node in nodes {
        bind_input_limits(node, input)?;
    }
    Ok(())
}

fn bind_choice(group: &mut ChoiceNode, input: &Value) -> Result<(), NativeV2AdmissionError> {
    for branch in group.branches.as_mut_slice() {
        bind_input_limits(&mut branch.node, input)?;
    }
    if let Some(otherwise) = &mut group.otherwise {
        bind_input_limits(otherwise, input)?;
    }
    Ok(())
}

fn bind_loop(group: &mut LoopNode, input: &Value) -> Result<(), NativeV2AdmissionError> {
    if let Some(path) = &group.max_iterations_input {
        if let Some(value) = input_at_path(input, path) {
            group.max_iterations = value
                .as_u64()
                .and_then(|value| PositiveInteger::new(value).ok())
                .ok_or_else(|| NativeV2AdmissionError::InvalidLoopLimit {
                    path: path
                        .segments()
                        .iter()
                        .map(|segment| segment.as_str())
                        .collect::<Vec<_>>()
                        .join("."),
                })?;
        }
    }
    bind_input_limits(&mut group.body, input)
}

fn input_at_path<'a>(input: &'a Value, path: &FieldPath) -> Option<&'a Value> {
    path.segments()
        .iter()
        .try_fold(input, |value, segment| value.get(segment.as_str()))
}
