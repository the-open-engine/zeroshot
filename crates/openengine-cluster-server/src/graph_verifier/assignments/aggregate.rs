use super::*;

pub(super) fn aggregate_scope_dimensions(
    inputs: DimensionInputs<'_, '_>,
    scope_nodes: &BTreeMap<NodeName, Vec<SelectorKey>>,
    shared_keys: BTreeSet<SelectorKey>,
) -> Option<Vec<Vec<Assignment>>> {
    let scope_keys = scope_nodes.values().flatten().cloned().collect::<Vec<_>>();
    let scope_key_set = scope_keys.iter().cloned().collect::<BTreeSet<_>>();
    let available_keys = scope_key_set
        .union(&shared_keys)
        .cloned()
        .collect::<BTreeSet<_>>();
    let components = aggregate_components(inputs, scope_nodes, &scope_key_set, &available_keys);
    if shared_keys.is_empty() {
        let context = AggregateScopeContext {
            inputs,
            scope_nodes,
            available_keys: &scope_key_set,
        };
        return component_dimensions(&context, &components, &Assignment::new());
    }

    let shared_dimensions =
        dimensions_for_keys(inputs.nodes, &shared_keys, inputs.budget, inputs.runtime)?;
    if !dimension_product_is_bounded(&shared_dimensions) {
        return None;
    }
    let context = AggregateScopeContext {
        inputs,
        scope_nodes,
        available_keys: &available_keys,
    };
    combine_shared_dimensions(&context, &components, shared_dimensions)
}

fn aggregate_components(
    inputs: DimensionInputs<'_, '_>,
    scope_nodes: &BTreeMap<NodeName, Vec<SelectorKey>>,
    scope_keys: &BTreeSet<SelectorKey>,
    available_keys: &BTreeSet<SelectorKey>,
) -> Vec<BTreeSet<NodeName>> {
    let mut components = scope_nodes
        .keys()
        .cloned()
        .map(|name| BTreeSet::from([name]))
        .collect::<Vec<_>>();
    correlate_join_components(
        inputs.join_correlations,
        scope_keys,
        available_keys,
        &mut components,
    );
    correlate_execution_components(
        inputs.execution_correlations,
        scope_nodes,
        scope_keys,
        &mut components,
    );
    components.sort_by(|left, right| left.iter().next().cmp(&right.iter().next()));
    components
}

fn correlate_join_components(
    correlations: &BTreeMap<NodeName, ParallelJoinCorrelation>,
    scope_keys: &BTreeSet<SelectorKey>,
    available_keys: &BTreeSet<SelectorKey>,
    components: &mut Vec<BTreeSet<NodeName>>,
) {
    for (name, correlation) in correlations {
        if !correlation_is_available_to_scope(name, correlation, scope_keys, available_keys) {
            continue;
        }
        let mut guards = Vec::new();
        correlation.collect_guards(&mut guards);
        merge_guard_component(name, guards, scope_keys, components);
    }
}

fn correlate_execution_components(
    correlations: &BTreeMap<NodeName, MapExecutionCorrelation>,
    scope_nodes: &BTreeMap<NodeName, Vec<SelectorKey>>,
    scope_keys: &BTreeSet<SelectorKey>,
    components: &mut Vec<BTreeSet<NodeName>>,
) {
    let always_present = scope_nodes
        .keys()
        .filter(|name| {
            correlations.get(*name).is_some_and(|correlation| {
                matches!(correlation.presence, CompletionPredicate::Always)
            })
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    if !always_present.is_empty() {
        merge_assignment_components(components, always_present);
    }
    for (name, correlation) in correlations {
        if !scope_nodes.contains_key(name) {
            continue;
        }
        let mut guards = Vec::new();
        correlation.collect_guards(&mut guards);
        merge_guard_component(name, guards, scope_keys, components);
    }
}

fn merge_guard_component(
    name: &NodeName,
    guards: Vec<&Guard>,
    scope_keys: &BTreeSet<SelectorKey>,
    components: &mut Vec<BTreeSet<NodeName>>,
) {
    let mut correlated = BTreeSet::from([name.clone()]);
    correlated.extend(scoped_guard_names(guards, scope_keys));
    merge_assignment_components(components, correlated);
}

fn scoped_guard_names<'a>(
    guards: Vec<&'a Guard>,
    scope_keys: &'a BTreeSet<SelectorKey>,
) -> impl Iterator<Item = NodeName> + 'a {
    guards
        .into_iter()
        .flat_map(guard_selectors)
        .filter(|selector| scope_keys.contains(&SelectorKey::from(*selector)))
        .map(|selector| selector.name.clone())
}

struct AggregateScopeContext<'a, 'graph> {
    inputs: DimensionInputs<'a, 'graph>,
    scope_nodes: &'a BTreeMap<NodeName, Vec<SelectorKey>>,
    available_keys: &'a BTreeSet<SelectorKey>,
}

fn component_dimensions(
    context: &AggregateScopeContext<'_, '_>,
    components: &[BTreeSet<NodeName>],
    shared_assignment: &Assignment,
) -> Option<Vec<Vec<Assignment>>> {
    components
        .iter()
        .map(|component| {
            aggregate_component_dimension(
                &AggregateComponentContext {
                    inputs: context.inputs,
                    scope_nodes: context.scope_nodes,
                    shared_assignment,
                    available_keys: context.available_keys,
                },
                component,
            )
        })
        .collect()
}

fn combine_shared_dimensions(
    context: &AggregateScopeContext<'_, '_>,
    components: &[BTreeSet<NodeName>],
    shared_dimensions: Vec<Vec<Assignment>>,
) -> Option<Vec<Vec<Assignment>>> {
    let mut combined = Vec::new();
    for shared in enumerate_assignments(shared_dimensions, context.inputs.budget)? {
        let local_dimensions = component_dimensions(context, components, &shared)?;
        if !dimension_product_is_bounded(&local_dimensions) {
            return None;
        }
        for local in enumerate_assignments(local_dimensions, context.inputs.budget)? {
            if u64::try_from(combined.len()).unwrap_or(u64::MAX) >= FULL_V1_MAX_GUARD_ASSIGNMENTS {
                return None;
            }
            let mut assignment = shared.clone();
            assignment.extend(local);
            combined.push(assignment);
        }
    }
    Some(vec![combined])
}

pub(super) fn merge_assignment_components(
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

pub(super) struct AggregateComponentContext<'a, 'graph> {
    pub(super) inputs: DimensionInputs<'a, 'graph>,
    pub(super) scope_nodes: &'a BTreeMap<NodeName, Vec<SelectorKey>>,
    pub(super) shared_assignment: &'a Assignment,
    pub(super) available_keys: &'a BTreeSet<SelectorKey>,
}

pub(super) fn aggregate_component_dimension(
    context: &AggregateComponentContext<'_, '_>,
    component: &BTreeSet<NodeName>,
) -> Option<Vec<Assignment>> {
    let keys = component_keys(context.scope_nodes, component);
    let mut item_outcomes = component_item_outcomes(context, component, None)?;
    item_outcomes.retain(|assignment| !assignment_is_empty(assignment));
    aggregate_assignments_from_outcomes(AggregateCounts {
        keys: &keys,
        outcomes: &item_outcomes,
        saturation: context.inputs.saturation,
        budget: context.inputs.budget,
        exact_occurrences: None,
    })
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
        && correlation
            .item_predicates
            .iter()
            .all(|(field, predicate)| {
                assignment
                    .get(&SelectorKey {
                        name: name.clone(),
                        source: ControlSourceKey::Group,
                        field: Some(field.clone()),
                    })
                    .is_none_or(|control| {
                        control_reports_completion(control, predicate.evaluate(assignment))
                    })
            })
}

fn execution_key(name: &NodeName) -> SelectorKey {
    SelectorKey {
        name: name.clone(),
        source: ControlSourceKey::Execution,
        field: None,
    }
}

pub(super) fn empty_outcome(keys: &[SelectorKey]) -> Assignment {
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

pub(super) fn dimension_product_is_bounded(dimensions: &[Vec<Assignment>]) -> bool {
    dimensions
        .iter()
        .try_fold(1_u64, |product, dimension| {
            product
                .checked_mul(u64::try_from(dimension.len()).ok()?)
                .filter(|product| *product <= FULL_V1_MAX_GUARD_ASSIGNMENTS)
        })
        .is_some()
}

pub(super) fn correlation_control_is_present(
    name: &NodeName,
    correlation: &ParallelJoinCorrelation,
    keys: &BTreeSet<SelectorKey>,
) -> bool {
    keys.iter().any(|key| {
        key.name == *name
            && key.source == ControlSourceKey::Group
            && key.field.as_ref().is_some_and(|field| {
                field.as_str() == correlation.field() || retention_index(Some(field)).is_some()
            })
    })
}

pub(super) fn correlation_is_available_to_scope(
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

pub(super) fn normalize_outcome_labels(
    outcomes: &mut [Assignment],
    classes: &BTreeMap<SelectorKey, BTreeMap<EnumLabel, EnumLabel>>,
) -> Option<()> {
    for outcome in outcomes {
        for (key, control) in outcome {
            let Some(labels) = classes.get(key) else {
                continue;
            };
            let mut counts = BTreeMap::new();
            for (label, count) in &control.counts {
                *counts.entry(labels.get(label)?.clone()).or_insert(0) += count;
            }
            control.counts = counts;
        }
    }
    Some(())
}

fn component_item_outcomes(
    context: &AggregateComponentContext<'_, '_>,
    component: &BTreeSet<NodeName>,
    runtime: Option<RuntimeScope<'_>>,
) -> Option<Vec<Assignment>> {
    let keys = component_keys(context.scope_nodes, component);
    let key_set = keys.iter().cloned().collect::<BTreeSet<_>>();
    let mut item_dimensions = Vec::new();
    for name in component {
        item_dimensions.push(item_node_dimension(context, name, runtime)?);
    }
    if !dimension_product_is_bounded(&item_dimensions) {
        return None;
    }
    let mut item_outcomes = enumerate_assignments(item_dimensions, context.inputs.budget)?;
    item_outcomes
        .retain(|assignment| item_assignment_matches(context, component, &key_set, assignment));
    for assignment in &mut item_outcomes {
        assignment.retain(|key, _| key.source != ControlSourceKey::Execution);
    }
    normalize_outcome_labels(&mut item_outcomes, context.inputs.label_classes)?;
    item_outcomes = item_outcomes
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Some(item_outcomes)
}

pub(super) fn add_count_dimensions(
    left: &[Assignment],
    right: &[Assignment],
    inputs: DimensionInputs<'_, '_>,
) -> Option<Vec<Assignment>> {
    let mut results = BTreeSet::new();
    for prefix in left {
        for suffix in right {
            inputs.budget.charge(
                1 + prefix.len()
                    + suffix
                        .values()
                        .map(|control| 1 + control.counts.len())
                        .sum::<usize>(),
            )?;
            let mut next = prefix.clone();
            for (key, control) in suffix {
                let target = next
                    .entry(key.clone())
                    .or_insert_with(|| assigned_control(None, 0));
                for (label, count) in &control.counts {
                    let value = target.counts.entry(label.clone()).or_default();
                    *value = value.saturating_add(*count).min(inputs.saturation);
                }
            }
            if !results.contains(&next) && results.len() as u64 >= FULL_V1_MAX_GUARD_ASSIGNMENTS {
                return None;
            }
            results.insert(next);
        }
    }
    Some(results.into_iter().collect())
}

pub(super) fn item_node_dimension(
    context: &AggregateComponentContext<'_, '_>,
    name: &NodeName,
    runtime: Option<RuntimeScope<'_>>,
) -> Option<Vec<Assignment>> {
    let node_keys = context.scope_nodes.get(name)?;
    let node = context.inputs.nodes.get(name)?.node;
    let mut outcome_keys = node_keys.clone();
    if context.inputs.execution_correlations.contains_key(name) {
        outcome_keys.push(execution_key(name));
    }
    let markers = outcome_keys
        .iter()
        .filter(|key| is_map_item_predicate(key))
        .cloned()
        .collect::<Vec<_>>();
    outcome_keys.retain(|key| !is_map_item_predicate(key));
    let mut outcomes =
        if let Some(fixed) = runtime.and_then(|scope| fixed_outcome(name, &outcome_keys, scope)) {
            vec![fixed]
        } else {
            unfixed_item_outcomes(context, name, &outcome_keys)?
        };
    for marker in markers {
        context
            .inputs
            .budget
            .charge(outcomes.len().checked_mul(2 * (1 + outcome_keys.len()))?)?;
        outcomes = cartesian_emitted_outcomes(
            outcomes,
            &marker,
            &proof_group_domain(node, marker.field.as_ref())?,
        );
    }
    Some(
        outcomes
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    )
}

pub(super) fn item_assignment_matches(
    context: &AggregateComponentContext<'_, '_>,
    component: &BTreeSet<NodeName>,
    key_set: &BTreeSet<SelectorKey>,
    assignment: &Assignment,
) -> bool {
    let mut correlated = context.shared_assignment.clone();
    correlated.extend(assignment.clone());
    context
        .inputs
        .join_correlations
        .iter()
        .all(|(name, correlation)| {
            !correlation_is_available_to_scope(name, correlation, key_set, context.available_keys)
                || assignment_matches_parallel_join_correlation(&correlated, name, correlation)
        })
        && component.iter().all(|name| {
            let loop_matches = match context.inputs.nodes.get(name).map(|info| info.node) {
                Some(GraphNode::Loop(group)) => loop_outcome_matches(group, &correlated),
                _ => true,
            };
            loop_matches
                && context
                    .inputs
                    .execution_correlations
                    .get(name)
                    .is_none_or(|correlation| {
                        assignment_matches_map_execution_correlation(&correlated, name, correlation)
                    })
        })
}

fn component_keys(
    scope_nodes: &BTreeMap<NodeName, Vec<SelectorKey>>,
    component: &BTreeSet<NodeName>,
) -> Vec<SelectorKey> {
    component
        .iter()
        .flat_map(|name| scope_nodes.get(name).into_iter().flatten())
        .cloned()
        .collect()
}

fn control_reports_completion(control: &AssignedControl, complete: bool) -> bool {
    let expected = if complete { "complete" } else { "blocked" };
    control
        .counts
        .iter()
        .any(|(label, count)| label.as_str() == expected && *count > 0)
}

fn unfixed_item_outcomes(
    context: &AggregateComponentContext<'_, '_>,
    name: &NodeName,
    keys: &[SelectorKey],
) -> Option<Vec<Assignment>> {
    let mut outcomes = per_execution_outcomes(
        context.inputs.nodes.get(name)?.node,
        keys,
        context.inputs.budget,
    )?;
    if context.inputs.execution_correlations.contains_key(name) {
        outcomes.push(empty_outcome(keys));
    }
    Some(outcomes)
}
