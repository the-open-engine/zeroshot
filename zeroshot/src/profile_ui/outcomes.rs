//! Explicit editor operations lower to ordinary graph nodes. No policies or editor metadata
//! cross into the protocol, and these operations never persist or execute a profile.

use std::collections::BTreeSet;

use openengine_cluster_protocol::{
    ChoiceBranch, ChoiceNode, ControlSelector, ControlSource, EnumLabel, FailNode, FailReason,
    FieldName, GraphNode, GraphSpec, Guard, Join, NodeName, NonEmptyEnumSet, NonEmptyVec,
    PayloadType, PositiveInteger, SeqNode, SucceedNode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::WorkspaceError as ApiError;

#[path = "outcomes/required.rs"]
mod required;
pub(super) use required::ensure_required_output;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Request {
    graph: GraphSpec,
    // Runtime fields may still be blank in an editor draft. This operation does not interpret them;
    // normal profile admission remains the authority when the draft is saved.
    runtime: Value,
    action: Action,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
enum Action {
    FailureReason {
        terminal: NodeName,
        reason: FailReason,
    },
    Complete {
        owner: NodeName,
    },
    Protect {
        node: NodeName,
    },
}

#[derive(Serialize)]
pub(super) struct Document {
    graph: GraphSpec,
    runtime: Value,
}

pub(super) fn apply(request: Request) -> Result<Document, ApiError> {
    let Request {
        mut graph,
        runtime,
        action,
    } = request;
    let mut names = reserved_names(&graph, &runtime)?;
    let target = match &action {
        Action::FailureReason { terminal, .. } => terminal,
        Action::Complete { owner } => owner,
        Action::Protect { node } => node,
    };
    if named_nodes(&graph.root, target) != 1 {
        return Err(invalid("Select one uniquely named existing node."));
    }
    match action {
        Action::FailureReason { terminal, reason } => {
            edit(&mut graph.root, &mut |node| {
                if node.name() == &terminal {
                    let GraphNode::Fail(failure) = node else {
                        return Err(invalid("Failure reasons belong to failure outcomes."));
                    };
                    failure.reason = reason.clone();
                }
                Ok(())
            })?;
        }
        Action::Complete { owner } => {
            if inside_parallel_or_map(&graph.root, &owner, false) {
                return Err(invalid(
                    "Run completion cannot be added inside a parallel branch or map item.",
                ));
            }
            let name = fresh_name(&mut names, "completed")?;
            edit(&mut graph.root, &mut |node| {
                if node.name() == &owner {
                    if has_terminal(node) {
                        return Err(invalid(
                            "This scope already contains a run outcome; edit that outcome instead.",
                        ));
                    }
                    let GraphNode::Seq(sequence) = node else {
                        return Err(invalid(
                            "Ordinary completion can be added only to a sequence.",
                        ));
                    };
                    let mut children = sequence.children.clone().into_vec();
                    children.push(GraphNode::Succeed(SucceedNode {
                        name: name.clone(),
                        output: PayloadType::Null,
                        bindings: Vec::new(),
                    }));
                    sequence.children = non_empty(children)?;
                }
                Ok(())
            })?;
        }
        Action::Protect { node } => protect(&mut graph.root, &node, &mut names)?,
    }
    Ok(Document { graph, runtime })
}

fn protect(
    root: &mut GraphNode,
    target: &NodeName,
    names: &mut BTreeSet<String>,
) -> Result<(), ApiError> {
    protect_with_reason(root, target, names, None)
}

fn protect_with_reason(
    root: &mut GraphNode,
    target: &NodeName,
    names: &mut BTreeSet<String>,
    failure_reason: Option<FailReason>,
) -> Result<(), ApiError> {
    if inside_parallel_or_map(root, target, false) {
        return Err(invalid(
            "Handle errors after the parallel or map group; a branch must not finish the whole run.",
        ));
    }
    let choice_name = fresh_name(names, "on_error")?;
    let failure_name = fresh_name(names, "worker_failed")?;
    let continuation_name = fresh_name(names, "continuation")?;
    let mut found = false;
    edit(root, &mut |node| {
        let GraphNode::Seq(sequence) = node else {
            return Ok(());
        };
        let Some(index) = sequence
            .children
            .as_slice()
            .iter()
            .position(|child| child.name() == target)
        else {
            return Ok(());
        };
        let protected = &sequence.children.as_slice()[index];
        let guard = protected_errors(protected)?;
        let mut children = sequence.children.clone().into_vec();
        let mut suffix = children.split_off(index + 1);
        if suffix.is_empty() {
            return Err(invalid(
                "Add the next step or run result before configuring failure handling.",
            ));
        }
        if let Some(GraphNode::Loop(group)) = children.last_mut() {
            // Stop after the first failed round, before a later round can replace its error
            // control or a stale result can reach the continuation. Exhaustion of ordinary
            // fixed-count repetition is still successful completion of the requested work.
            group.until = Some(loop_error_exit(group.until.as_ref(), &guard)?);
        }
        let mut watched = BTreeSet::new();
        collect_guard_names(&guard, &mut watched);
        if refresh_standard_guard(&mut suffix, &guard, &watched)? {
            children.extend(suffix);
            sequence.children = non_empty(children)?;
            found = true;
            return Ok(());
        }
        if suffix
            .iter()
            .any(|node| watched.iter().any(|name| references_guard(node, name)))
        {
            return Err(invalid(
                "The following path already handles this worker's outcomes; edit its existing handling.",
            ));
        }
        // The entire suffix moves together. Carry the original owner's exposed writes through
        // both new scopes; success bindings within the suffix continue to see the same state.
        let otherwise = GraphNode::Seq(SeqNode {
            name: continuation_name.clone(),
            state: sequence.state.clone(),
            children: non_empty(suffix)?,
            promoted_state_paths: sequence.promoted_state_paths.clone(),
        });
        let failure = GraphNode::Fail(FailNode {
            name: failure_name.clone(),
            reason: match &failure_reason {
                Some(reason) => reason.clone(),
                None => FailReason::new(EnumLabel::new("execution_failed").map_err(value_error)?)
                    .map_err(value_error)?,
            },
        });
        children.push(GraphNode::Choice(ChoiceNode {
            name: choice_name.clone(),
            state: sequence.state.clone(),
            branches: non_empty(vec![ChoiceBranch {
                when: guard,
                node: failure,
            }])?,
            otherwise: Some(Box::new(otherwise)),
            promoted_state_paths: sequence.promoted_state_paths.clone(),
        }));
        sequence.children = non_empty(children)?;
        found = true;
        Ok(())
    })?;
    if !found {
        return Err(invalid("Choose an agent or worker directly in a sequence."));
    }
    Ok(())
}

fn refresh_standard_guard(
    suffix: &mut [GraphNode],
    guard: &Guard,
    watched: &BTreeSet<NodeName>,
) -> Result<bool, ApiError> {
    let [GraphNode::Choice(choice)] = suffix else {
        return Ok(false);
    };
    let [branch] = choice.branches.as_slice() else {
        return Ok(false);
    };
    if !matches!(branch.node, GraphNode::Fail(_)) || choice.otherwise.is_none() {
        return Ok(false);
    }
    if &branch.when == guard {
        return Ok(true);
    }
    if !compatible_standard_policy(&branch.when, guard) {
        return Ok(false);
    }
    if choice
        .otherwise
        .as_deref()
        .is_some_and(|node| watched.iter().any(|name| references_guard(node, name)))
    {
        return Err(invalid(
            "The continuation has custom handling for these workers; keep its existing outcome handling.",
        ));
    }
    let mut branch = branch.clone();
    branch.when = guard.clone();
    choice.branches = non_empty(vec![branch])?;
    Ok(true)
}

type ErrorAtoms = BTreeSet<(NodeName, &'static str)>;
struct PolicyTerm {
    conditions: Vec<Guard>,
    errors: ErrorAtoms,
}

fn policy_terms(guard: &Guard) -> Option<Vec<PolicyTerm>> {
    let mut errors = BTreeSet::new();
    if standard_atoms(guard, &mut errors) {
        return Some(vec![PolicyTerm {
            conditions: Vec::new(),
            errors,
        }]);
    }
    match guard {
        Guard::Any { guards } => {
            let mut terms = Vec::new();
            for guard in guards.as_slice() {
                terms.extend(policy_terms(guard)?);
            }
            Some(terms)
        }
        Guard::All { guards } => {
            let (last, conditions) = guards.as_slice().split_last()?;
            let mut errors = BTreeSet::new();
            standard_atoms(last, &mut errors).then(|| {
                vec![PolicyTerm {
                    conditions: conditions.to_vec(),
                    errors,
                }]
            })
        }
        _ => None,
    }
}

fn compatible_standard_policy(existing: &Guard, expected: &Guard) -> bool {
    let (Some(existing), Some(expected)) = (policy_terms(existing), policy_terms(expected)) else {
        return false;
    };
    existing.iter().all(|old| {
        expected.iter().any(|new| {
            new.conditions.starts_with(&old.conditions) && old.errors.is_subset(&new.errors)
        })
    })
}

fn standard_atoms(guard: &Guard, names: &mut BTreeSet<(NodeName, &'static str)>) -> bool {
    match guard {
        Guard::Any { guards } => guards
            .as_slice()
            .iter()
            .all(|guard| standard_atoms(guard, names)),
        Guard::In { value, labels } | Guard::KOfMap { value, labels, .. }
            if value.source == ControlSource::Error
                && value.field.is_none()
                && labels
                    .values()
                    .iter()
                    .map(|label| label.as_str())
                    .collect::<BTreeSet<_>>()
                    == BTreeSet::from(["timeout", "crash", "malformed", "refusal"])
                && !matches!(guard, Guard::KOfMap { count, .. } if count.get() != 1) =>
        {
            names.insert((value.name.clone(), "error"));
            true
        }
        Guard::In { value, labels }
            if value.source == ControlSource::Group
                && value
                    .field
                    .as_ref()
                    .is_some_and(|field| field.as_str() == "overflow")
                && labels.values().len() == 1
                && labels.values()[0].as_str() == "overflow" =>
        {
            names.insert((value.name.clone(), "overflow"));
            true
        }
        _ => false,
    }
}

fn protected_errors(node: &GraphNode) -> Result<Guard, ApiError> {
    if !matches!(
        node,
        GraphNode::Step(_)
            | GraphNode::Verifier(_)
            | GraphNode::Par(_)
            | GraphNode::Map(_)
            | GraphNode::Loop(_)
            | GraphNode::Choice(_)
    ) {
        return Err(invalid(
            "Choose an agent, worker, join-all parallel group, map, or simple loop directly in a sequence.",
        ));
    }
    if let GraphNode::Choice(choice) = node {
        return choice_errors(choice);
    }
    let mut guards = Vec::new();
    if let GraphNode::Map(map) = node {
        guards.push(Guard::In {
            value: ControlSelector {
                name: map.name.clone(),
                source: ControlSource::Group,
                field: Some(FieldName::new("overflow").map_err(value_error)?),
            },
            labels: NonEmptyEnumSet::new(vec![EnumLabel::new("overflow").map_err(value_error)?])
                .map_err(value_error)?,
        });
        collect_errors(&map.body, true, &mut guards)?;
    } else if let GraphNode::Loop(group) = node {
        collect_errors(&group.body, false, &mut guards)?;
    } else {
        collect_errors(node, false, &mut guards)?;
    }
    if guards.len() == 1 {
        return guards
            .pop()
            .ok_or_else(|| invalid("No executable error source was found."));
    }
    Ok(Guard::Any {
        guards: non_empty(guards)?,
    })
}

fn choice_errors(choice: &ChoiceNode) -> Result<Guard, ApiError> {
    let mut prior = Vec::new();
    let mut terms = Vec::new();
    for branch in choice.branches.as_slice() {
        let mut conditions = prior.clone();
        conditions.push(branch.when.clone());
        if can_continue(&branch.node) {
            terms.push(choice_branch_errors(&branch.node, conditions)?);
        }
        prior.push(Guard::Not {
            guard: Box::new(branch.when.clone()),
        });
    }
    if let Some(otherwise) = choice
        .otherwise
        .as_deref()
        .filter(|node| can_continue(node))
    {
        terms.push(choice_branch_errors(otherwise, prior)?);
    }
    if terms.len() == 1 {
        return terms
            .pop()
            .ok_or_else(|| invalid("No continuing error source was found."));
    }
    Ok(Guard::Any {
        guards: non_empty(terms)?,
    })
}

fn choice_branch_errors(node: &GraphNode, mut conditions: Vec<Guard>) -> Result<Guard, ApiError> {
    let mut errors = Vec::new();
    collect_errors(node, false, &mut errors)?;
    let errors = if errors.len() == 1 {
        errors
            .pop()
            .ok_or_else(|| invalid("No error source was found."))?
    } else {
        Guard::Any {
            guards: non_empty(errors)?,
        }
    };
    conditions.push(errors);
    Ok(Guard::All {
        guards: non_empty(conditions)?,
    })
}

fn can_continue(node: &GraphNode) -> bool {
    match node {
        GraphNode::Fail(_) | GraphNode::Succeed(_) => false,
        GraphNode::Seq(node) => node.children.as_slice().iter().all(can_continue),
        GraphNode::Choice(node) => {
            node.branches
                .as_slice()
                .iter()
                .any(|branch| can_continue(&branch.node))
                || node.otherwise.as_deref().is_some_and(can_continue)
        }
        _ => true,
    }
}

fn loop_error_exit(until: Option<&Guard>, errors: &Guard) -> Result<Guard, ApiError> {
    let mut guards = Vec::new();
    if let Some(until) = until {
        retain_authored_loop_exits(until, &mut guards);
    }
    match errors {
        Guard::Any {
            guards: body_errors,
        } => guards.extend(body_errors.clone().into_vec()),
        _ => guards.push(errors.clone()),
    }
    if guards.len() == 1 {
        return guards
            .pop()
            .ok_or_else(|| invalid("No executable error source was found."));
    }
    Ok(Guard::Any {
        guards: non_empty(guards)?,
    })
}

fn retain_authored_loop_exits(guard: &Guard, retained: &mut Vec<Guard>) {
    match guard {
        Guard::Any { guards } => {
            for guard in guards.as_slice() {
                retain_authored_loop_exits(guard, retained);
            }
        }
        Guard::In { value, .. }
            if value.source == ControlSource::Error
                && standard_atoms(guard, &mut BTreeSet::new()) => {}
        // Preserve composite predicates intact: an error within an authored All/Not is not
        // the independent stop-on-any-error condition inserted by this operation.
        _ => retained.push(guard.clone()),
    }
}

fn collect_errors(node: &GraphNode, mapped: bool, guards: &mut Vec<Guard>) -> Result<(), ApiError> {
    match node {
        GraphNode::Step(_) | GraphNode::Verifier(_) => {
            guards.push(worker_errors(node.name(), mapped)?)
        }
        GraphNode::Seq(node) => {
            for child in node.children.as_slice() {
                collect_errors(child, mapped, guards)?;
            }
        }
        GraphNode::Par(node) if matches!(node.join, Join::All {}) => {
            for child in node.branches.as_slice() {
                collect_errors(child, mapped, guards)?;
            }
        }
        GraphNode::Par(_) => {
            return Err(invalid(
                "Default failure handling supports join-all parallel groups. Keep custom handling for other joins.",
            ));
        }
        _ => {
            return Err(invalid(
                "This group has conditional, repeated, or terminal paths; configure its existing outcome handling explicitly.",
            ));
        }
    }
    Ok(())
}

fn worker_errors(name: &NodeName, mapped: bool) -> Result<Guard, ApiError> {
    let labels = ["timeout", "crash", "malformed", "refusal"]
        .into_iter()
        .map(|label| EnumLabel::new(label).map_err(value_error))
        .collect::<Result<Vec<_>, _>>()?;
    let value = ControlSelector {
        name: name.clone(),
        source: ControlSource::Error,
        field: None,
    };
    let labels = NonEmptyEnumSet::new(labels).map_err(value_error)?;
    Ok(if mapped {
        Guard::KOfMap {
            count: PositiveInteger::new(1).map_err(value_error)?,
            value,
            labels,
        }
    } else {
        Guard::In { value, labels }
    })
}

fn collect_guard_names(guard: &Guard, names: &mut BTreeSet<NodeName>) {
    match guard {
        Guard::In { value, .. } | Guard::KOfMap { value, .. } => {
            names.insert(value.name.clone());
        }
        Guard::All { guards } | Guard::Any { guards } => {
            for guard in guards.as_slice() {
                collect_guard_names(guard, names);
            }
        }
        Guard::Not { guard } => collect_guard_names(guard, names),
        Guard::KOfN { values, .. } => {
            names.extend(values.as_slice().iter().map(|value| value.name.clone()))
        }
    }
}

fn children(node: &GraphNode) -> Vec<&GraphNode> {
    match node {
        GraphNode::Seq(node) => node.children.as_slice().iter().collect(),
        GraphNode::Par(node) => node.branches.as_slice().iter().collect(),
        GraphNode::Choice(node) => node
            .branches
            .as_slice()
            .iter()
            .map(|branch| &branch.node)
            .chain(node.otherwise.as_deref())
            .collect(),
        GraphNode::Loop(node) => vec![&node.body],
        GraphNode::Map(node) => vec![&node.body],
        _ => Vec::new(),
    }
}

fn named_nodes(node: &GraphNode, target: &NodeName) -> usize {
    usize::from(node.name() == target)
        + children(node)
            .into_iter()
            .map(|child| named_nodes(child, target))
            .sum::<usize>()
}

fn has_terminal(node: &GraphNode) -> bool {
    matches!(node, GraphNode::Succeed(_) | GraphNode::Fail(_))
        || children(node).into_iter().any(has_terminal)
}

fn inside_parallel_or_map(node: &GraphNode, target: &NodeName, enclosed: bool) -> bool {
    if node.name() == target {
        return enclosed;
    }
    let enclosed = enclosed || matches!(node, GraphNode::Par(_) | GraphNode::Map(_));
    children(node)
        .into_iter()
        .any(|child| inside_parallel_or_map(child, target, enclosed))
}

fn references_guard(node: &GraphNode, target: &NodeName) -> bool {
    let own = match node {
        GraphNode::Choice(node) => node
            .branches
            .as_slice()
            .iter()
            .any(|branch| guard_references(&branch.when, target)),
        GraphNode::Loop(node) => node
            .until
            .as_ref()
            .is_some_and(|guard| guard_references(guard, target)),
        GraphNode::Par(openengine_cluster_protocol::ParNode {
            join: openengine_cluster_protocol::Join::First { when },
            ..
        }) => guard_references(when, target),
        _ => false,
    };
    own || children(node)
        .into_iter()
        .any(|child| references_guard(child, target))
}

fn guard_references(guard: &Guard, target: &NodeName) -> bool {
    match guard {
        Guard::In { value, .. } | Guard::KOfMap { value, .. } => &value.name == target,
        Guard::All { guards } | Guard::Any { guards } => guards
            .as_slice()
            .iter()
            .any(|guard| guard_references(guard, target)),
        Guard::Not { guard } => guard_references(guard, target),
        Guard::KOfN { values, .. } => values.as_slice().iter().any(|value| &value.name == target),
    }
}

fn edit(
    node: &mut GraphNode,
    operation: &mut impl FnMut(&mut GraphNode) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    operation(node)?;
    match node {
        GraphNode::Seq(node) => node.children = edit_list(&node.children, operation)?,
        GraphNode::Par(node) => node.branches = edit_list(&node.branches, operation)?,
        GraphNode::Choice(node) => {
            let mut branches = node.branches.clone().into_vec();
            for branch in &mut branches {
                edit(&mut branch.node, operation)?;
            }
            node.branches = non_empty(branches)?;
            if let Some(otherwise) = &mut node.otherwise {
                edit(otherwise, operation)?;
            }
        }
        GraphNode::Loop(node) => edit(&mut node.body, operation)?,
        GraphNode::Map(node) => edit(&mut node.body, operation)?,
        _ => {}
    }
    Ok(())
}

fn edit_list(
    nodes: &NonEmptyVec<GraphNode>,
    operation: &mut impl FnMut(&mut GraphNode) -> Result<(), ApiError>,
) -> Result<NonEmptyVec<GraphNode>, ApiError> {
    let mut nodes = nodes.clone().into_vec();
    for node in &mut nodes {
        edit(node, operation)?;
    }
    non_empty(nodes)
}

fn reserved_names(graph: &GraphSpec, runtime: &Value) -> Result<BTreeSet<String>, ApiError> {
    let mut names = BTreeSet::new();
    // Invalid drafts can still reference a removed node. Never accidentally repair such a
    // reference by assigning its identity to a generated outcome or control node.
    reserve_references(
        &serde_json::to_value(graph).map_err(value_error)?,
        &mut names,
    );
    if let Some(nodes) = runtime.get("nodes").and_then(Value::as_object) {
        names.extend(nodes.keys().cloned());
    }
    Ok(names)
}

fn reserve_references(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(key.as_str(), "name" | "node") {
                    if let Some(name) = value.as_str() {
                        names.insert(name.to_owned());
                    }
                }
                reserve_references(value, names);
            }
        }
        Value::Array(values) => {
            for value in values {
                reserve_references(value, names);
            }
        }
        _ => {}
    }
}

fn fresh_name(names: &mut BTreeSet<String>, stem: &str) -> Result<NodeName, ApiError> {
    for index in 0..=names.len() {
        let name = if index == 0 {
            stem.to_owned()
        } else {
            format!("{stem}_{index}")
        };
        if names.insert(name.clone()) {
            return NodeName::new(name).map_err(value_error);
        }
    }
    Err(invalid("Unable to allocate a unique outcome name."))
}

fn non_empty<T>(values: Vec<T>) -> Result<NonEmptyVec<T>, ApiError> {
    NonEmptyVec::new(values).map_err(value_error)
}

fn value_error(error: impl std::fmt::Display) -> ApiError {
    invalid(&error.to_string())
}
fn invalid(message: &str) -> ApiError {
    ApiError::invalid(message.to_owned())
}

#[cfg(test)]
#[path = "outcomes/tests.rs"]
mod tests;
