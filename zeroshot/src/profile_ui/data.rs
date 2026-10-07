//! Input/output authoring lowers to native state, writes, and promotion paths.
//! Drafts stay editable; native profile admission proves availability before saving.

use std::collections::{BTreeMap, BTreeSet};

use openengine_cluster_protocol::{FieldName, GraphSpec, NodeName, PayloadType};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::WorkspaceError as ApiError;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    graph: Value,
    runtime: Value,
    action: Action,
}

#[derive(Serialize)]
pub(super) struct Document {
    graph: Value,
    runtime: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    node: NodeName,
    input: FieldName,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
enum Source {
    RunInput {
        path: Vec<FieldName>,
    },
    NodeOutput {
        node: NodeName,
        channel: Channel,
        path: Vec<FieldName>,
    },
    MapItem {
        path: Vec<FieldName>,
    },
    LoopInput {
        node: NodeName,
        path: Vec<FieldName>,
    },
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Channel {
    Out,
    Signal,
    Diagnostic,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
enum Action {
    Connect {
        target: Target,
        source: Source,
    },
    RemoveInput {
        target: Target,
    },
    MapCollection {
        node: NodeName,
        source: Source,
    },
    RunInputField {
        before: Option<FieldName>,
        name: FieldName,
        #[serde(rename = "type")]
        value_type: PayloadType,
        required: bool,
    },
    RemoveRunInput {
        name: FieldName,
    },
}

#[derive(Clone)]
struct Node {
    pointer: String,
    parent: Option<String>,
    order: usize,
    kind: String,
}
type Index = BTreeMap<String, Node>;

struct Location {
    pointer: String,
    parent: Option<String>,
    order: usize,
}
struct InputEdit<'a> {
    node: &'a str,
    input: &'a str,
    value_type: Value,
    selector: Value,
}
struct RunInputEdit<'a> {
    before: Option<&'a str>,
    name: &'a str,
    field: Option<Value>,
}

#[derive(Clone)]
struct Producer {
    name: String,
    channel: Channel,
    path: Vec<String>,
    value_type: Value,
}

struct ExistingLoopInput<'a> {
    node: &'a str,
    path: &'a [String],
}

pub(super) fn apply(request: Request) -> Result<Document, ApiError> {
    let Request {
        mut graph,
        runtime,
        action,
    } = request;
    let index = index(&graph)?;
    match action {
        Action::Connect { target, source } => {
            let mut nodes = index;
            // Establish success handling before allocating any routing fields. The
            // request owns this draft; either all edits return together or none do.
            if let Source::NodeOutput { node, .. } = &source {
                if structurally_complete(&graph["root"]) {
                    let mut typed: GraphSpec =
                        serde_json::from_value(graph).map_err(value_error)?;
                    super::outcomes::ensure_required_output(
                        &mut typed,
                        &runtime,
                        node,
                        &target.node,
                    )?;
                    graph = serde_json::to_value(typed).map_err(value_error)?;
                    nodes = self::index(&graph)?;
                }
            }
            let target_node = target.node.as_str();
            let (selector, value_type) = source_route(&mut graph, &nodes, target_node, &source)?;
            set_input(
                &mut graph,
                &nodes,
                InputEdit {
                    node: target_node,
                    input: target.input.as_str(),
                    value_type,
                    selector,
                },
            )?;
        }
        Action::RemoveInput { target } => remove_input(
            &mut graph,
            &index,
            target.node.as_str(),
            target.input.as_str(),
        )?,
        Action::MapCollection { node, source } => {
            if get(&graph, &index, node.as_str())?["kind"] != "map" {
                return Err(invalid("Select a map."));
            }
            let (selector, value_type) = source_route(&mut graph, &index, node.as_str(), &source)?;
            if value_type["kind"] != "array" {
                return Err(invalid("Select a list."));
            }
            get_mut(&mut graph, &index, node.as_str())?["over"] = selector;
            refresh_map_inputs(&mut graph, &index, node.as_str(), &value_type["items"])?;
        }
        Action::RunInputField {
            before,
            name,
            value_type,
            required,
        } => {
            edit_run_input(
                &mut graph,
                &index,
                RunInputEdit {
                    before: before.as_ref().map(FieldName::as_str),
                    name: name.as_str(),
                    field: Some(json!({"type":value_type,"required":required})),
                },
            )?;
        }
        Action::RemoveRunInput { name } => edit_run_input(
            &mut graph,
            &index,
            RunInputEdit {
                before: Some(name.as_str()),
                name: name.as_str(),
                field: None,
            },
        )?,
    }
    Ok(Document { graph, runtime })
}

fn index(graph: &Value) -> Result<Index, ApiError> {
    let mut index = BTreeMap::new();
    index_node(
        &graph["root"],
        Location {
            pointer: "/root".into(),
            parent: None,
            order: 0,
        },
        &mut index,
    )?;
    Ok(index)
}

fn index_node(node: &Value, location: Location, index: &mut Index) -> Result<(), ApiError> {
    let Location {
        pointer,
        parent,
        order,
    } = location;
    let name = node["name"]
        .as_str()
        .ok_or_else(|| invalid("A node has no name."))?;
    NodeName::new(name).map_err(value_error)?;
    let kind = node["kind"]
        .as_str()
        .ok_or_else(|| invalid("A node has no type."))?;
    if index
        .insert(
            name.into(),
            Node {
                pointer: pointer.clone(),
                parent,
                order,
                kind: kind.into(),
            },
        )
        .is_some()
    {
        return Err(invalid("Node names must be unique."));
    }
    for (suffix, child) in child_values(node) {
        index_node(
            child,
            Location {
                pointer: format!("{pointer}{suffix}"),
                parent: Some(name.into()),
                order: index_child_order(&suffix),
            },
            index,
        )?;
    }
    Ok(())
}

fn index_child_order(suffix: &str) -> usize {
    suffix
        .split('/')
        .filter_map(|part| part.parse::<usize>().ok())
        .next()
        .unwrap_or(0)
}

fn child_values(node: &Value) -> Vec<(String, &Value)> {
    match node["kind"].as_str() {
        Some("seq") => node["children"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, node)| (format!("/children/{i}"), node))
            .collect(),
        Some("par") => node["branches"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, node)| (format!("/branches/{i}"), node))
            .collect(),
        Some("choice") => {
            let mut children: Vec<_> = node["branches"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(i, branch)| (format!("/branches/{i}/node"), &branch["node"]))
                .collect();
            if node["otherwise"].is_object() {
                children.push(("/otherwise".into(), &node["otherwise"]));
            }
            children
        }
        Some("loop" | "map") if node["body"].is_object() => vec![("/body".into(), &node["body"])],
        _ => Vec::new(),
    }
}

fn get<'a>(graph: &'a Value, index: &Index, name: &str) -> Result<&'a Value, ApiError> {
    graph
        .pointer(
            &index
                .get(name)
                .ok_or_else(|| invalid("Select an existing node."))?
                .pointer,
        )
        .ok_or_else(|| invalid("Node not found."))
}
fn get_mut<'a>(graph: &'a mut Value, index: &Index, name: &str) -> Result<&'a mut Value, ApiError> {
    graph
        .pointer_mut(
            &index
                .get(name)
                .ok_or_else(|| invalid("Select an existing node."))?
                .pointer,
        )
        .ok_or_else(|| invalid("Node not found."))
}
fn ancestry(index: &Index, name: &str) -> Result<Vec<String>, ApiError> {
    let mut path = Vec::new();
    let mut next = Some(name.to_owned());
    while let Some(name) = next {
        next = index
            .get(&name)
            .ok_or_else(|| invalid("Node not found."))?
            .parent
            .clone();
        path.push(name);
    }
    path.reverse();
    Ok(path)
}
fn path_strings(path: &[FieldName]) -> Result<Vec<String>, ApiError> {
    if path.is_empty() {
        return Err(invalid("Select a named field."));
    }
    Ok(path.iter().map(|part| part.as_str().to_owned()).collect())
}

fn selected_type(schema: &Value, path: &[String]) -> Result<Value, ApiError> {
    let mut current = schema;
    for segment in path {
        let field = current["fields"]
            .get(segment)
            .ok_or_else(|| invalid("The selected field no longer exists."))?;
        if field["required"] != true {
            return Err(invalid("The selected output must be required."));
        }
        current = &field["type"];
    }
    serde_json::from_value::<PayloadType>(current.clone()).map_err(value_error)?;
    Ok(current.clone())
}

fn source_route(
    graph: &mut Value,
    index: &Index,
    consumer: &str,
    source: &Source,
) -> Result<(Value, Value), ApiError> {
    match source {
        Source::RunInput { path } => {
            let path = path_strings(path)?;
            let value_type = selected_type(&graph["initialInput"], &path)?;
            if state_path_written(graph, &path) {
                return Err(invalid("This run input is overwritten by the graph."));
            }
            let field = graph["initialInput"]["fields"][&path[0]].clone();
            for name in ancestry(index, consumer)?.into_iter().filter(|name| {
                index[name].kind != "step"
                    && index[name].kind != "verifier"
                    && index[name].kind != "succeed"
                    && index[name].kind != "fail"
            }) {
                add_field(get_mut(graph, index, &name)?, &path[0], field.clone())?;
            }
            Ok((json!({"source":"state","path":path}), value_type))
        }
        Source::MapItem { path } => {
            let path = path_strings(path)?;
            let parents = ancestry(index, consumer)?;
            let owner = parents
                .iter()
                .rev()
                .skip(1)
                .find(|name| index[*name].kind == "map")
                .ok_or_else(|| invalid("Select an input inside a map."))?;
            let map = get(graph, index, owner)?;
            let over = &map["over"];
            if over["source"] != "state" {
                return Err(invalid("Configure the map list first."));
            }
            let selected = over["path"]
                .as_array()
                .ok_or_else(|| invalid("Configure the map list first."))?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| invalid("Invalid map list."))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut schema = &map["state"];
            for part in selected {
                schema = &schema["fields"][part]["type"];
            }
            if schema["kind"] != "array" {
                return Err(invalid("Configure the map list first."));
            }
            Ok((
                json!({"source":"item","path":path}),
                selected_type(&schema["items"], &path)?,
            ))
        }
        Source::NodeOutput {
            node,
            channel,
            path,
        } => {
            let path = path_strings(path)?;
            let producers = producers(get(graph, index, node.as_str())?, *channel, &path)?;
            connect_outputs(graph, index, consumer, &producers)
        }
        Source::LoopInput { node, path } => loop_input_route(
            graph,
            index,
            consumer,
            ExistingLoopInput {
                node: node.as_str(),
                path: &path_strings(path)?,
            },
        ),
    }
}

fn structurally_complete(node: &Value) -> bool {
    let children = child_values(node);
    if node["kind"] == "choice" && node["branches"].as_array().is_none_or(Vec::is_empty) {
        return false;
    }
    (!matches!(
        node["kind"].as_str(),
        Some("seq" | "par" | "map" | "loop" | "choice")
    ) || !children.is_empty())
        && children
            .iter()
            .all(|(_, child)| structurally_complete(child))
}

fn field_path(value: &Value) -> Result<Vec<String>, ApiError> {
    let parts: Vec<FieldName> = serde_json::from_value(value.clone()).map_err(value_error)?;
    path_strings(&parts)
}

fn nearest_loop<'a>(index: &Index, path: &'a [String]) -> Option<&'a String> {
    path.iter()
        .rev()
        .skip(1)
        .find(|name| index[*name].kind == "loop")
}

fn previous_round_route(index: &Index, writer: &str, consumer: &str) -> Result<bool, ApiError> {
    if writer == consumer {
        return Ok(true);
    }
    let source = ancestry(index, writer)?;
    let target = ancestry(index, consumer)?;
    let shared = source
        .iter()
        .zip(&target)
        .take_while(|(a, b)| a == b)
        .count();
    Ok(shared > 0
        && shared < source.len()
        && shared < target.len()
        && index[&source[shared - 1]].kind == "seq"
        && index[&source[shared]].order > index[&target[shared]].order)
}

/// Reuse one already-authored feedback route. No slots, writes, promotions, or initial
/// values are added; the native verifier retains authority over the complete graph.
fn loop_input_route(
    graph: &Value,
    index: &Index,
    consumer: &str,
    source: ExistingLoopInput<'_>,
) -> Result<(Value, Value), ApiError> {
    let donor = get(graph, index, source.node)?;
    if !executable(donor) || !executable(get(graph, index, consumer)?) || source.path.len() != 1 {
        return Err(invalid(
            "Select an existing activity input inside the same loop.",
        ));
    }
    let (selector, path, value_type) = saved_feedback_input(donor, source.path)?;
    let donor_path = ancestry(index, source.node)?;
    let consumer_path = ancestry(index, consumer)?;
    let owner = nearest_loop(index, &donor_path)
        .ok_or_else(|| invalid("This input is not inside a loop."))?;
    if nearest_loop(index, &consumer_path) != Some(owner) {
        return Err(invalid("Reuse feedback only inside the same loop."));
    }

    // A copied selector must retain the same unambiguous writer, including nested
    // target collisions. An identically named path from another scope is not guessed.
    let (writer, write) = feedback_writer(graph, index, &path)?;
    let writer_path = ancestry(index, writer)?;
    if nearest_loop(index, &writer_path) != Some(owner)
        || !previous_round_route(index, writer, source.node)?
        || !previous_round_route(index, writer, consumer)?
    {
        return Err(invalid(
            "Select feedback already carried from the previous loop attempt.",
        ));
    }
    let channel = serde_json::from_value(write["value"]["channel"].clone()).map_err(value_error)?;
    let output_path = field_path(&write["value"]["path"])?;
    let output = producers(get(graph, index, writer)?, channel, &output_path)?;
    if output.len() != 1 || output[0].value_type != value_type {
        return Err(invalid("The saved feedback output has a different type."));
    }
    validate_feedback_scopes(
        graph,
        index,
        FeedbackScopeCheck {
            routes: [&donor_path, &consumer_path, &writer_path],
            owner,
            path: &path,
            value_type: &value_type,
        },
    )?;
    validate_initial_feedback(graph, &path, &value_type)?;
    Ok((selector, value_type))
}

struct FeedbackScopeCheck<'a> {
    routes: [&'a Vec<String>; 3],
    owner: &'a str,
    path: &'a [String],
    value_type: &'a Value,
}

fn validate_feedback_scopes(
    graph: &Value,
    index: &Index,
    check: FeedbackScopeCheck<'_>,
) -> Result<(), ApiError> {
    let FeedbackScopeCheck {
        routes,
        owner,
        path,
        value_type,
    } = check;
    for route in &routes {
        for name in route.iter().take(route.len() - 1) {
            let scope = get(graph, index, name)?;
            if scope["kind"] == "map" || selected_type(&scope["state"], path)? != *value_type {
                return Err(invalid(
                    "Feedback must keep the same required type in every enclosing scope.",
                ));
            }
        }
    }
    let writer_path = routes[2];
    let loop_index = writer_path
        .iter()
        .position(|name| name == owner)
        .ok_or_else(|| invalid("Loop not found."))?;
    for name in &writer_path[loop_index + 1..writer_path.len() - 1] {
        let scope = get(graph, index, name)?;
        let promoted = scope["promotedStatePaths"].as_array().is_some_and(|paths| {
            paths.iter().any(|candidate| {
                field_path(candidate).is_ok_and(|candidate| path.starts_with(&candidate))
            })
        });
        if !promoted {
            return Err(invalid(
                "The existing feedback is not returned to this loop.",
            ));
        }
    }
    Ok(())
}

fn validate_initial_feedback(
    graph: &Value,
    path: &[String],
    value_type: &Value,
) -> Result<(), ApiError> {
    let initial: PayloadType =
        serde_json::from_value(graph["initialInput"].clone()).map_err(value_error)?;
    let root_state: PayloadType =
        serde_json::from_value(graph["root"]["state"].clone()).map_err(value_error)?;
    let effective = initial
        .materialized_subtype_of(&root_state)
        .ok_or_else(|| invalid("The feedback has no defined value on the first attempt."))?;
    if selected_type(&serde_json::to_value(effective).map_err(value_error)?, path)? != *value_type {
        return Err(invalid("The feedback has no compatible initial value."));
    }
    Ok(())
}

fn saved_feedback_input(
    donor: &Value,
    source_path: &[String],
) -> Result<(Value, Vec<String>, Value), ApiError> {
    let value_type = selected_type(&donor["input"], source_path)?;
    let bindings = donor["inputBindings"]
        .as_array()
        .ok_or_else(|| invalid("This input has no saved source."))?;
    let overlapping = bindings
        .iter()
        .filter(|binding| {
            binding["target"]
                .as_array()
                .and_then(|path| path.first())
                .and_then(Value::as_str)
                == Some(source_path[0].as_str())
        })
        .collect::<Vec<_>>();
    let [binding] = overlapping.as_slice() else {
        return Err(invalid("Select one complete existing input mapping."));
    };
    if binding["target"] != json!(source_path) || binding["value"]["source"] != "state" {
        return Err(invalid("Select an existing loop feedback input."));
    }
    let selector = binding["value"].clone();
    let path = field_path(&selector["path"])?;
    Ok((selector, path, value_type))
}

fn feedback_writer<'a>(
    graph: &'a Value,
    index: &'a Index,
    path: &[String],
) -> Result<(&'a String, &'a Value), ApiError> {
    let mut writes = Vec::new();
    for (name, location) in index {
        let node = graph
            .pointer(&location.pointer)
            .ok_or_else(|| invalid("Node not found."))?;
        for binding in node["writeBindings"].as_array().into_iter().flatten() {
            let target = field_path(&binding["target"])?;
            if target.iter().zip(path).all(|(left, right)| left == right) {
                writes.push((name, binding, target));
            }
        }
    }
    let [(writer, write, target)] = writes.as_slice() else {
        return Err(invalid(
            "This feedback path needs one unambiguous existing writer.",
        ));
    };
    if target != path || write["value"]["node"].as_str() != Some(writer.as_str()) {
        return Err(invalid(
            "Select feedback written directly by its producing activity.",
        ));
    }
    Ok((writer, write))
}

fn executable(node: &Value) -> bool {
    matches!(node["kind"].as_str(), Some("step" | "verifier"))
}
fn can_continue(node: &Value) -> bool {
    match node["kind"].as_str() {
        Some("fail" | "succeed") => false,
        Some("seq") => child_values(node)
            .iter()
            .all(|(_, child)| can_continue(child)),
        Some("choice") => child_values(node)
            .iter()
            .any(|(_, child)| can_continue(child)),
        _ => true,
    }
}

fn producers(node: &Value, channel: Channel, path: &[String]) -> Result<Vec<Producer>, ApiError> {
    if executable(node) {
        let value_type = match channel {
            Channel::Out => selected_type(&node["output"], path)?,
            Channel::Diagnostic if node["kind"] == "verifier" => {
                selected_type(&node["diagnostic"], path)?
            }
            Channel::Signal if node["kind"] == "verifier" && path.len() == 1 => {
                let labels = node["signals"]
                    .get(&path[0])
                    .filter(|value| value.is_array())
                    .ok_or_else(|| invalid("The selected outcome no longer exists."))?;
                json!({"kind":"enum","values":labels})
            }
            _ => return Err(invalid("Select an output from this node.")),
        };
        return Ok(vec![Producer {
            name: node["name"]
                .as_str()
                .ok_or_else(|| invalid("Node not found."))?
                .into(),
            channel,
            path: path.to_vec(),
            value_type,
        }]);
    }
    if channel != Channel::Out {
        return Err(invalid("Select an output."));
    }
    let continuing: Vec<_> = child_values(node)
        .into_iter()
        .filter(|(_, child)| can_continue(child))
        .collect();
    if node["kind"] == "choice" {
        let mut result = Vec::new();
        for (_, branch) in continuing {
            result.extend(producers(branch, channel, path)?);
        }
        if result.is_empty() {
            return Err(invalid("This decision has no continuing output."));
        }
        let first = &result[0].value_type;
        if result.iter().any(|source| &source.value_type != first) {
            return Err(invalid("Decision outputs must have matching types."));
        }
        return Ok(result);
    }
    let mut candidates = continuing
        .into_iter()
        .filter_map(|(_, child)| producers(child, channel, path).ok())
        .collect::<Vec<_>>();
    if candidates.len() != 1 {
        return Err(invalid("Select an unambiguous producing node."));
    }
    Ok(candidates.remove(0))
}

fn connect_outputs(
    graph: &mut Value,
    index: &Index,
    consumer: &str,
    producers: &[Producer],
) -> Result<(Value, Value), ApiError> {
    let consumer_path = ancestry(index, consumer)?;
    let field = fresh_field(graph);
    let mut final_type = None;
    let mut plans = Vec::new();
    let producer_names = producers
        .iter()
        .map(|producer| producer.name.clone())
        .collect::<BTreeSet<_>>();
    for producer in producers {
        let source_path = ancestry(index, &producer.name)?;
        let shared = source_path
            .iter()
            .zip(&consumer_path)
            .take_while(|(a, b)| a == b)
            .count();
        if shared == 0 || shared == source_path.len() || shared == consumer_path.len() {
            return Err(invalid("Select an earlier output."));
        }
        let common = &source_path[shared - 1];
        if index[common].kind != "seq"
            || index[&source_path[shared]].order >= index[&consumer_path[shared]].order
        {
            return Err(invalid("Select an earlier output."));
        }
        let source_scopes = &source_path[shared..source_path.len() - 1];
        let value_type = produced_output_type(
            graph,
            index,
            OutputSource {
                scopes: source_scopes,
                producer_names: &producer_names,
            },
            producer.value_type.clone(),
        )?;
        if final_type
            .as_ref()
            .is_some_and(|value| value != &value_type)
        {
            return Err(invalid(
                "Selected outputs have different collection shapes.",
            ));
        }
        final_type = Some(value_type.clone());
        let mut scopes = std::iter::once(common.clone())
            .chain(source_scopes.iter().cloned())
            .chain(
                consumer_path[shared..consumer_path.len() - 1]
                    .iter()
                    .cloned(),
            )
            .collect::<BTreeSet<_>>();
        if matches!(
            index[consumer].kind.as_str(),
            "seq" | "par" | "map" | "loop" | "choice"
        ) {
            scopes.insert(consumer.into());
        }
        plans.push((producer.clone(), scopes, source_scopes.to_vec(), value_type));
    }
    for (producer, scopes, promotions, value_type) in plans {
        for scope in scopes {
            add_field(
                get_mut(graph, index, &scope)?,
                &field,
                json!({"type":value_type,"required":false}),
            )?;
        }
        let node = get_mut(graph, index, &producer.name)?;
        array_mut(node,"writeBindings")?.push(json!({"value":{"node":producer.name,"channel":producer.channel,"path":producer.path},"target":[field]}));
        for scope in promotions {
            let promoted = array_mut(get_mut(graph, index, &scope)?, "promotedStatePaths")?;
            let path = json!([field]);
            if !promoted.contains(&path) {
                promoted.push(path);
            }
        }
    }
    Ok((
        json!({"source":"state","path":[field]}),
        final_type.ok_or_else(|| invalid("Select an output."))?,
    ))
}

struct OutputSource<'a> {
    scopes: &'a [String],
    producer_names: &'a BTreeSet<String>,
}

fn produced_output_type(
    graph: &Value,
    index: &Index,
    source: OutputSource<'_>,
    mut value_type: Value,
) -> Result<Value, ApiError> {
    for scope in source.scopes.iter().rev() {
        let node = get(graph, index, scope)?;
        if node["kind"] == "par" && node["join"]["kind"] != "all" {
            return Err(invalid("Select an output from a completed group."));
        }
        if node["kind"] == "loop" && !guaranteed(&node["body"], source.producer_names) {
            return Err(invalid("This output is not produced in every round."));
        }
        if node["kind"] == "choice" && !guaranteed(node, source.producer_names) {
            return Err(invalid("Select a common decision output."));
        }
        if node["kind"] == "map" {
            value_type = json!({"kind":"array","items":value_type});
        }
    }
    Ok(value_type)
}

fn guaranteed(node: &Value, names: &BTreeSet<String>) -> bool {
    if executable(node) {
        return node["name"]
            .as_str()
            .is_some_and(|name| names.contains(name));
    }
    let branches = child_values(node)
        .into_iter()
        .filter(|(_, child)| can_continue(child))
        .collect::<Vec<_>>();
    if node["kind"] == "choice" {
        !branches.is_empty() && branches.iter().all(|(_, child)| guaranteed(child, names))
    } else {
        branches.iter().any(|(_, child)| guaranteed(child, names))
    }
}

fn add_field(node: &mut Value, name: &str, field: Value) -> Result<(), ApiError> {
    let fields = node["state"]["fields"]
        .as_object_mut()
        .ok_or_else(|| invalid("The group needs named data fields."))?;
    if let Some(existing) = fields.get(name) {
        if existing != &field {
            return Err(invalid("An existing field has a different type."));
        }
    }
    fields.insert(name.to_owned(), field);
    Ok(())
}
fn input_keys(node: &Value) -> Result<(&'static str, &'static str), ApiError> {
    if executable(node) {
        Ok(("input", "inputBindings"))
    } else if node["kind"] == "succeed" {
        Ok(("output", "bindings"))
    } else {
        Err(invalid("Select an agent input or run result."))
    }
}
fn set_input(graph: &mut Value, index: &Index, edit: InputEdit<'_>) -> Result<(), ApiError> {
    let InputEdit {
        node,
        input,
        value_type,
        selector,
    } = edit;
    let node = get_mut(graph, index, node)?;
    let (schema, bindings) = input_keys(node)?;
    if node[schema]["kind"] == "null" {
        node[schema] = json!({"kind":"record","fields":{}});
    }
    node[schema]["fields"]
        .as_object_mut()
        .ok_or_else(|| invalid("This input uses a scalar schema."))?
        .insert(input.into(), json!({"type":value_type,"required":true}));
    let bindings = array_mut(node, bindings)?;
    bindings.retain(|binding| {
        binding["target"]
            .as_array()
            .and_then(|path| path.first())
            .and_then(Value::as_str)
            != Some(input)
    });
    bindings.push(json!({"target":[input],"value":selector}));
    Ok(())
}
fn remove_input(graph: &mut Value, index: &Index, node: &str, input: &str) -> Result<(), ApiError> {
    let node = get_mut(graph, index, node)?;
    let (schema, bindings) = input_keys(node)?;
    node[schema]["fields"]
        .as_object_mut()
        .ok_or_else(|| invalid("This input uses a scalar schema."))?
        .remove(input);
    array_mut(node, bindings)?.retain(|binding| {
        binding["target"]
            .as_array()
            .and_then(|path| path.first())
            .and_then(Value::as_str)
            != Some(input)
    });
    Ok(())
}
fn array_mut<'a>(node: &'a mut Value, key: &str) -> Result<&'a mut Vec<Value>, ApiError> {
    if node.get(key).is_none() {
        node[key] = json!([]);
    }
    node[key]
        .as_array_mut()
        .ok_or_else(|| invalid("Invalid data mappings."))
}
fn fresh_field(graph: &Value) -> String {
    let text = graph.to_string();
    for number in 1.. {
        let field = format!("__ui_data_{number}");
        if !text.contains(&field) {
            return field;
        }
    }
    unreachable!()
}
fn state_path_written(graph: &Value, path: &[String]) -> bool {
    let Ok(index) = index(graph) else {
        return true;
    };
    index.values().any(|node| {
        graph
            .pointer(&node.pointer)
            .and_then(|node| node["writeBindings"].as_array())
            .is_some_and(|bindings| {
                bindings.iter().any(|binding| {
                    let Some(target) = binding["target"].as_array() else {
                        return true;
                    };
                    target.iter().zip(path).all(|(a, b)| a.as_str() == Some(b))
                })
            })
    })
}

fn edit_run_input(
    graph: &mut Value,
    index: &Index,
    edit: RunInputEdit<'_>,
) -> Result<(), ApiError> {
    let RunInputEdit {
        before,
        name,
        field,
    } = edit;
    let affected_maps = index
        .iter()
        .filter(|(_, node)| node.kind == "map")
        .filter_map(|(node, _)| {
            let map = get(graph, index, node).ok()?;
            (map["over"]["source"] == "state" && map["over"]["path"][0] == before.unwrap_or(name))
                .then(|| node.clone())
        })
        .collect::<Vec<_>>();
    let fields = graph["initialInput"]["fields"]
        .as_object()
        .ok_or_else(|| invalid("Run inputs need named fields."))?;
    let previous = before
        .map(|old| {
            fields
                .get(old)
                .cloned()
                .ok_or_else(|| invalid("Run input not found."))
        })
        .transpose()?;
    if before != Some(name) && fields.contains_key(name) {
        return Err(invalid("That input name already exists."));
    }
    if let Some(old) = before {
        if state_path_written(graph, &[old.into()]) {
            return Err(invalid("This input is also authored as writable state."));
        }
    }
    let mut linked = BTreeSet::new();
    for (node, info) in index {
        let value = graph
            .pointer(&info.pointer)
            .ok_or_else(|| invalid("Node not found."))?;
        if info.parent.is_none()
            || before.is_some_and(|old| previous.as_ref() == value["state"]["fields"].get(old))
        {
            linked.insert(node.clone());
        }
    }
    let fields = graph["initialInput"]["fields"]
        .as_object_mut()
        .ok_or_else(|| invalid("Run inputs need named fields."))?;
    if let Some(old) = before {
        fields.remove(old);
    }
    if let Some(field) = &field {
        fields.insert(name.into(), field.clone());
    }
    for node in &linked {
        if let Some(fields) = get_mut(graph, index, node)?
            .get_mut("state")
            .and_then(|schema| schema.get_mut("fields"))
            .and_then(Value::as_object_mut)
        {
            if let Some(old) = before {
                fields.remove(old);
            }
            if let Some(field) = &field {
                fields.insert(name.into(), field.clone());
            }
        }
    }
    if let Some(old) = before {
        rewrite_input_references(
            graph,
            index,
            InputRewrite {
                old,
                name,
                field: field.as_ref(),
                linked: &linked,
            },
        )?;
    }
    for name in affected_maps {
        let map = get_mut(graph, index, &name)?;
        let mut value_type = &map["state"];
        for part in map["over"]["path"].as_array().into_iter().flatten() {
            value_type = &value_type["fields"][part.as_str().unwrap_or("")]["type"];
        }
        let items = if map["over"]["source"] == "state" && value_type["kind"] == "array" {
            value_type["items"].clone()
        } else {
            map["over"] = Value::Null;
            Value::Null
        };
        refresh_map_inputs(graph, index, &name, &items)?;
    }
    Ok(())
}

struct InputRewrite<'a> {
    old: &'a str,
    name: &'a str,
    field: Option<&'a Value>,
    linked: &'a BTreeSet<String>,
}

fn rewrite_input_references(
    graph: &mut Value,
    index: &Index,
    rewrite: InputRewrite<'_>,
) -> Result<(), ApiError> {
    let InputRewrite {
        old,
        name,
        field,
        linked,
    } = rewrite;
    for (node, info) in index {
        let owner = info.parent.as_ref();
        if !owner.is_some_and(|owner| linked.contains(owner)) && !linked.contains(node) {
            continue;
        }
        let value = get_mut(graph, index, node)?;
        for key in ["inputBindings", "bindings"] {
            let Some(bindings) = value.get_mut(key).and_then(Value::as_array_mut) else {
                continue;
            };
            let mut changes = Vec::new();
            bindings.retain_mut(|binding| {
                if binding["value"]["source"] != "state" || binding["value"]["path"][0] != old {
                    return true;
                }
                changes.push((binding["target"].clone(), binding["value"]["path"].clone()));
                if field.is_some() {
                    binding["value"]["path"][0] = json!(name);
                    true
                } else {
                    false
                }
            });
            let schema = if key == "bindings" { "output" } else { "input" };
            rewrite_connected_fields(value, schema, changes, field)?;
        }
        if value["over"]["source"] == "state" && value["over"]["path"][0] == old {
            if field.is_some() {
                value["over"]["path"][0] = json!(name);
            } else {
                value["over"] = Value::Null;
            }
        }
        if let Some(promotions) = value
            .get_mut("promotedStatePaths")
            .and_then(Value::as_array_mut)
        {
            promotions.retain_mut(|path| {
                if path[0] != old {
                    return true;
                }
                if field.is_some() {
                    path[0] = json!(name);
                    true
                } else {
                    false
                }
            });
        }
    }
    Ok(())
}

fn rewrite_connected_fields(
    value: &mut Value,
    schema: &str,
    changes: Vec<(Value, Value)>,
    field: Option<&Value>,
) -> Result<(), ApiError> {
    for (target, source_path) in changes {
        if let Some(input) = target
            .as_array()
            .filter(|path| path.len() == 1)
            .and_then(|path| path[0].as_str())
        {
            if let Some(fields) = value
                .get_mut(schema)
                .and_then(|schema| schema.get_mut("fields"))
                .and_then(Value::as_object_mut)
            {
                if let Some(field) = field {
                    let mut selected = &field["type"];
                    for part in source_path.as_array().into_iter().flatten().skip(1) {
                        let part = part
                            .as_str()
                            .ok_or_else(|| invalid("Invalid connected input."))?;
                        selected = selected["fields"]
                            .get(part)
                            .map(|field| &field["type"])
                            .ok_or_else(|| invalid("A connected field no longer exists."))?;
                    }
                    fields.insert(input.into(), json!({"type":selected,"required":true}));
                } else {
                    fields.remove(input);
                }
            }
        }
    }
    Ok(())
}

fn refresh_map_inputs(
    graph: &mut Value,
    index: &Index,
    map: &str,
    items: &Value,
) -> Result<(), ApiError> {
    for (name, node) in index {
        if !matches!(node.kind.as_str(), "step" | "verifier" | "succeed") {
            continue;
        }
        let ancestry = ancestry(index, name)?;
        if ancestry
            .iter()
            .rev()
            .skip(1)
            .find(|name| index[*name].kind == "map")
            .map(String::as_str)
            != Some(map)
        {
            continue;
        }
        let target = get_mut(graph, index, name)?;
        let (schema, key) = input_keys(target)?;
        let updates = target[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|binding| {
                if binding["value"]["source"] != "item" {
                    return None;
                }
                let target = binding["target"]
                    .as_array()
                    .filter(|path| path.len() == 1)?[0]
                    .as_str()?
                    .to_owned();
                let path = binding["value"]["path"]
                    .as_array()?
                    .iter()
                    .map(|part| part.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()?;
                Some((target, path))
            })
            .collect::<Vec<_>>();
        for (input, path) in updates {
            // A source replacement never guesses a renamed field. Keep the input row for an
            // explicit replacement, and clear its invalid mapping when the selected field is gone.
            if let Ok(value_type) = selected_type(items, &path) {
                if let Some(fields) = target
                    .get_mut(schema)
                    .and_then(|schema| schema.get_mut("fields"))
                    .and_then(Value::as_object_mut)
                {
                    fields.insert(input, json!({"type":value_type,"required":true}));
                }
            } else if let Some(bindings) = target.get_mut(key).and_then(Value::as_array_mut) {
                bindings.retain(|binding| {
                    binding["value"]["source"] != "item"
                        || binding["target"] != json!([input])
                        || binding["value"]["path"] != json!(path)
                });
            }
        }
    }
    Ok(())
}

fn invalid(message: &str) -> ApiError {
    ApiError::invalid(message.into())
}
fn value_error(error: impl std::fmt::Display) -> ApiError {
    invalid(&error.to_string())
}

#[cfg(test)]
#[path = "data_tests.rs"]
mod tests;
