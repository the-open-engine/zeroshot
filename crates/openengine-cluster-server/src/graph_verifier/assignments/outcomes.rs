use super::*;

pub(in crate::graph_verifier) struct AggregateCounts<'a> {
    pub(super) keys: &'a [SelectorKey],
    pub(super) outcomes: &'a [Assignment],
    pub(super) count_limits: &'a CountLimits,
    pub(super) budget: &'a ProofWork,
}

pub(in crate::graph_verifier) fn aggregate_assignments_from_outcomes(
    context: AggregateCounts<'_>,
) -> Option<Vec<Assignment>> {
    let keys = context.keys;
    let empty = keys
        .iter()
        .cloned()
        .map(|key| (key, assigned_control(None, 0)))
        .collect::<Assignment>();
    // Close over finite input cardinalities using saturated correlated counts.
    let mut states = BTreeSet::from([empty.clone()]);
    let mut frontier = BTreeSet::from([empty]);
    loop {
        let next = next_aggregate_layer(&frontier, &states, &context)?;
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

fn next_aggregate_layer(
    frontier: &BTreeSet<Assignment>,
    excluded: &BTreeSet<Assignment>,
    context: &AggregateCounts<'_>,
) -> Option<BTreeSet<Assignment>> {
    let mut next = BTreeSet::new();
    for state in frontier {
        for outcome in context.outcomes {
            let candidate =
                add_aggregate_outcome(state, outcome, context.count_limits, context.budget)?;
            if excluded.contains(&candidate) {
                continue;
            }
            if !next.contains(&candidate) && next.len() as u64 >= FULL_V1_MAX_GUARD_ASSIGNMENTS {
                return None;
            }
            next.insert(candidate);
        }
    }
    Some(next)
}

fn add_aggregate_outcome(
    state: &Assignment,
    outcome: &Assignment,
    count_limits: &CountLimits,
    budget: &ProofWork,
) -> Option<Assignment> {
    budget.charge(
        2 * (1 + state.len() + state.values().map(|c| c.counts.len()).sum::<usize>())
            + outcome.values().map(|c| c.counts.len()).sum::<usize>(),
    )?;
    let mut next = state.clone();
    for (key, control) in outcome {
        let target = next.get_mut(key)?;
        for (label, count) in &control.counts {
            let limit = count_limits
                .get(key)
                .and_then(|limits| limits.get(label))
                .copied()
                .unwrap_or(0);
            if limit == 0 {
                continue;
            }
            let previous = target.counts.entry(label.clone()).or_default();
            *previous = previous.saturating_add(*count).min(limit);
        }
    }
    Some(next)
}

pub(in crate::graph_verifier) fn assignment_matches_parallel_join_correlation(
    assignment: &Assignment,
    name: &NodeName,
    correlation: &ParallelJoinCorrelation,
) -> bool {
    if !retained_selection_matches(assignment, name, correlation.retention()) {
        return false;
    }
    let Some(control) = parallel_join_control(assignment, name, correlation) else {
        return true;
    };
    if control.counts.values().all(|count| *count == 0) {
        return true;
    }
    match correlation {
        ParallelJoinCorrelation::Joined { completion, .. } => {
            joined_control_matches(control, completion, assignment)
        }
        ParallelJoinCorrelation::First { satisfaction, .. } => {
            first_control_matches(control, satisfaction, assignment)
        }
    }
}

fn joined_control_matches(
    control: &AssignedControl,
    completion: &CompletionPredicate,
    assignment: &Assignment,
) -> bool {
    let completed = completion.evaluate(assignment);
    (control_reports(control, "reached") && completed)
        || (control_reports(control, "quorum_unreachable") && !completed)
}

fn first_control_matches(
    control: &AssignedControl,
    satisfaction: &CompletionPredicate,
    assignment: &Assignment,
) -> bool {
    let satisfied = satisfaction.evaluate(assignment);
    (control_reports(control, "satisfied") && satisfied)
        || (control_reports(control, "no_satisfier") && !satisfied)
}

fn parallel_join_control<'a>(
    assignment: &'a Assignment,
    name: &NodeName,
    correlation: &ParallelJoinCorrelation,
) -> Option<&'a AssignedControl> {
    assignment.iter().find_map(|(key, value)| {
        (key.name == *name
            && key.source == ControlSourceKey::Group
            && key
                .field
                .as_ref()
                .is_some_and(|field| field.as_str() == correlation.field()))
        .then_some(value)
    })
}

fn control_reports(control: &AssignedControl, expected: &str) -> bool {
    control
        .counts
        .iter()
        .any(|(label, count)| label.as_str() == expected && *count > 0)
}

pub(in crate::graph_verifier) fn per_execution_outcomes(
    node: &GraphNode,
    keys: &[SelectorKey],
    budget: &ProofWork,
) -> Option<Vec<Assignment>> {
    let width = keys.len().saturating_mul(2).saturating_add(1);
    let keys = OutcomeKeys::from(keys);
    let successful_count = successful_outcome_count(node, &keys.emitted)?;
    let runtime_errors = runtime_errors_for_node(node);
    let total_outcomes = successful_count.checked_add(u64::try_from(runtime_errors.len()).ok()?)?;
    if total_outcomes > FULL_V1_MAX_GUARD_ASSIGNMENTS {
        return None;
    }
    budget.charge(usize::try_from(total_outcomes).ok()?.checked_mul(width)?)?;
    let successful = successful_outcomes(node, &keys);
    Some(with_runtime_errors(successful, &runtime_errors, &keys))
}

struct OutcomeKeys {
    emitted: Vec<SelectorKey>,
    errors: Vec<SelectorKey>,
    executions: Vec<SelectorKey>,
}

impl From<&[SelectorKey]> for OutcomeKeys {
    fn from(keys: &[SelectorKey]) -> Self {
        let emitted = keys
            .iter()
            .filter(|key| {
                matches!(
                    key.source,
                    ControlSourceKey::Signal | ControlSourceKey::Group
                )
            })
            .cloned()
            .collect();
        let errors = keys
            .iter()
            .filter(|key| key.source == ControlSourceKey::Error)
            .cloned()
            .collect();
        let executions = keys
            .iter()
            .filter(|key| key.source == ControlSourceKey::Execution)
            .cloned()
            .collect();
        Self {
            emitted,
            errors,
            executions,
        }
    }
}

fn successful_outcome_count(node: &GraphNode, emitted_keys: &[SelectorKey]) -> Option<u64> {
    let mut count = 1_u64;
    for key in emitted_keys {
        let label_count = u64::try_from(selector_key_domain(key, node).len()).ok()?;
        count = count.checked_mul(label_count)?;
        if count > FULL_V1_MAX_GUARD_ASSIGNMENTS {
            return None;
        }
    }
    Some(count)
}

fn runtime_errors_for_node(node: &GraphNode) -> Vec<EnumLabel> {
    matches!(node, GraphNode::Step(_) | GraphNode::Verifier(_))
        .then(error_labels)
        .unwrap_or_default()
}

fn successful_outcomes(node: &GraphNode, keys: &OutcomeKeys) -> Vec<Assignment> {
    let mut successful = vec![Assignment::new()];
    for key in &keys.emitted {
        let labels = selector_key_domain(key, node);
        successful = cartesian_emitted_outcomes(successful, key, &labels);
    }
    for assignment in &mut successful {
        insert_empty_controls(assignment, &keys.errors);
        insert_execution_controls(assignment, &keys.executions);
    }
    successful
}

fn with_runtime_errors(
    mut outcomes: Vec<Assignment>,
    runtime_errors: &[EnumLabel],
    keys: &OutcomeKeys,
) -> Vec<Assignment> {
    for error in runtime_errors {
        let mut assignment = Assignment::new();
        insert_empty_controls(&mut assignment, &keys.emitted);
        insert_execution_controls(&mut assignment, &keys.executions);
        for key in &keys.errors {
            assignment.insert(key.clone(), assigned_control(Some(error.clone()), 1));
        }
        outcomes.push(assignment);
    }
    outcomes.dedup();
    outcomes
}

fn insert_execution_controls(assignment: &mut Assignment, keys: &[SelectorKey]) {
    let Some(executed) = enum_label("executed") else {
        return;
    };
    for key in keys {
        assignment.insert(key.clone(), assigned_control(Some(executed.clone()), 1));
    }
}

pub(in crate::graph_verifier) fn cartesian_emitted_outcomes(
    prefixes: Vec<Assignment>,
    key: &SelectorKey,
    labels: &[EnumLabel],
) -> Vec<Assignment> {
    prefixes
        .into_iter()
        .flat_map(|prefix| {
            labels.iter().map(move |label| {
                let mut choice = prefix.clone();
                choice.insert(key.clone(), assigned_control(Some(label.clone()), 1));
                choice
            })
        })
        .collect()
}

pub(in crate::graph_verifier) fn insert_empty_controls(
    assignment: &mut Assignment,
    keys: &[SelectorKey],
) {
    for key in keys {
        assignment.insert(key.clone(), assigned_control(None, 0));
    }
}

pub(in crate::graph_verifier) fn evaluate_guard(guard: &Guard, assignment: &Assignment) -> bool {
    match guard {
        Guard::In { value, labels } => selector_matches(value, labels.values(), assignment),
        Guard::All { guards } => guards
            .as_slice()
            .iter()
            .all(|guard| evaluate_guard(guard, assignment)),
        Guard::Any { guards } => guards
            .as_slice()
            .iter()
            .any(|guard| evaluate_guard(guard, assignment)),
        Guard::Not { guard } => !evaluate_guard(guard, assignment),
        Guard::KOfN {
            count,
            values,
            labels,
        } => {
            u64::try_from(
                values
                    .as_slice()
                    .iter()
                    .filter(|selector| selector_matches(selector, labels.values(), assignment))
                    .count(),
            )
            .unwrap_or(u64::MAX)
                >= count.get()
        }
        Guard::KOfMap {
            count,
            value,
            labels,
        } => selector_occurrences(value, labels.values(), assignment) >= count.get(),
    }
}

pub(in crate::graph_verifier) fn selector_matches(
    selector: &ControlSelector,
    labels: &[EnumLabel],
    assignment: &Assignment,
) -> bool {
    assignment
        .get(&SelectorKey::from(selector))
        .is_some_and(|value| {
            labels
                .iter()
                .any(|label| value.counts.get(label).is_some_and(|count| *count > 0))
        })
}

pub(in crate::graph_verifier) fn selector_occurrences(
    selector: &ControlSelector,
    labels: &[EnumLabel],
    assignment: &Assignment,
) -> u64 {
    assignment
        .get(&SelectorKey::from(selector))
        .map_or(0, |value| {
            labels
                .iter()
                .filter_map(|label| value.counts.get(label))
                .copied()
                .sum()
        })
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

// Check before item aggregation: different map items may terminate differently, but a single
// final body verdict and its loop label must describe the same legal outcome.
pub(super) fn loop_outcome_matches(group: &LoopNode, assignment: &Assignment) -> bool {
    let key = SelectorKey {
        name: group.name.clone(),
        source: ControlSourceKey::Group,
        field: FieldName::new("terminated").ok(),
    };
    let Some(control) = assignment.get(&key) else {
        return true;
    };
    let converged = group
        .until
        .as_ref()
        .is_some_and(|until| evaluate_guard(until, assignment));
    control.counts.iter().all(|(label, count)| {
        *count == 0
            || match label.as_str() {
                "converged" => converged,
                "exhausted" => !converged,
                _ => false,
            }
    })
}
