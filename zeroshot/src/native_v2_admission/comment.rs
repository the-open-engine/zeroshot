use openengine_cluster_protocol::{
    DataSelector, FieldPath, GraphNode, GraphSpec, PayloadType, VerifierNode,
};
use serde_json::Value;

use super::NativeV2AdmissionError;
use crate::native_v2_contract::AdmittedRun;
use crate::native_v2_comment_delivery::COMMENT_DELIVERY_WORKER_REF;

pub(crate) struct CommentInput {
    pub url: String,
    pub destination: crate::source_code_provider::SourceCommentDestination,
    pub review_scope: Option<Value>,
}

fn invalid(message: &str) -> NativeV2AdmissionError {
    NativeV2AdmissionError::InvalidCommentDelivery(message.to_owned())
}

pub(crate) fn comment_node(root: &GraphNode) -> Option<&VerifierNode> {
    if let GraphNode::Verifier(node) = root
        && node.worker.as_str() == COMMENT_DELIVERY_WORKER_REF
    {
        return Some(node);
    }
    openengine_cluster_server::graph_verifier::graph_node_children(root)
        .into_iter()
        .find_map(comment_node)
}

fn root_binding<'a>(
    node: &'a VerifierNode,
    field: &str,
) -> Result<Option<&'a FieldPath>, NativeV2AdmissionError> {
    let binding = node.input_bindings.iter().find(|binding| {
        binding.target.segments().len() == 1 && binding.target.segments()[0].as_str() == field
    });
    match binding.map(|binding| &binding.value) {
        Some(DataSelector::State { path }) => Ok(Some(path)),
        None => Ok(None),
        _ => Err(invalid(
            "comment destination and scope must bind directly from caller input",
        )),
    }
}

fn required_path<'a>(mut schema: &'a PayloadType, path: &FieldPath) -> Option<&'a PayloadType> {
    for segment in path.segments() {
        let PayloadType::Record { fields } = schema else {
            return None;
        };
        let field = fields.get(segment)?;
        if !field.required {
            return None;
        }
        schema = &field.value_type;
    }
    Some(schema)
}

fn value_at<'a>(mut value: &'a Value, path: &FieldPath) -> Option<&'a Value> {
    for segment in path.segments() {
        value = value.get(segment.as_str())?;
    }
    Some(value)
}

pub(super) fn validate_comment_profile(graph: &GraphSpec) -> Result<(), NativeV2AdmissionError> {
    let Some(node) = comment_node(&graph.root) else {
        return Ok(());
    };
    let path = root_binding(node, "url")?
        .ok_or_else(|| invalid("comment URL requires a caller input binding"))?;
    if required_path(&graph.initial_input, path) != Some(&PayloadType::String) {
        return Err(invalid(
            "comment URL must select a required string in initial input",
        ));
    }
    if let Some(path) = root_binding(node, "reviewScope")?
        && required_path(&graph.initial_input, path).is_none()
    {
        return Err(invalid(
            "comment reviewScope must select a required initial input value",
        ));
    }
    validate_profile_inputs(&graph.root, node, &graph.initial_input)?;
    let mut published = false;
    validate_position(&graph.root, false, &mut published)
}

fn validate_profile_inputs(
    root: &GraphNode,
    delivery: &VerifierNode,
    initial_input: &PayloadType,
) -> Result<bool, NativeV2AdmissionError> {
    if root.name() == &delivery.name {
        return Ok(true);
    }
    for child in openengine_cluster_server::graph_verifier::graph_node_children(root) {
        if validate_profile_inputs(child, delivery, initial_input)? {
            if let GraphNode::Profile(profile) = root {
                let input = profile
                    .initial_input
                    .as_ref()
                    .ok_or_else(|| invalid("comment delivery requires a resolved profile input"))?;
                for field in ["url", "reviewScope"] {
                    if let Some(path) = root_binding(delivery, field)?
                        && !preserves_required_input(initial_input, input, path)
                    {
                        return Err(invalid(
                            "comment destination and scope must pass unchanged through profile inputs",
                        ));
                    }
                }
            }
            return Ok(true);
        }
    }
    Ok(false)
}

fn preserves_required_input(source: &PayloadType, target: &PayloadType, path: &FieldPath) -> bool {
    required_path(source, path)
        .zip(required_path(target, path))
        .is_some_and(|(source, target)| preserves_projection(source, target))
}

fn preserves_projection(source: &PayloadType, target: &PayloadType) -> bool {
    match (source, target) {
        (PayloadType::Record { fields: source }, PayloadType::Record { fields: target }) => {
            source.iter().all(|(name, field)| {
                target
                    .get(name)
                    .is_some_and(|other| preserves_projection(&field.value_type, &other.value_type))
            })
        }
        (PayloadType::Array { items: source }, PayloadType::Array { items: target }) => {
            preserves_projection(source, target)
        }
        _ => source.is_subtype_of(target),
    }
}

fn validate_position(
    node: &GraphNode,
    repeated: bool,
    published: &mut bool,
) -> Result<(), NativeV2AdmissionError> {
    let is_comment =
        matches!(node, GraphNode::Verifier(v) if v.worker.as_str() == COMMENT_DELIVERY_WORKER_REF);
    if matches!(node, GraphNode::Step(_) | GraphNode::Verifier(_)) {
        if *published {
            return Err(invalid(
                "comment delivery must be the final executable node",
            ));
        }
        if is_comment && repeated {
            return Err(invalid(
                "comment delivery cannot execute inside a loop or map",
            ));
        }
        *published |= is_comment;
    }
    let repeated = repeated || matches!(node, GraphNode::Loop(_) | GraphNode::Map(_));
    for child in openengine_cluster_server::graph_verifier::graph_node_children(node) {
        validate_position(child, repeated, published)?;
    }
    Ok(())
}

struct UnresolvedCommentInput<'a> {
    url: &'a str,
    review_scope: Option<Value>,
}

fn unresolved_input<'a>(
    root: &GraphNode,
    input: &'a Value,
) -> Result<Option<UnresolvedCommentInput<'a>>, NativeV2AdmissionError> {
    let Some(node) = comment_node(root) else {
        return Ok(None);
    };
    let path =
        root_binding(node, "url")?.ok_or_else(|| invalid("comment URL requires caller input"))?;
    let url = value_at(input, path)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("comment URL is missing"))?;
    let review_scope = root_binding(node, "reviewScope")?
        .map(|path| {
            value_at(input, path)
                .cloned()
                .ok_or_else(|| invalid("comment reviewScope is missing"))
        })
        .transpose()?;
    Ok(Some(UnresolvedCommentInput { url, review_scope }))
}

pub(crate) fn resolve_comment_input_at(
    root: &GraphNode,
    input: &Value,
    forge: Option<&openengine_cluster_protocol::SourceForgeIdentity>,
) -> Result<Option<CommentInput>, NativeV2AdmissionError> {
    let Some(input) = unresolved_input(root, input)? else {
        return Ok(None);
    };
    let destination =
        crate::source_code_provider::adapters::core::comment_destination_with_context(
            input.url, forge,
        )
        .map_err(|error| NativeV2AdmissionError::InvalidCommentDelivery(error.to_string()))?;
    if let Some(scope) = &input.review_scope {
        crate::source_code_provider::adapters::core::validate_comment_review_scope(
            &destination,
            scope,
        )
        .map_err(|error| NativeV2AdmissionError::InvalidCommentDelivery(error.to_string()))?;
    }
    Ok(Some(CommentInput {
        url: input.url.to_owned(),
        destination,
        review_scope: input.review_scope,
    }))
}

pub(crate) fn admitted_comment_input(
    admitted: &AdmittedRun,
) -> Result<Option<CommentInput>, NativeV2AdmissionError> {
    resolve_comment_input_at(
        &admitted.graph.root,
        &admitted.initial_input,
        admitted
            .source
            .as_ref()
            .and_then(|source| source.remote.as_ref())
            .and_then(|remote| remote.forge.as_ref()),
    )
}

/// Source-neutral preparation validates syntax; source-aware admission binds repository identity.
pub(super) fn validate_comment_input_shape(
    root: &GraphNode,
    input: &Value,
) -> Result<(), NativeV2AdmissionError> {
    let Some(input) = unresolved_input(root, input)? else {
        return Ok(());
    };
    crate::source_code_provider::adapters::core::comment_destination(input.url)
        .map_err(|error| NativeV2AdmissionError::InvalidCommentDelivery(error.to_string()))?;
    if input
        .review_scope
        .as_ref()
        .is_some_and(|scope| !scope.is_object())
    {
        return Err(invalid("comment reviewScope must be an object"));
    }
    Ok(())
}
