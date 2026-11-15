use super::*;

pub(super) struct AggregateComponentContext<'a, 'graph> {
    pub(super) inputs: DimensionInputs<'a, 'graph>,
    pub(super) owner: Option<&'a NodeName>,
    pub(super) scope_nodes: &'a BTreeMap<NodeName, Vec<SelectorKey>>,
    pub(super) shared_assignment: &'a Assignment,
    pub(super) available_keys: &'a BTreeSet<SelectorKey>,
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

pub(super) fn item_node_dimension(
    context: &AggregateComponentContext<'_, '_>,
    name: &NodeName,
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
    let mut outcomes = unfixed_item_outcomes(context, name, &outcome_keys)?;
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

/// Qualification needs every executed racer's private facts. Only after validating that joint
/// assignment may losing branch controls disappear from the item exported to its parent.
pub(super) fn project_retained_controls(
    outcomes: &mut [Assignment],
    context: &AggregateComponentContext<'_, '_>,
) -> Option<()> {
    for outcome in outcomes {
        context
            .inputs
            .budget
            .charge(1 + outcome.len() + context.shared_assignment.len())?;
        let mut private = context.shared_assignment.clone();
        private.extend(outcome.clone());
        let discarded = outcome
            .keys()
            .filter(|key| {
                key.source != ControlSourceKey::Execution
                    && context
                        .inputs
                        .execution_correlations
                        .get(&key.name)
                        .and_then(|correlation| scoped_retention(context, correlation))
                        .is_some_and(|retention| !retention.evaluate(&private))
            })
            .cloned()
            .collect::<Vec<_>>();
        for key in discarded {
            outcome.insert(key, assigned_control(None, 0));
        }
    }
    Some(())
}

// Nested facts are masked only at the scope that owns their retaining join. Child scopes
// keep private evidence until their parent's joint candidate qualification has completed.
fn scoped_retention<'a>(
    context: &AggregateComponentContext<'a, '_>,
    mut correlation: &'a MapExecutionCorrelation,
) -> Option<&'a CompletionPredicate> {
    while Some(&correlation.owner) != context.owner {
        correlation = context
            .inputs
            .execution_correlations
            .get(&correlation.owner)?;
    }
    Some(&correlation.retention)
}
