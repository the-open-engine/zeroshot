use super::*;

pub(super) fn outcome_refinement(assignments: &[&Assignment]) -> OutcomeRefinement {
    let names = assignments
        .iter()
        .flat_map(|assignment| assignment.keys())
        .filter(|key| {
            matches!(
                key.source,
                ControlSourceKey::Signal | ControlSourceKey::Error
            )
        })
        .map(|key| key.name.clone())
        .collect::<BTreeSet<_>>();
    let mut refinement = OutcomeRefinement::default();
    for name in names {
        if assignments
            .iter()
            .any(|assignment| assignment_has_error(assignment, &name))
        {
            refinement.failed.insert(name);
        } else {
            refinement.success.insert(name);
        }
    }
    refinement
}

pub(super) fn assignment_has_error(assignment: &Assignment, name: &NodeName) -> bool {
    let mut has_signal = false;
    let mut signal_occurrences = 0_u64;
    for (key, value) in assignment.iter().filter(|(key, _)| key.name == *name) {
        match key.source {
            ControlSourceKey::Error if value.counts.values().any(|count| *count > 0) => {
                return true;
            }
            ControlSourceKey::Signal => {
                has_signal = true;
                signal_occurrences =
                    signal_occurrences.saturating_add(value.counts.values().copied().sum::<u64>());
            }
            ControlSourceKey::Error | ControlSourceKey::Group => {}
        }
    }
    has_signal && signal_occurrences == 0
}

pub(super) fn build_dimensions(
    selectors: &[ControlSelector],
    nodes: &BTreeMap<NodeName, NodeInfo<'_>>,
    map_aggregates: &BTreeMap<SelectorKey, u64>,
) -> Option<Vec<Vec<Assignment>>> {
    let mut by_node = BTreeMap::<NodeName, BTreeSet<SelectorKey>>::new();
    for selector in selectors {
        by_node
            .entry(selector.name.clone())
            .or_default()
            .insert(SelectorKey::from(selector));
    }
    let mut dimensions = Vec::new();
    for (name, keys) in by_node {
        let Some(node) = nodes.get(&name).map(|info| info.node) else {
            continue;
        };
        dimensions.extend(dimensions_for_node(node, &keys, map_aggregates)?);
    }
    Some(dimensions)
}

pub(super) fn enumerate_assignments(dimensions: Vec<Vec<Assignment>>) -> Vec<Assignment> {
    let mut assignments = vec![Assignment::new()];
    for dimension in dimensions {
        let mut next = Vec::with_capacity(assignments.len() * dimension.len());
        for prefix in &assignments {
            for choice in &dimension {
                let mut assignment = prefix.clone();
                assignment.extend(choice.clone());
                next.push(assignment);
            }
        }
        assignments = next;
    }
    assignments
}

pub(super) fn dimensions_for_node(
    node: &GraphNode,
    keys: &BTreeSet<SelectorKey>,
    map_aggregates: &BTreeMap<SelectorKey, u64>,
) -> Option<Vec<Vec<Assignment>>> {
    let aggregate = keys
        .iter()
        .filter(|key| {
            map_aggregates.contains_key(*key)
                && matches!(
                    key.source,
                    ControlSourceKey::Signal | ControlSourceKey::Error
                )
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut dimensions = Vec::new();
    if !aggregate.is_empty() {
        let maximum = aggregate
            .iter()
            .filter_map(|key| map_aggregates.get(key))
            .copied()
            .min()?;
        dimensions.push(aggregate_outcome_dimension(node, &aggregate, maximum)?);
    }
    let scalar = keys
        .iter()
        .filter(|key| !aggregate.contains(key))
        .cloned()
        .collect::<Vec<_>>();
    dimensions.extend(group_control_dimensions(node, &scalar));
    let outcomes = scalar
        .into_iter()
        .filter(|key| key.source != ControlSourceKey::Group)
        .collect::<Vec<_>>();
    if !outcomes.is_empty() {
        dimensions.push(single_outcome_dimension(node, &outcomes)?);
    }
    Some(dimensions)
}

pub(super) fn group_control_dimensions(
    node: &GraphNode,
    keys: &[SelectorKey],
) -> Vec<Vec<Assignment>> {
    keys.iter()
        .filter(|key| key.source == ControlSourceKey::Group)
        .map(|key| {
            group_domain(node, key.field.as_ref())
                .unwrap_or_default()
                .into_iter()
                .map(|label| BTreeMap::from([(key.clone(), assigned_control(Some(label), 1))]))
                .collect()
        })
        .collect()
}

pub(super) fn single_outcome_dimension(
    node: &GraphNode,
    keys: &[SelectorKey],
) -> Option<Vec<Assignment>> {
    let choices = per_execution_outcomes(node, keys)?;
    (choices.len() as u64 <= FULL_V1_MAX_GUARD_ASSIGNMENTS).then_some(choices)
}

pub(super) fn aggregate_outcome_dimension(
    node: &GraphNode,
    keys: &[SelectorKey],
    maximum: u64,
) -> Option<Vec<Assignment>> {
    let outcomes = per_execution_outcomes(node, keys)?;
    let count = bounded_distribution_count(maximum, outcomes.len())?;
    if count > FULL_V1_MAX_GUARD_ASSIGNMENTS {
        return None;
    }
    let expected = usize::try_from(count).ok()?;
    let mut assignments = Vec::with_capacity(expected);
    let mut valid = true;
    let completed = for_each_outcome_multiset(outcomes.len(), maximum, |outcome_occurrences| {
        if assignments.len() >= expected {
            valid = false;
            return false;
        }
        let Some(assignment) = combine_outcome_occurrences(keys, &outcomes, outcome_occurrences)
        else {
            valid = false;
            return false;
        };
        assignments.push(assignment);
        true
    });
    (completed && valid && assignments.len() == expected).then_some(assignments)
}

pub(super) fn per_execution_outcomes(
    node: &GraphNode,
    keys: &[SelectorKey],
) -> Option<Vec<Assignment>> {
    let signal_keys = keys
        .iter()
        .filter(|key| key.source == ControlSourceKey::Signal)
        .cloned()
        .collect::<Vec<_>>();
    let error_keys = keys
        .iter()
        .filter(|key| key.source == ControlSourceKey::Error)
        .cloned()
        .collect::<Vec<_>>();
    let mut successful = vec![Assignment::new()];
    for key in &signal_keys {
        let labels = selector_key_domain(key, node);
        successful = cartesian_signal_outcomes(successful, key, &labels);
        if successful.len() as u64 > FULL_V1_MAX_GUARD_ASSIGNMENTS {
            return None;
        }
    }
    for assignment in &mut successful {
        insert_empty_controls(assignment, &error_keys);
    }
    let mut outcomes = successful;
    for error in error_labels() {
        let mut assignment = Assignment::new();
        insert_empty_controls(&mut assignment, &signal_keys);
        for key in &error_keys {
            assignment.insert(key.clone(), assigned_control(Some(error.clone()), 1));
        }
        outcomes.push(assignment);
    }
    Some(outcomes)
}

pub(super) fn cartesian_signal_outcomes(
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

pub(super) fn insert_empty_controls(assignment: &mut Assignment, keys: &[SelectorKey]) {
    for key in keys {
        assignment.insert(key.clone(), assigned_control(None, 0));
    }
}

pub(super) fn combine_outcome_occurrences(
    keys: &[SelectorKey],
    outcomes: &[Assignment],
    outcome_occurrences: &[usize],
) -> Option<Assignment> {
    let mut combined = keys
        .iter()
        .cloned()
        .map(|key| (key, assigned_control(None, 0)))
        .collect::<Assignment>();
    for outcome_index in outcome_occurrences {
        let outcome = outcomes.get(*outcome_index)?;
        for (key, value) in outcome {
            let target = combined
                .entry(key.clone())
                .or_insert_with(|| assigned_control(None, 0));
            for (label, count) in &value.counts {
                let target_count = target.counts.entry(label.clone()).or_default();
                *target_count = target_count.checked_add(*count)?;
            }
        }
    }
    Some(combined)
}

pub(super) fn evaluate_guard(guard: &Guard, assignment: &Assignment) -> bool {
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
            values
                .as_slice()
                .iter()
                .filter(|selector| selector_matches(selector, labels.values(), assignment))
                .count() as u64
                >= count.get()
        }
        Guard::KOfMap {
            count,
            value,
            labels,
        } => selector_occurrences(value, labels.values(), assignment) >= count.get(),
    }
}

pub(super) fn selector_matches(
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

pub(super) fn selector_occurrences(
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

pub(super) fn assigned_control(label: Option<EnumLabel>, occurrences: u64) -> AssignedControl {
    AssignedControl {
        counts: label
            .map(|label| BTreeMap::from([(label, occurrences)]))
            .unwrap_or_default(),
    }
}

pub(super) fn bounded_distribution_count(maximum: u64, labels: usize) -> Option<u64> {
    // Weak compositions of at most `maximum` occurrences across `labels` values:
    // C(maximum + labels, labels). Stop as soon as the verifier ceiling is crossed.
    let labels = u64::try_from(labels).ok()?;
    let mut result = 1_u64;
    for divisor in 1..=labels {
        let numerator = maximum.checked_add(divisor)?;
        result = result.checked_mul(numerator)?.checked_div(divisor)?;
        if result > FULL_V1_MAX_GUARD_ASSIGNMENTS {
            return Some(result);
        }
    }
    Some(result)
}

pub(super) fn for_each_outcome_multiset(
    outcomes: usize,
    maximum: u64,
    mut visit: impl FnMut(&[usize]) -> bool,
) -> bool {
    if !visit(&[]) {
        return false;
    }
    if outcomes == 0 {
        return true;
    }
    let Ok(maximum) = usize::try_from(maximum) else {
        return false;
    };

    // A nondecreasing sequence of outcome indexes is the sparse form of one
    // count vector. Iterating multisets keeps stack and working memory bounded
    // by `maximum`, even when one map item has tens of thousands of outcomes.
    for total_occurrences in 1..=maximum {
        let mut occurrence_indexes = vec![0; total_occurrences];
        loop {
            if !visit(&occurrence_indexes) {
                return false;
            }
            let Some(position) = occurrence_indexes
                .iter()
                .rposition(|outcome| *outcome + 1 < outcomes)
            else {
                break;
            };
            let next_outcome = occurrence_indexes[position] + 1;
            occurrence_indexes[position..].fill(next_outcome);
        }
    }
    true
}

pub(super) fn selector_key_domain(key: &SelectorKey, node: &GraphNode) -> Vec<EnumLabel> {
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
        ControlSourceKey::Group => group_domain(node, key.field.as_ref()).unwrap_or_default(),
    }
}

pub(super) fn group_domain(node: &GraphNode, field: Option<&FieldName>) -> Option<Vec<EnumLabel>> {
    let expected = group_domain_labels(node, field?.as_str())?;
    Some(expected.iter().map(|label| enum_label(label)).collect())
}

fn group_domain_labels(node: &GraphNode, field: &str) -> Option<&'static [&'static str]> {
    match field {
        "terminated" => {
            matches!(node, GraphNode::Loop(_)).then_some(&["converged", "exhausted"][..])
        }
        "overflow" => matches!(node, GraphNode::Map(_)).then_some(&["ok", "overflow"][..]),
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

pub(super) fn error_labels() -> Vec<EnumLabel> {
    ["timeout", "crash", "malformed", "refusal"]
        .into_iter()
        .map(enum_label)
        .collect()
}

pub(super) fn signal_path_type(verifier: &VerifierNode, path: &FieldPath) -> Option<PayloadType> {
    let [field] = path.segments() else {
        return None;
    };
    verifier
        .signals
        .get(field)
        .cloned()
        .map(|values| PayloadType::Enum { values })
}
