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
mod tests {
    use super::*;
    use async_trait::async_trait;
    use openengine_cluster_protocol::{WorkerDescriptor, WorkerErrorCode, WorkerOutcome, WorkerRef};
    use openengine_cluster_server::admission::{GraphVerifier, VerifiedGraph};
    use openengine_cluster_server::graph_verifier::ProductionGraphVerifier;
    use openengine_cluster_server::worker_registry::{WorkerRegistry, WorkerRegistryError};
    use openengine_cluster_testkit::assertions::AssertValue;
    use serde_json::json;
    use crate::full_v1_reducer::{
        Decision, DurableExecution, DurableExecutionState, ExecutionId, FullV1Reducer,
        HistoryPosition, NodeInstanceId, Reduction, ReductionInput, StructuralOccurrence,
        TerminalProjection,
    };

    fn worker(name: &str) -> Value {
        json!({"kind":"step","name":name,"worker":format!("{name}@1"),"instructions":"Write a useful file.","input":{"kind":"null"},"output":{"kind":"null"},"inputBindings":[],"writeBindings":[],"attempts":1})
    }
    fn done() -> Value {
        json!({"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]})
    }
    fn sequence(name: &str, children: Vec<Value>) -> Value {
        json!({"kind":"seq","name":name,"state":{"kind":"record","fields":{}},"children":children,"promotedStatePaths":[]})
    }
    fn graph(children: Vec<Value>) -> Value {
        json!({"profile":"openengine.graph.full/v1","initialInput":{"kind":"record","fields":{}},"policy":{"policy":"policy.native-v2@1","default":"deny"},"root":sequence("run",children)})
    }
    fn transform(graph: Value, action: Value) -> Result<Value, String> {
        let request = serde_json::from_value(
            json!({"graph":graph,"runtime":{"harness":"","nodes":{}},"action":action}),
        )
        .map_err(|error| error.to_string())?;
        let document = apply(request).map_err(|error| error.message)?;
        serde_json::to_value(document).map_err(|error| error.to_string())
    }
    fn parallel(branches: Vec<Value>) -> Value {
        json!({"kind":"par","name":"plans","state":{"kind":"record","fields":{}},"branches":branches,"promotedStatePaths":[],"join":{"kind":"all"}})
    }

    fn repeat(body: Value) -> Value {
        json!({"kind":"loop","name":"repeat","state":{"kind":"record","fields":{}},"body":body,"maxIterations":3,"promotedStatePaths":[]})
    }

    #[test]
    fn loop_protection_refreshes_error_exits_without_duplicate_guards_or_exhaustion_failure() {
        let source = graph(vec![repeat(worker("write")), worker("publish"), done()]);
        let action = json!({"kind":"protect","node":"repeat"});
        let first = transform(source.clone(), action.clone()).assert_value()["graph"].clone();
        let until = &first["root"]["children"][0]["until"];
        assert_eq!(until["value"]["name"], "write");
        assert_eq!(
            first["root"]["children"][1]["branches"]
                .as_array()
                .assert_value()
                .len(),
            1
        );
        assert_eq!(first["root"]["children"][1]["branches"][0]["when"], *until);
        assert_eq!(
            first["root"]["children"][1]["otherwise"]["children"],
            json!([source["root"]["children"][1], source["root"]["children"][2]])
        );
        assert_eq!(
            transform(first.clone(), action.clone()).assert_value()["graph"],
            first
        );

        let mut expanded = first;
        expanded["root"]["children"][0]["body"] =
            sequence("round", vec![worker("write"), verifier("review")]);
        let expanded = transform(expanded, action.clone()).assert_value()["graph"].clone();
        let guards = expanded["root"]["children"][0]["until"]["guards"]
            .as_array()
            .assert_value();
        assert_eq!(guards.len(), 2);
        assert_eq!(guards[0]["value"]["name"], "write");
        assert_eq!(guards[1]["value"]["name"], "review");
        assert_eq!(
            expanded["root"]["children"][1]["branches"][0]["when"],
            expanded["root"]["children"][0]["until"]
        );
        assert_eq!(
            transform(expanded.clone(), action).assert_value()["graph"],
            expanded
        );
    }

    #[test]
    fn loop_protection_keeps_authored_verdict_condition_and_requires_a_guaranteed_body() {
        let condition = json!({"kind":"in","value":{"name":"review","source":"signal","field":"verdict"},"labels":["accepted"]});
        let mut repeated = repeat(sequence("round", vec![worker("write"), verifier("review")]));
        repeated["until"] = condition.clone();
        let action = json!({"kind":"protect","node":"repeat"});
        let result =
            transform(graph(vec![repeated, done()]), action.clone()).assert_value()["graph"]
                .clone();
        let guards = result["root"]["children"][0]["until"]["guards"]
            .as_array()
            .assert_value();
        assert_eq!(guards.len(), 3);
        assert_eq!(guards[0], condition);
        assert_eq!(
            transform(result.clone(), action.clone()).assert_value()["graph"],
            result
        );
        for body in [
            done(),
            repeat(worker("write")),
            json!({"kind":"choice","name":"route","state":{"kind":"record","fields":{}},"branches":[{"when":condition,"node":worker("write")}],"otherwise":worker("other"),"promotedStatePaths":[]}),
            {
                let mut group = parallel(vec![verifier("left"), verifier("right")]);
                group["join"] = json!({"kind":"any"});
                group
            },
        ] {
            assert!(transform(graph(vec![repeat(body), done()]), action.clone()).is_err());
        }
        assert!(transform(graph(vec![repeat(worker("write"))]), action).is_err());
    }

    #[test]
    fn completion_reserves_dangling_references_and_incomplete_runtime_names() {
        let mut source = graph(vec![worker("write")]);
        source["root"]["children"][0]["writeBindings"] = json!([{"target":["missing"],"value":{"node":"completed","channel":"out","path":["missing"]}}]);
        let runtime =
            json!({"harness":"","provider":"","nodes":{"completed_1":{"kind":"agent","model":""}}});
        let request = serde_json::from_value(
            json!({"graph":source,"runtime":runtime,"action":{"kind":"complete","owner":"run"}}),
        )
        .assert_value();
        let result =
            serde_json::to_value(apply(request).map_err(|error| error.message).assert_value())
                .assert_value();
        assert_eq!(result["runtime"], runtime);
        assert_eq!(
            result["graph"]["root"]["children"][1]["name"],
            "completed_2"
        );
        assert_eq!(
            result["graph"]["root"]["children"][0],
            source["root"]["children"][0]
        );
    }

    #[test]
    fn completion_rejects_existing_terminal_and_non_sequence_and_ambiguous_names() {
        for (source, owner) in [
            (graph(vec![worker("write"), done()]), "run"),
            (graph(vec![worker("write")]), "write"),
            (graph(vec![worker("write"), worker("write")]), "write"),
            (
                graph(vec![parallel(vec![sequence(
                    "branch",
                    vec![worker("write")],
                )])]),
                "branch",
            ),
        ] {
            assert!(transform(source, json!({"kind":"complete","owner":owner})).is_err());
        }
    }

    #[test]
    fn failure_reason_changes_only_exact_failure_and_native_type_rejects_reserved_reasons() {
        let source = graph(vec![
            json!({"kind":"fail","name":"failed","reason":"old_reason"}),
        ]);
        let result = transform(
            source.clone(),
            json!({"kind":"failure_reason","terminal":"failed","reason":"budget_exhausted"}),
        )
        .assert_value();
        let mut expected = source.clone();
        expected["root"]["children"][0]["reason"] = json!("budget_exhausted");
        assert_eq!(result["graph"], expected);
        for reason in [
            "unhandled",
            "runtime_failed",
            "runtime_lost",
            "invalid reason",
        ] {
            assert!(
                transform(
                    source.clone(),
                    json!({"kind":"failure_reason","terminal":"failed","reason":reason})
                )
                .is_err()
            );
        }
        assert!(
            transform(
                graph(vec![worker("write")]),
                json!({"kind":"failure_reason","terminal":"write","reason":"failed"})
            )
            .is_err()
        );
    }

    #[test]
    fn serial_protection_preserves_entire_continuation_and_is_idempotent() {
        let source = graph(vec![worker("write"), worker("review"), done()]);
        let action = json!({"kind":"protect","node":"write"});
        let result = transform(source.clone(), action.clone()).assert_value();
        let root = &result["graph"]["root"];
        assert_eq!(root["children"][0], source["root"]["children"][0]);
        let route = &root["children"][1];
        assert_eq!(route["branches"][0]["when"]["value"]["name"], "write");
        assert_eq!(
            route["branches"][0]["when"]["labels"]
                .as_array()
                .assert_value()
                .len(),
            4
        );
        assert_eq!(
            route["otherwise"]["children"],
            json!([source["root"]["children"][1], source["root"]["children"][2]])
        );
        assert_eq!(
            transform(result["graph"].clone(), action).assert_value()["graph"],
            result["graph"]
        );
    }

    #[test]
    fn parallel_failure_is_after_join_and_refresh_includes_added_writer() {
        let original = transform(
            graph(vec![worker("venue"), done()]),
            json!({"kind":"protect","node":"venue"}),
        )
        .assert_value();
        let mut expanded = original["graph"].clone();
        let group = parallel(vec![
            expanded["root"]["children"][0].clone(),
            worker("agenda"),
        ]);
        expanded["root"]["children"][0] = group.clone();
        let result =
            transform(expanded.clone(), json!({"kind":"protect","node":"plans"})).assert_value();
        let root = &result["graph"]["root"];
        assert_eq!(root["children"][0], group);
        assert_eq!(root["children"].as_array().assert_value().len(), 2);
        let mut expected_route = expanded["root"]["children"][1].clone();
        expected_route["branches"][0]["when"] = root["children"][1]["branches"][0]["when"].clone();
        assert_eq!(root["children"][1], expected_route);
        let selectors = root["children"][1]["branches"][0]["when"]["guards"]
            .as_array()
            .assert_value();
        assert_eq!(selectors.len(), 2);
        assert_eq!(selectors[0]["value"]["name"], "venue");
        assert_eq!(selectors[1]["value"]["name"], "agenda");
    }

    #[test]
    fn map_failure_uses_aggregate_errors_and_overflow_after_collection() {
        let mapped = json!({"kind":"map","name":"items","state":{"kind":"record","fields":{}},"body":worker("draft"),"over":{"source":"state","path":["items"]},"maxItems":8,"promotedStatePaths":[]});
        let result = transform(
            graph(vec![mapped.clone(), done()]),
            json!({"kind":"protect","node":"items"}),
        )
        .assert_value();
        let root = &result["graph"]["root"];
        assert_eq!(root["children"][0], mapped);
        let guards = root["children"][1]["branches"][0]["when"]["guards"]
            .as_array()
            .assert_value();
        assert_eq!(guards[0]["value"]["field"], "overflow");
        assert_eq!(guards[1]["kind"], "k_of_map");
        assert_eq!(guards[1]["count"], 1);
        assert_eq!(guards[1]["value"]["name"], "draft");
    }

    #[test]
    fn protection_rejects_branch_terminals_other_joins_and_partial_custom_guards() {
        let mut group = parallel(vec![sequence(
            "branch",
            vec![worker("write"), worker("review")],
        )]);
        assert!(
            transform(
                graph(vec![group.clone(), done()]),
                json!({"kind":"protect","node":"write"})
            )
            .is_err()
        );
        group["join"] = json!({"kind":"any"});
        assert!(
            transform(
                graph(vec![group, done()]),
                json!({"kind":"protect","node":"plans"})
            )
            .is_err()
        );
        assert!(
            transform(
                graph(vec![worker("write")]),
                json!({"kind":"protect","node":"write"})
            )
            .is_err()
        );
        let mut protected = transform(
            graph(vec![worker("write"), done()]),
            json!({"kind":"protect","node":"write"}),
        )
        .assert_value()["graph"]
            .clone();
        protected["root"]["children"][1]["branches"][0]["when"]["labels"] = json!(["crash"]);
        assert!(transform(protected, json!({"kind":"protect","node":"write"})).is_err());
    }

    #[tokio::test]
    async fn serial_lowering_passes_native_admission() {
        let completed = transform(
            graph(vec![worker("write")]),
            json!({"kind":"complete","owner":"run"}),
        )
        .assert_value();
        let protected = transform(
            completed["graph"].clone(),
            json!({"kind":"protect","node":"write"}),
        )
        .assert_value();
        super::super::validate_profile(
            &serde_json::from_value(protected["graph"].clone()).assert_value(),
            &serde_json::from_value(json!({"harness":"codex","provider":"openai","size":"small","nodes":{"write":{"kind":"agent","model":"opaque-model"}}})).assert_value(),
        ).await.map_err(|error| error.message).assert_value();
    }

    #[test]
    fn choice_error_policy_uses_branch_residuals_and_is_idempotent() {
        let when = json!({"kind":"in","value":{"name":"router","source":"signal","field":"verdict"},"labels":["accepted"]});
        let choice = json!({"kind":"choice","name":"choose","state":{"kind":"record","fields":{}},"branches":[{"when":when,"node":worker("left")}],"otherwise":worker("right"),"promotedStatePaths":[]});
        let source = graph(vec![verifier("router"), choice.clone(), done()]);
        let result = transform(source, json!({"kind":"protect","node":"choose"})).assert_value();
        assert_eq!(result["graph"]["root"]["children"][1], choice);
        let guards = &result["graph"]["root"]["children"][2]["branches"][0]["when"]["guards"];
        assert_eq!(guards[0]["guards"][0], when);
        assert_eq!(guards[0]["guards"][1]["value"]["name"], "left");
        assert_eq!(guards[1]["guards"][0], json!({"kind":"not","guard":when}));
        assert_eq!(guards[1]["guards"][1]["value"]["name"], "right");
        assert_eq!(
            transform(
                result["graph"].clone(),
                json!({"kind":"protect","node":"choose"})
            )
            .assert_value(),
            result
        );
    }

    #[test]
    fn choice_policy_refreshes_appended_branch_and_preserves_checkpoint_identity() {
        let when = json!({"kind":"in","value":{"name":"router","source":"signal","field":"verdict"},"labels":["accepted"]});
        let choice = json!({"kind":"choice","name":"choose","state":{"kind":"record","fields":{}},"branches":[{"when":when,"node":worker("left")}],"otherwise":worker("right"),"promotedStatePaths":[]});
        let result = transform(
            graph(vec![verifier("router"), choice, done()]),
            json!({"kind":"protect","node":"choose"}),
        )
        .assert_value();
        let mut expanded = result["graph"].clone();
        expanded["root"]["children"][1]["branches"].as_array_mut().assert_value().push(json!({"when":{"kind":"in","value":{"name":"router","source":"signal","field":"verdict"},"labels":["clarify"]},"node":worker("middle")}));
        let refreshed =
            transform(expanded, json!({"kind":"protect","node":"choose"})).assert_value();
        let checkpoint = &refreshed["graph"]["root"]["children"][2];
        assert_eq!(
            checkpoint["name"],
            result["graph"]["root"]["children"][2]["name"]
        );
        assert_eq!(
            checkpoint["otherwise"],
            result["graph"]["root"]["children"][2]["otherwise"]
        );
        let terms = checkpoint["branches"][0]["when"]["guards"]
            .as_array()
            .assert_value();
        assert_eq!(terms.len(), 3);
        assert_eq!(terms[1]["guards"][2]["value"]["name"], "middle");
        assert_eq!(terms[2]["guards"].as_array().assert_value().len(), 3);
    }

    fn verifier(name: &str) -> Value {
        let mut node = worker(name);
        node["kind"] = json!("verifier");
        node["signals"] = json!({"verdict":["accepted","rejected"]});
        node["diagnostic"] = json!({"kind":"null"});
        node
    }

    async fn admit_graph(graph: Value, nodes: Value) {
        super::super::validate_profile(
            &serde_json::from_value(graph).assert_value(),
            &serde_json::from_value(
                json!({"harness":"codex","provider":"openai","size":"small","nodes":nodes}),
            )
            .assert_value(),
        )
        .await
        .map_err(|error| error.message)
        .assert_value();
    }

    #[tokio::test]
    async fn parallel_and_map_checkpoints_pass_native_guard_availability_proofs() {
        // Step authoring is tested above; these fixtures keep guard proof independent of the
        // runtime's separately evolving writer concurrency policy.
        let source = graph(vec![
            parallel(vec![
                verifier("venue"),
                sequence("agenda_steps", vec![verifier("agenda"), verifier("budget")]),
            ]),
            done(),
        ]);
        let protected = transform(source, json!({"kind":"protect","node":"plans"})).assert_value();
        admit_graph(protected["graph"].clone(),json!({"venue":{"kind":"agent","model":"opaque-model"},"agenda":{"kind":"agent","model":"opaque-model"},"budget":{"kind":"agent","model":"opaque-model"}})).await;

        let state = json!({"kind":"record","fields":{"items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}});
        let mapped = json!({"kind":"map","name":"items_map","state":state,"body":verifier("draft"),"over":{"source":"state","path":["items"]},"maxItems":8,"promotedStatePaths":[]});
        let mut source = graph(vec![mapped, done()]);
        source["initialInput"] = state.clone();
        source["root"]["state"] = state;
        let protected =
            transform(source, json!({"kind":"protect","node":"items_map"})).assert_value();
        admit_graph(
            protected["graph"].clone(),
            json!({"draft":{"kind":"agent","model":"opaque-model"}}),
        )
        .await;
    }

    #[tokio::test]
    async fn protected_nested_sequence_preserves_suffix_state_promotions() {
        let state =
            json!({"kind":"record","fields":{"result":{"type":{"kind":"string"},"required":true}}});
        let mut writer = worker("write");
        writer["output"] = state.clone();
        writer["writeBindings"] = json!([{"target":["result"],"value":{"node":"write","channel":"out","path":["result"]}}]);
        let mut inner = sequence("inner", vec![worker("prepare"), writer]);
        inner["state"] = state.clone();
        inner["promotedStatePaths"] = json!([["result"]]);
        let mut result = done();
        result["output"] = state.clone();
        result["bindings"] =
            json!([{"target":["result"],"value":{"source":"state","path":["result"]}}]);
        let mut source = graph(vec![inner, result]);
        source["root"]["state"] = state;
        let protected =
            transform(source, json!({"kind":"protect","node":"prepare"})).assert_value();
        let checkpoint = &protected["graph"]["root"]["children"][0]["children"][1];
        assert_eq!(checkpoint["promotedStatePaths"], json!([["result"]]));
        assert_eq!(
            checkpoint["otherwise"]["promotedStatePaths"],
            json!([["result"]])
        );
        admit_graph(protected["graph"].clone(),json!({"prepare":{"kind":"agent","model":"opaque-model"},"write":{"kind":"agent","model":"opaque-model"}})).await;
    }

    #[tokio::test]
    async fn protected_loop_bodies_pass_native_guaranteed_exit_and_checkpoint_proofs() {
        for (body, nodes) in [
            (
                worker("write"),
                json!({"write":{"kind":"agent","model":"opaque-model"}}),
            ),
            (
                verifier("review"),
                json!({"review":{"kind":"agent","model":"opaque-model"}}),
            ),
            (
                sequence("round", vec![worker("write"), verifier("review")]),
                json!({"write":{"kind":"agent","model":"opaque-model"},"review":{"kind":"agent","model":"opaque-model"}}),
            ),
            (
                parallel(vec![verifier("left"), verifier("right")]),
                json!({"left":{"kind":"agent","model":"opaque-model"},"right":{"kind":"agent","model":"opaque-model"}}),
            ),
        ] {
            let protected = transform(
                graph(vec![repeat(body), done()]),
                json!({"kind":"protect","node":"repeat"}),
            )
            .assert_value();
            admit_graph(protected["graph"].clone(), nodes).await;
        }
    }

    struct ResultWorker;
    #[async_trait]
    impl WorkerRegistry for ResultWorker {
        async fn resolve(
            &self,
            worker: &WorkerRef,
        ) -> Result<WorkerDescriptor, WorkerRegistryError> {
            serde_json::from_value(json!({
                "worker":worker,"graphProfiles":["openengine.graph.full/v1"],"binding":{"protocol":"fixture","version":"1","profile":"fixture.worker/v1"},
                "contract":{"input":{"kind":"null"},"output":result_schema(),"errors":["timeout","crash","malformed","refusal"]},
                "capabilityPolicy":{"autonomy":"strict","permissionPolicy":"policy.strict@1"},
                "artifactProfile":{"allowedTypeIds":["openengine.result@1"],"allowedMediaTypes":["application/json"],"minimumRedaction":"internal"},"credentialRequirements":[]
            })).map_err(|_|WorkerRegistryError::NotFound{worker:worker.clone()})
        }
    }

    fn result_schema() -> Value {
        json!({"kind":"record","fields":{"result":{"type":{"kind":"string"},"required":true}}})
    }

    async fn protected_result_loop() -> VerifiedGraph {
        let state = result_schema();
        let mut write = worker("write");
        write["output"] = state.clone();
        write["writeBindings"] = json!([{"target":["result"],"value":{"node":"write","channel":"out","path":["result"]}}]);
        let mut repeated = repeat(write);
        repeated["state"] = state.clone();
        repeated["promotedStatePaths"] = json!([["result"]]);
        let mut result = done();
        result["output"] = state.clone();
        result["bindings"] =
            json!([{"target":["result"],"value":{"source":"state","path":["result"]}}]);
        let mut source = graph(vec![repeated, result]);
        source["root"]["state"] = state;
        let protected = transform(source, json!({"kind":"protect","node":"repeat"})).assert_value()
            ["graph"]
            .clone();
        admit_graph(
            protected.clone(),
            json!({"write":{"kind":"agent","model":"opaque-model"}}),
        )
        .await;
        ProductionGraphVerifier::new(ResultWorker)
            .verify(&serde_json::from_value(protected).assert_value())
            .await
            .assert_value()
    }

    fn settled_round(id: u64, outcome: WorkerOutcome) -> DurableExecution {
        DurableExecution {
            dispatch_position: HistoryPosition::new(id * 2 - 1).assert_value(),
            node_instance: NodeInstanceId::new(1).assert_value(),
            execution: ExecutionId::new(id).assert_value(),
            occurrence: StructuralOccurrence {
                node: NodeName::new("write").assert_value(),
                map_indices: Vec::new(),
            },
            attempt: PositiveInteger::new(1).assert_value(),
            input: Value::Null,
            state: DurableExecutionState::Settled {
                position: HistoryPosition::new(id * 2).assert_value(),
                outcome,
            },
        }
    }

    fn successful_round(id: u64) -> DurableExecution {
        settled_round(
            id,
            WorkerOutcome::Verified {
                output: json!({"result":format!("round {id}")}),
                artifacts: Vec::new(),
            },
        )
    }

    fn reduce_rounds(graph: &VerifiedGraph, history: &[DurableExecution]) -> Reduction {
        let result = FullV1Reducer::native_v2(graph).reduce(ReductionInput {
            initial_input: &json!({}),
            executions: history,
            next_node_instance: history.len() as u64 + 1,
            next_execution: history.len() as u64 + 1,
        });
        assert!(result.is_ok(), "{result:?}");
        result.assert_value()
    }

    #[tokio::test]
    async fn protected_fixed_count_repetition_finishes_all_successful_rounds_with_latest_result() {
        let graph = protected_result_loop().await;
        let mut history = Vec::new();
        for id in 1..=3 {
            let before = reduce_rounds(&graph, &history);
            assert!(before.terminal.is_none());
            assert!(before.decisions.iter().any(|decision|matches!(decision,Decision::Dispatch{occurrence,..} if occurrence.node.as_str()=="write")));
            history.push(successful_round(id));
        }
        let completed = reduce_rounds(&graph, &history);
        assert!(
            matches!(completed.terminal,Some(TerminalProjection::Succeeded{output,..}) if output==json!({"result":"round 3"}))
        );
    }

    #[tokio::test]
    async fn protected_fixed_count_repetition_fails_first_bad_round_including_final_round() {
        let graph = protected_result_loop().await;
        for error in [
            WorkerErrorCode::Crash,
            WorkerErrorCode::Malformed,
            WorkerErrorCode::Refusal,
            WorkerErrorCode::Timeout,
        ] {
            for failed_round in 1..=3 {
                let mut history = (1..failed_round).map(successful_round).collect::<Vec<_>>();
                history.push(settled_round(
                    failed_round,
                    WorkerOutcome::declared_failure(error),
                ));
                let failed = reduce_rounds(&graph, &history);
                assert!(
                    matches!(failed.terminal,Some(TerminalProjection::Failed{reason,..}) if reason.as_str()=="execution_failed")
                );
                assert!(
                    !failed
                        .decisions
                        .iter()
                        .any(|decision| matches!(decision, Decision::Dispatch { .. }))
                );
                assert!(!failed.decisions.iter().any(|decision| matches!(
                    decision,
                    Decision::Terminal {
                        projection: TerminalProjection::Succeeded { .. }
                    }
                )));
            }
        }
    }
}
