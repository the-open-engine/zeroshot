use super::*;

pub(super) fn outcome_refinement(assignments: &[&Assignment]) -> OutcomeRefinement {
    let names = assignments
        .iter()
        .flat_map(|assignment| assignment.keys())
        .filter(|key| {
            matches!(
                key.source,
                ControlSourceKey::Signal | ControlSourceKey::Error
            ) || is_parallel_group_control(key)
        })
        .map(|key| key.name.clone())
        .collect::<BTreeSet<_>>();
    let mut refinement = OutcomeRefinement::default();
    for name in names {
        if assignments
            .iter()
            .any(|assignment| assignment_has_failure(assignment, &name))
        {
            refinement.failed.insert(name);
        } else {
            refinement.success.insert(name);
        }
    }
    refinement
}

pub(super) fn assignment_has_failure(assignment: &Assignment, name: &NodeName) -> bool {
    let mut has_signal = false;
    let mut signal_occurrences = 0_u64;
    for (key, value) in assignment.iter().filter(|(key, _)| key.name == *name) {
        match key.source {
            ControlSourceKey::Error if value.counts.values().any(|count| *count > 0) => {
                return true;
            }
            ControlSourceKey::Group
                if is_parallel_group_control(key)
                    && value.counts.iter().any(|(label, count)| {
                        *count > 0
                            && matches!(label.as_str(), "quorum_unreachable" | "no_satisfier")
                    }) =>
            {
                return true;
            }
            ControlSourceKey::Signal => {
                has_signal = true;
                signal_occurrences =
                    signal_occurrences.saturating_add(value.counts.values().copied().sum::<u64>());
            }
            ControlSourceKey::Error | ControlSourceKey::Group | ControlSourceKey::Execution => {}
        }
    }
    has_signal && signal_occurrences == 0
}

fn is_parallel_group_control(key: &SelectorKey) -> bool {
    key.source == ControlSourceKey::Group
        && key
            .field
            .as_ref()
            .is_some_and(|field| matches!(field.as_str(), "joined" | "raced"))
}

pub(super) fn build_dimensions(
    selectors: &[ControlSelector],
    nodes: &BTreeMap<NodeName, NodeInfo<'_>>,
    map_aggregates: &BTreeMap<SelectorKey, MapAggregate>,
    join_correlations: &BTreeMap<NodeName, ParallelJoinCorrelation>,
    execution_correlations: &BTreeMap<NodeName, MapExecutionCorrelation>,
) -> Option<Vec<Vec<Assignment>>> {
    let mut by_node = BTreeMap::<NodeName, BTreeSet<SelectorKey>>::new();
    for selector in selectors {
        by_node
            .entry(selector.name.clone())
            .or_default()
            .insert(SelectorKey::from(selector));
    }
    let mut aggregate_scopes =
        BTreeMap::<NodeName, (u64, BTreeMap<NodeName, Vec<SelectorKey>>)>::new();
    for (name, keys) in &by_node {
        for key in keys {
            let Some(aggregate) = map_aggregates.get(key) else {
                continue;
            };
            let (maximum, scope_nodes) = aggregate_scopes
                .entry(aggregate.owner.clone())
                .or_insert_with(|| (aggregate.maximum, BTreeMap::new()));
            if *maximum != aggregate.maximum {
                return None;
            }
            scope_nodes
                .entry(name.clone())
                .or_default()
                .push(key.clone());
        }
    }
    let shared_by_scope = aggregate_scopes
        .iter()
        .map(|(owner, (_, scope_nodes))| {
            let mut shared = aggregate_shared_keys(scope_nodes, map_aggregates, join_correlations);
            shared.extend(aggregate_shared_execution_keys(
                scope_nodes,
                map_aggregates,
                execution_correlations,
            ));
            (owner.clone(), shared)
        })
        .collect::<BTreeMap<_, _>>();
    let shared_keys = shared_by_scope
        .values()
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();

    let mut dimensions = Vec::new();
    for (name, keys) in &by_node {
        let Some(node) = nodes.get(name).map(|info| info.node) else {
            continue;
        };
        let standalone = keys
            .difference(&shared_keys)
            .cloned()
            .collect::<BTreeSet<_>>();
        dimensions.extend(scalar_dimensions_for_node(
            node,
            &standalone,
            map_aggregates,
        )?);
    }
    for (owner, (maximum, scope_nodes)) in aggregate_scopes {
        dimensions.extend(aggregate_scope_dimensions(
            nodes,
            &scope_nodes,
            maximum,
            join_correlations,
            execution_correlations,
            shared_by_scope.get(&owner).cloned().unwrap_or_default(),
        )?);
    }
    Some(dimensions)
}

pub(super) fn enumerate_assignments(dimensions: Vec<Vec<Assignment>>) -> Vec<Assignment> {
    let mut assignments = vec![Assignment::new()];
    for dimension in dimensions {
        let capacity = assignments
            .len()
            .saturating_mul(dimension.len())
            .min(FULL_V1_MAX_GUARD_ASSIGNMENTS as usize);
        let mut next = Vec::with_capacity(capacity);
        for prefix in &assignments {
            for choice in &dimension {
                if !assignments_are_compatible(prefix, choice) {
                    continue;
                }
                let mut assignment = prefix.clone();
                assignment.extend(choice.clone());
                next.push(assignment);
            }
        }
        assignments = next;
    }
    assignments
}

pub(super) fn compatible_assignment_count_is_bounded(
    dimensions: &[Vec<Assignment>],
    maximum: u64,
) -> bool {
    let mut assignments = vec![Assignment::new()];
    for dimension in dimensions {
        let mut next = Vec::new();
        for prefix in &assignments {
            for choice in dimension {
                if !assignments_are_compatible(prefix, choice) {
                    continue;
                }
                if next.len() as u64 >= maximum {
                    return false;
                }
                let mut assignment = prefix.clone();
                assignment.extend(choice.clone());
                next.push(assignment);
            }
        }
        assignments = next;
    }
    true
}

fn assignments_are_compatible(left: &Assignment, right: &Assignment) -> bool {
    right
        .iter()
        .all(|(key, value)| left.get(key).is_none_or(|existing| existing == value))
}

pub(super) fn scalar_dimensions_for_node(
    node: &GraphNode,
    keys: &BTreeSet<SelectorKey>,
    map_aggregates: &BTreeMap<SelectorKey, MapAggregate>,
) -> Option<Vec<Vec<Assignment>>> {
    let mut dimensions = Vec::new();
    let scalar = keys
        .iter()
        .filter(|key| !map_aggregates.contains_key(*key))
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

fn aggregate_scope_dimensions(
    nodes: &BTreeMap<NodeName, NodeInfo<'_>>,
    scope_nodes: &BTreeMap<NodeName, Vec<SelectorKey>>,
    maximum: u64,
    join_correlations: &BTreeMap<NodeName, ParallelJoinCorrelation>,
    execution_correlations: &BTreeMap<NodeName, MapExecutionCorrelation>,
    shared_keys: BTreeSet<SelectorKey>,
) -> Option<Vec<Vec<Assignment>>> {
    let scope_keys = scope_nodes.values().flatten().cloned().collect::<Vec<_>>();
    let scope_key_set = scope_keys.iter().cloned().collect::<BTreeSet<_>>();
    let available_keys = scope_key_set
        .union(&shared_keys)
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut components = scope_nodes
        .keys()
        .cloned()
        .map(|name| BTreeSet::from([name]))
        .collect::<Vec<_>>();
    for (name, correlation) in join_correlations {
        if !correlation_is_available_to_scope(name, correlation, &scope_key_set, &available_keys) {
            continue;
        }
        let mut correlated = BTreeSet::from([name.clone()]);
        let mut guards = Vec::new();
        correlation.collect_guards(&mut guards);
        correlated.extend(
            guards
                .into_iter()
                .flat_map(guard_selectors)
                .filter(|selector| scope_key_set.contains(&SelectorKey::from(*selector)))
                .map(|selector| selector.name.clone()),
        );
        merge_assignment_components(&mut components, correlated);
    }
    let always_present = scope_nodes
        .keys()
        .filter(|name| {
            execution_correlations
                .get(*name)
                .is_some_and(|correlation| {
                    matches!(correlation.presence, CompletionPredicate::Always)
                })
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    if !always_present.is_empty() {
        merge_assignment_components(&mut components, always_present);
    }
    for (name, correlation) in execution_correlations {
        if !scope_nodes.contains_key(name) {
            continue;
        }
        let mut correlated = BTreeSet::from([name.clone()]);
        let mut guards = Vec::new();
        correlation.presence.collect_guards(&mut guards);
        correlated.extend(
            guards
                .into_iter()
                .flat_map(guard_selectors)
                .filter(|selector| scope_key_set.contains(&SelectorKey::from(*selector)))
                .map(|selector| selector.name.clone()),
        );
        merge_assignment_components(&mut components, correlated);
    }
    components.sort_by(|left, right| left.iter().next().cmp(&right.iter().next()));
    if shared_keys.is_empty() {
        return components
            .into_iter()
            .map(|component| {
                aggregate_component_dimension(
                    &AggregateComponentContext {
                        nodes,
                        scope_nodes,
                        maximum,
                        join_correlations,
                        execution_correlations,
                        shared_assignment: &Assignment::new(),
                        available_keys: &scope_key_set,
                    },
                    &component,
                )
            })
            .collect();
    }

    let shared_dimensions = dimensions_for_keys(nodes, &shared_keys)?;
    if !dimension_product_is_bounded(&shared_dimensions) {
        return None;
    }
    let shared_assignments = enumerate_assignments(shared_dimensions);
    let mut combined = Vec::new();
    for shared in shared_assignments {
        let local_dimensions = components
            .iter()
            .map(|component| {
                aggregate_component_dimension(
                    &AggregateComponentContext {
                        nodes,
                        scope_nodes,
                        maximum,
                        join_correlations,
                        execution_correlations,
                        shared_assignment: &shared,
                        available_keys: &available_keys,
                    },
                    component,
                )
            })
            .collect::<Option<Vec<_>>>()?;
        if !dimension_product_is_bounded(&local_dimensions) {
            return None;
        }
        for local in enumerate_assignments(local_dimensions) {
            if combined.len() as u64 >= FULL_V1_MAX_GUARD_ASSIGNMENTS {
                return None;
            }
            let mut assignment = shared.clone();
            assignment.extend(local);
            combined.push(assignment);
        }
    }
    Some(vec![combined])
}

fn dimensions_for_keys(
    nodes: &BTreeMap<NodeName, NodeInfo<'_>>,
    keys: &BTreeSet<SelectorKey>,
) -> Option<Vec<Vec<Assignment>>> {
    let mut by_node = BTreeMap::<NodeName, BTreeSet<SelectorKey>>::new();
    for key in keys {
        by_node
            .entry(key.name.clone())
            .or_default()
            .insert(key.clone());
    }
    let no_aggregates = BTreeMap::new();
    by_node
        .into_iter()
        .try_fold(Vec::new(), |mut dimensions, (name, keys)| {
            let node = nodes.get(&name)?.node;
            dimensions.extend(scalar_dimensions_for_node(node, &keys, &no_aggregates)?);
            Some(dimensions)
        })
}

fn aggregate_shared_keys(
    scope_nodes: &BTreeMap<NodeName, Vec<SelectorKey>>,
    map_aggregates: &BTreeMap<SelectorKey, MapAggregate>,
    join_correlations: &BTreeMap<NodeName, ParallelJoinCorrelation>,
) -> BTreeSet<SelectorKey> {
    let scope_keys = scope_nodes
        .values()
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut available = scope_keys.clone();
    let mut shared = BTreeSet::new();
    loop {
        let mut discovered = false;
        for (name, correlation) in join_correlations {
            if !correlation_control_is_present(name, correlation, &available) {
                continue;
            }
            let mut guards = Vec::new();
            correlation.collect_guards(&mut guards);
            for dependency in guards
                .into_iter()
                .flat_map(guard_selectors)
                .map(SelectorKey::from)
                .filter(|key| !map_aggregates.contains_key(key))
            {
                discovered |= shared.insert(dependency.clone());
                available.insert(dependency);
            }
        }
        if !discovered {
            break;
        }
    }
    shared
}

fn aggregate_shared_execution_keys(
    scope_nodes: &BTreeMap<NodeName, Vec<SelectorKey>>,
    map_aggregates: &BTreeMap<SelectorKey, MapAggregate>,
    execution_correlations: &BTreeMap<NodeName, MapExecutionCorrelation>,
) -> BTreeSet<SelectorKey> {
    scope_nodes
        .keys()
        .filter_map(|name| execution_correlations.get(name))
        .flat_map(|correlation| {
            let mut guards = Vec::new();
            correlation.presence.collect_guards(&mut guards);
            guards
        })
        .flat_map(guard_selectors)
        .map(SelectorKey::from)
        .filter(|key| !map_aggregates.contains_key(key))
        .collect()
}

fn merge_assignment_components(
    components: &mut Vec<BTreeSet<NodeName>>,
    mut correlated: BTreeSet<NodeName>,
) {
    let mut independent = Vec::new();
    for component in components.drain(..) {
        if component.is_disjoint(&correlated) {
            independent.push(component);
        } else {
            correlated.extend(component);
        }
    }
    independent.push(correlated);
    *components = independent;
}

struct AggregateComponentContext<'a, 'graph> {
    nodes: &'a BTreeMap<NodeName, NodeInfo<'graph>>,
    scope_nodes: &'a BTreeMap<NodeName, Vec<SelectorKey>>,
    maximum: u64,
    join_correlations: &'a BTreeMap<NodeName, ParallelJoinCorrelation>,
    execution_correlations: &'a BTreeMap<NodeName, MapExecutionCorrelation>,
    shared_assignment: &'a Assignment,
    available_keys: &'a BTreeSet<SelectorKey>,
}

fn aggregate_component_dimension(
    context: &AggregateComponentContext<'_, '_>,
    component: &BTreeSet<NodeName>,
) -> Option<Vec<Assignment>> {
    let keys = component
        .iter()
        .flat_map(|name| context.scope_nodes.get(name).into_iter().flatten())
        .cloned()
        .collect::<Vec<_>>();
    let key_set = keys.iter().cloned().collect::<BTreeSet<_>>();
    let mut item_dimensions = Vec::new();
    for name in component {
        let node_keys = context.scope_nodes.get(name)?;
        let node = context.nodes.get(name)?.node;
        let mut outcome_keys = node_keys.clone();
        if context.execution_correlations.contains_key(name) {
            outcome_keys.push(execution_key(name));
        }
        let mut outcomes = per_execution_outcomes(node, &outcome_keys)?;
        outcomes.push(empty_outcome(&outcome_keys));
        item_dimensions.push(outcomes);
    }
    if !dimension_product_is_bounded(&item_dimensions) {
        return None;
    }
    let mut item_outcomes = enumerate_assignments(item_dimensions);
    item_outcomes.retain(|assignment| {
        if assignment_is_empty(assignment) {
            return false;
        }
        let mut correlated = context.shared_assignment.clone();
        correlated.extend(assignment.clone());
        context.join_correlations.iter().all(|(name, correlation)| {
            !correlation_is_available_to_scope(name, correlation, &key_set, context.available_keys)
                || assignment_matches_parallel_join_correlation(&correlated, name, correlation)
        }) && component.iter().all(|name| {
            context
                .execution_correlations
                .get(name)
                .is_none_or(|correlation| {
                    assignment_matches_map_execution_correlation(&correlated, name, correlation)
                })
        })
    });
    for assignment in &mut item_outcomes {
        assignment.retain(|key, _| key.source != ControlSourceKey::Execution);
    }
    item_outcomes.retain(|assignment| !assignment_is_empty(assignment));
    item_outcomes = item_outcomes
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    aggregate_assignments_from_outcomes(&keys, &item_outcomes, context.maximum)
}

fn assignment_matches_map_execution_correlation(
    assignment: &Assignment,
    name: &NodeName,
    correlation: &MapExecutionCorrelation,
) -> bool {
    let executed = assignment
        .get(&execution_key(name))
        .is_some_and(|control| control.counts.values().any(|count| *count > 0));
    executed == correlation.presence.evaluate(assignment)
}

fn execution_key(name: &NodeName) -> SelectorKey {
    SelectorKey {
        name: name.clone(),
        source: ControlSourceKey::Execution,
        field: None,
    }
}

fn empty_outcome(keys: &[SelectorKey]) -> Assignment {
    keys.iter()
        .cloned()
        .map(|key| (key, assigned_control(None, 0)))
        .collect()
}

fn assignment_is_empty(assignment: &Assignment) -> bool {
    assignment
        .values()
        .all(|control| control.counts.values().all(|count| *count == 0))
}

fn dimension_product_is_bounded(dimensions: &[Vec<Assignment>]) -> bool {
    dimensions
        .iter()
        .try_fold(1_u64, |product, dimension| {
            product
                .checked_mul(u64::try_from(dimension.len()).ok()?)
                .filter(|product| *product <= FULL_V1_MAX_GUARD_ASSIGNMENTS)
        })
        .is_some()
}

fn correlation_control_is_present(
    name: &NodeName,
    correlation: &ParallelJoinCorrelation,
    keys: &BTreeSet<SelectorKey>,
) -> bool {
    keys.iter().any(|key| {
        key.name == *name
            && key.source == ControlSourceKey::Group
            && key
                .field
                .as_ref()
                .is_some_and(|field| field.as_str() == correlation.field())
    })
}

fn correlation_is_available_to_scope(
    name: &NodeName,
    correlation: &ParallelJoinCorrelation,
    control_keys: &BTreeSet<SelectorKey>,
    available_keys: &BTreeSet<SelectorKey>,
) -> bool {
    if !correlation_control_is_present(name, correlation, control_keys) {
        return false;
    }
    let mut guards = Vec::new();
    correlation.collect_guards(&mut guards);
    guards
        .into_iter()
        .flat_map(guard_selectors)
        .map(SelectorKey::from)
        .all(|key| available_keys.contains(&key))
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

fn aggregate_assignments_from_outcomes(
    keys: &[SelectorKey],
    outcomes: &[Assignment],
    maximum: u64,
) -> Option<Vec<Assignment>> {
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
        let Some(assignment) = combine_outcome_occurrences(keys, outcomes, outcome_occurrences)
        else {
            valid = false;
            return false;
        };
        assignments.push(assignment);
        true
    });
    (completed && valid && assignments.len() == expected).then_some(assignments)
}

pub(super) fn assignment_matches_parallel_join_correlation(
    assignment: &Assignment,
    name: &NodeName,
    correlation: &ParallelJoinCorrelation,
) -> bool {
    let Some(control) = assignment.iter().find_map(|(key, value)| {
        (key.name == *name
            && key.source == ControlSourceKey::Group
            && key
                .field
                .as_ref()
                .is_some_and(|field| field.as_str() == correlation.field()))
        .then_some(value)
    }) else {
        return true;
    };
    if control.counts.values().all(|count| *count == 0) {
        return true;
    }
    match correlation {
        ParallelJoinCorrelation::Joined { completion } => {
            let reports_reached = control
                .counts
                .iter()
                .any(|(label, count)| label.as_str() == "reached" && *count > 0);
            let reports_unreachable = control
                .counts
                .iter()
                .any(|(label, count)| label.as_str() == "quorum_unreachable" && *count > 0);
            let completed = completion.evaluate(assignment);
            (reports_reached && completed) || (reports_unreachable && !completed)
        }
        ParallelJoinCorrelation::First {
            satisfaction,
            all_completions,
        } => {
            let reports_satisfied = control
                .counts
                .iter()
                .any(|(label, count)| label.as_str() == "satisfied" && *count > 0);
            let reports_no_satisfier = control
                .counts
                .iter()
                .any(|(label, count)| label.as_str() == "no_satisfier" && *count > 0);
            let satisfied = satisfaction.evaluate(assignment);
            (reports_satisfied && satisfied)
                || (reports_no_satisfier && !satisfied && all_completions.evaluate(assignment))
        }
    }
}

pub(super) fn per_execution_outcomes(
    node: &GraphNode,
    keys: &[SelectorKey],
) -> Option<Vec<Assignment>> {
    let emitted_keys = keys
        .iter()
        .filter(|key| {
            matches!(
                key.source,
                ControlSourceKey::Signal | ControlSourceKey::Group
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let error_keys = keys
        .iter()
        .filter(|key| key.source == ControlSourceKey::Error)
        .cloned()
        .collect::<Vec<_>>();
    let execution_keys = keys
        .iter()
        .filter(|key| key.source == ControlSourceKey::Execution)
        .cloned()
        .collect::<Vec<_>>();
    let mut successful_count = 1_u64;
    for key in &emitted_keys {
        let label_count = u64::try_from(selector_key_domain(key, node).len()).ok()?;
        successful_count = successful_count.checked_mul(label_count)?;
        if successful_count > FULL_V1_MAX_GUARD_ASSIGNMENTS {
            return None;
        }
    }
    let runtime_errors = matches!(node, GraphNode::Step(_) | GraphNode::Verifier(_))
        .then(error_labels)
        .unwrap_or_default();
    let total_outcomes = successful_count.checked_add(u64::try_from(runtime_errors.len()).ok()?)?;
    if total_outcomes > FULL_V1_MAX_GUARD_ASSIGNMENTS {
        return None;
    }
    let mut successful = vec![Assignment::new()];
    for key in &emitted_keys {
        let labels = selector_key_domain(key, node);
        successful = cartesian_emitted_outcomes(successful, key, &labels);
    }
    for assignment in &mut successful {
        insert_empty_controls(assignment, &error_keys);
        insert_execution_controls(assignment, &execution_keys);
    }
    let mut outcomes = successful;
    for error in runtime_errors {
        let mut assignment = Assignment::new();
        insert_empty_controls(&mut assignment, &emitted_keys);
        insert_execution_controls(&mut assignment, &execution_keys);
        for key in &error_keys {
            assignment.insert(key.clone(), assigned_control(Some(error.clone()), 1));
        }
        outcomes.push(assignment);
    }
    outcomes.dedup();
    Some(outcomes)
}

fn insert_execution_controls(assignment: &mut Assignment, keys: &[SelectorKey]) {
    for key in keys {
        assignment.insert(
            key.clone(),
            assigned_control(Some(enum_label("executed")), 1),
        );
    }
}

pub(super) fn cartesian_emitted_outcomes(
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
        ControlSourceKey::Execution => vec![enum_label("executed")],
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
