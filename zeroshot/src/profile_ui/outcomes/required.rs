//! Success checkpoints for newly authored required data dependencies.
use super::*;

/// The data edit and this lowering form one draft transaction. Existing custom recovery is never
/// replaced: only a complete first error-to-Fail branch can move before a new required consumer.
pub(in crate::profile_ui) fn ensure_required_output(
    graph: &mut GraphSpec,
    runtime: &Value,
    source: &NodeName,
    consumer: &NodeName,
) -> Result<(), ApiError> {
    if named_nodes(&graph.root, source) != 1 || named_nodes(&graph.root, consumer) != 1 {
        return Err(invalid("Select uniquely named output and input nodes."));
    }
    let source_path = path_to(&graph.root, source).ok_or_else(|| invalid("Output not found."))?;
    let consumer_path =
        path_to(&graph.root, consumer).ok_or_else(|| invalid("Input not found."))?;
    let shared = source_path
        .iter()
        .zip(&consumer_path)
        .take_while(|(a, b)| a.name() == b.name())
        .count();
    if shared == 0 || shared == source_path.len() || shared == consumer_path.len() {
        return Err(invalid("Select an earlier output."));
    }
    let GraphNode::Seq(owner) = source_path[shared - 1] else {
        return Err(invalid("Select an earlier output."));
    };
    let scope = source_path[shared];
    let position = owner
        .children
        .as_slice()
        .iter()
        .position(|node| node.name() == scope.name())
        .ok_or_else(|| invalid("Output scope not found."))?;
    let suffix = &owner.children.as_slice()[position + 1..];
    if !suffix.iter().any(|node| named_nodes(node, consumer) == 1) {
        return Err(invalid("Select an earlier output."));
    }
    let guard = match protected_errors(scope) {
        Ok(guard) => guard,
        Err(error) => {
            // An imported complex scope may already have a valid, more precise checkpoint for
            // this source. Recognize it without rewriting or broadening that scope's policy.
            if protected_errors(source_path[source_path.len() - 1])
                .is_ok_and(|guard| consumer_is_guarded(suffix, &guard, consumer))
            {
                return Ok(());
            }
            return Err(error);
        }
    };
    if consumer_is_guarded(suffix, &guard, consumer) {
        return Ok(());
    }
    let target = scope.name().clone();
    let mut watched = BTreeSet::new();
    collect_guard_names(&guard, &mut watched);
    let references = suffix
        .iter()
        .any(|node| watched.iter().any(|name| references_guard(node, name)));
    let mut candidate = graph.clone();
    let mut reason = None;
    if references {
        let mut expected = BTreeSet::new();
        // Route-masked Choice errors and Map aggregates retain their existing explicit policy;
        // factoring is intentionally limited to direct full worker-error sets.
        let expected_guards = plain_error_terms(&guard).ok_or_else(custom_handling)?;
        for error in expected_guards {
            collect_guard_names(error, &mut expected);
        }
        let mut seen = false;
        let (handler, _) = find_late_handler(suffix, consumer, &expected, &mut seen)
            .ok()
            .flatten()
            .ok_or_else(custom_handling)?;
        let mut edited_suffix = suffix.to_vec();
        for node in &mut edited_suffix {
            edit(node, &mut |node| {
                if node.name() == &handler {
                    reason = Some(split_failure(node, &expected)?);
                }
                Ok(())
            })?;
        }
        // Other ordered, partial, signal, or recovery handlers may still own these outcomes.
        // Reject instead of changing their priority or borrowing one arbitrary failure reason.
        if edited_suffix
            .iter()
            .any(|node| watched.iter().any(|name| references_guard(node, name)))
        {
            return Err(custom_handling());
        }
        edit(&mut candidate.root, &mut |node| {
            if node.name() == &handler {
                split_failure(node, &expected)?;
            }
            Ok(())
        })?;
    }
    let mut names = reserved_names(graph, runtime)?;
    protect_with_reason(&mut candidate.root, &target, &mut names, reason)?;
    *graph = candidate;
    Ok(())
}

fn path_to<'a>(node: &'a GraphNode, target: &NodeName) -> Option<Vec<&'a GraphNode>> {
    if node.name() == target {
        return Some(vec![node]);
    }
    children(node).into_iter().find_map(|child| {
        let mut path = path_to(child, target)?;
        path.insert(0, node);
        Some(path)
    })
}

fn covers_errors(existing: &Guard, expected: &Guard) -> bool {
    if existing == expected {
        return true;
    }
    let mut existing_atoms = BTreeSet::new();
    let mut expected_atoms = BTreeSet::new();
    standard_atoms(existing, &mut existing_atoms)
        && standard_atoms(expected, &mut expected_atoms)
        && expected_atoms.is_subset(&existing_atoms)
}

fn consumer_is_guarded(suffix: &[GraphNode], guard: &Guard, consumer: &NodeName) -> bool {
    let Some(path) = suffix.iter().find_map(|node| path_to(node, consumer)) else {
        return false;
    };
    path.windows(2).any(|pair| {
        let GraphNode::Choice(choice) = pair[0] else {
            return false;
        };
        let first = &choice.branches.as_slice()[0];
        first.node.name() != pair[1].name()
            && matches!(first.node, GraphNode::Fail(_))
            && covers_errors(&first.when, guard)
    })
}

fn plain_error_terms(guard: &Guard) -> Option<Vec<&Guard>> {
    match guard {
        Guard::Any { guards } => {
            let mut terms = Vec::new();
            for guard in guards.as_slice() {
                terms.extend(plain_error_terms(guard)?);
            }
            Some(terms)
        }
        Guard::In { value, .. }
            if value.source == ControlSource::Error
                && standard_atoms(guard, &mut BTreeSet::new()) =>
        {
            Some(vec![guard])
        }
        _ => None,
    }
}

fn split_terms(choice: &ChoiceNode, expected: &BTreeSet<NodeName>) -> Option<Vec<Guard>> {
    let branch = &choice.branches.as_slice()[0];
    if !matches!(branch.node, GraphNode::Fail(_)) {
        return None;
    }
    let terms = plain_error_terms(&branch.when)?;
    let mut matched = BTreeSet::new();
    let mut remaining = Vec::new();
    for term in terms {
        let Guard::In { value, .. } = term else {
            return None;
        };
        if expected.contains(&value.name) {
            matched.insert(value.name.clone());
        } else {
            remaining.push(term.clone());
        }
    }
    (matched == *expected).then_some(remaining)
}

/// Search only a sequential continuation, including otherwise paths of ordinary failure wrappers.
/// A business branch or a parallel/map body does not grant authority to hoist a terminal outcome.
fn find_late_handler(
    nodes: &[GraphNode],
    consumer: &NodeName,
    expected: &BTreeSet<NodeName>,
    seen: &mut bool,
) -> Result<Option<(NodeName, FailReason)>, ()> {
    for node in nodes {
        match node {
            GraphNode::Seq(group) => {
                if let Some(found) =
                    find_late_handler(group.children.as_slice(), consumer, expected, seen)?
                {
                    return Ok(Some(found));
                }
            }
            GraphNode::Choice(choice) => {
                if *seen && split_terms(choice, expected).is_some() {
                    let GraphNode::Fail(failure) = &choice.branches.as_slice()[0].node else {
                        return Err(());
                    };
                    return Ok(Some((choice.name.clone(), failure.reason.clone())));
                }
                let [branch] = choice.branches.as_slice() else {
                    return Err(());
                };
                if !matches!(branch.node, GraphNode::Fail(_))
                    || plain_error_terms(&branch.when).is_none()
                {
                    return Err(());
                }
                if let Some(otherwise) = choice.otherwise.as_deref() {
                    let consumer_reached = *seen;
                    if let Some(found) = find_late_handler(
                        std::slice::from_ref(otherwise),
                        consumer,
                        expected,
                        seen,
                    )? {
                        let GraphNode::Fail(failure) = &branch.node else {
                            return Err(());
                        };
                        if failure.reason != found.1
                            && !(consumer_reached && is_consumer_default(branch, consumer))
                        {
                            return Err(());
                        }
                        return Ok(Some(found));
                    }
                }
                // Keep this lowering bounded to a handler inside the same continuation. A
                // sibling after a completed choice would need its earlier terminal priorities
                // carried through a different scope; leave that authored structure untouched.
                return Err(());
            }
            GraphNode::Loop(_) | GraphNode::Map(_) | GraphNode::Fail(_) | GraphNode::Succeed(_) => {
                return Err(());
            }
            _ => {}
        }
        *seen |= named_nodes(node, consumer) == 1;
    }
    Ok(None)
}

fn is_consumer_default(branch: &ChoiceBranch, consumer: &NodeName) -> bool {
    let GraphNode::Fail(failure) = &branch.node else {
        return false;
    };
    // A newly required dependency makes this consumer ineligible when the source fails. Its
    // ordinary execution_failed fallback stays intact for source-success/consumer-failure.
    failure.reason.as_label().as_str() == "execution_failed"
        && worker_errors(consumer, false).is_ok_and(|guard| branch.when == guard)
}

fn split_failure(
    node: &mut GraphNode,
    expected: &BTreeSet<NodeName>,
) -> Result<FailReason, ApiError> {
    let GraphNode::Choice(choice) = node else {
        return Err(custom_handling());
    };
    let remaining = split_terms(choice, expected).ok_or_else(custom_handling)?;
    let mut branches = choice.branches.clone().into_vec();
    let GraphNode::Fail(failure) = &branches[0].node else {
        return Err(custom_handling());
    };
    let reason = failure.reason.clone();
    if !remaining.is_empty() {
        branches[0].when = if remaining.len() == 1 {
            remaining[0].clone()
        } else {
            Guard::Any {
                guards: non_empty(remaining)?,
            }
        };
        choice.branches = non_empty(branches)?;
    } else {
        branches.remove(0);
        if branches.is_empty() {
            let otherwise = choice.otherwise.take().ok_or_else(custom_handling)?;
            *node = GraphNode::Seq(SeqNode {
                name: choice.name.clone(),
                state: choice.state.clone(),
                children: non_empty(vec![*otherwise])?,
                promoted_state_paths: choice.promoted_state_paths.clone(),
            });
        } else {
            choice.branches = non_empty(branches)?;
        }
    }
    Ok(reason)
}

fn custom_handling() -> ApiError {
    invalid("This output has custom outcome handling before it can be consumed.")
}

#[cfg(test)]
#[path = "required_tests.rs"]
mod tests;
