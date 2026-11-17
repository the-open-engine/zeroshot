use super::*;

pub(in crate::graph_verifier) struct AggregateCounts<'a> {
    pub(super) keys: &'a [SelectorKey],
    pub(super) outcomes: &'a [Assignment],
    pub(super) count_limits: &'a CountLimits,
}

/// Every saturated count state any number of items can add up to.
pub(in crate::graph_verifier) fn aggregate_assignments_from_outcomes(
    context: AggregateCounts<'_>,
    diagram: &mut Diagram,
) -> Option<Vec<Assignment>> {
    let empty = empty_outcome(context.keys);
    // Adding an item copies one count per selector; four counts weigh one search step.
    let weight = 1 + context.keys.len() / 4;
    // Close over finite input cardinalities using saturated correlated counts.
    let mut states = BTreeSet::from([empty.clone()]);
    let mut frontier = BTreeSet::from([empty]);
    loop {
        let mut next = BTreeSet::new();
        for state in &frontier {
            for outcome in context.outcomes {
                diagram.charge(weight)?;
                let candidate = add_aggregate_outcome(state, outcome, context.count_limits)?;
                if !states.contains(&candidate) {
                    next.insert(candidate);
                }
            }
        }
        if next.is_empty() {
            return Some(states.into_iter().collect());
        }
        if states.len().checked_add(next.len())? as u64 > FULL_V1_MAX_GUARD_ASSIGNMENTS {
            return None;
        }
        states.extend(next.iter().cloned());
        frontier = next;
    }
}

fn add_aggregate_outcome(
    state: &Assignment,
    outcome: &Assignment,
    count_limits: &CountLimits,
) -> Option<Assignment> {
    let mut next = state.clone();
    for (key, control) in outcome {
        let target = next.get_mut(key)?;
        for (label, count) in &control.counts {
            // Refinement and correlations observe presence even when no authored guard does.
            let limit = count_limits
                .get(key)
                .and_then(|limits| limits.get(label))
                .copied()
                .unwrap_or(1);
            let previous = target.counts.entry(label.clone()).or_default();
            *previous = previous.saturating_add(*count).min(limit);
        }
    }
    Some(next)
}

pub(in crate::graph_verifier) fn assigned_control(
    label: Option<EnumLabel>,
    occurrences: u64,
) -> AssignedControl {
    AssignedControl {
        counts: label
            .map(|label| BTreeMap::from([(label, occurrences)]))
            .unwrap_or_default(),
    }
}

pub(in crate::graph_verifier) fn selector_key_domain(
    key: &SelectorKey,
    node: &GraphNode,
) -> Vec<EnumLabel> {
    match key.source {
        ControlSourceKey::Signal => match node {
            GraphNode::Verifier(verifier) => key
                .field
                .as_ref()
                .and_then(|field| verifier.signals.get(field))
                .map(|labels| labels.values().to_vec())
                .unwrap_or_default(),
            _ => Vec::new(),
        },
        ControlSourceKey::Error => error_labels(),
        ControlSourceKey::Group => proof_group_domain(node, key.field.as_ref()).unwrap_or_default(),
        ControlSourceKey::Execution => enum_label("executed").into_iter().collect(),
    }
}

pub(in crate::graph_verifier) fn group_domain(
    node: &GraphNode,
    field: Option<&FieldName>,
) -> Option<Vec<EnumLabel>> {
    let expected = group_domain_labels(node, field?.as_str())?;
    Some(
        expected
            .iter()
            .filter_map(|label| enum_label(label))
            .collect(),
    )
}

fn group_domain_labels(node: &GraphNode, field: &str) -> Option<&'static [&'static str]> {
    match field {
        "terminated" => {
            matches!(node, GraphNode::Loop(_)).then_some(&["converged", "exhausted"][..])
        }

        "joined" => {
            parallel_join_domain(node, false).then_some(&["reached", "quorum_unreachable"][..])
        }
        "raced" => parallel_join_domain(node, true).then_some(&["satisfied", "no_satisfier"][..]),
        _ => None,
    }
}

fn parallel_join_domain(node: &GraphNode, first: bool) -> bool {
    let GraphNode::Par(parallel) = node else {
        return false;
    };
    matches!(parallel.join, Join::First { .. }) == first
}

pub(in crate::graph_verifier) fn error_labels() -> Vec<EnumLabel> {
    ["timeout", "crash", "malformed", "refusal"]
        .into_iter()
        .filter_map(enum_label)
        .collect()
}

pub(in crate::graph_verifier) fn signal_path_type(
    verifier: &VerifierNode,
    path: &FieldPath,
) -> Option<PayloadType> {
    let [field] = path.segments() else {
        return None;
    };
    verifier
        .signals
        .get(field)
        .cloned()
        .map(|values| PayloadType::Enum { values })
}
