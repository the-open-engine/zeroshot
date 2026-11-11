//! Static correlated counts at nested map scopes. Each child folds into its parent item.
use super::*;

#[derive(Default)]
struct ScopePlan<'a> {
    local: BTreeMap<NodeName, Vec<SelectorKey>>,
    maps: Vec<(&'a MapNode, ScopePlan<'a>)>,
    keys: BTreeSet<SelectorKey>,
}

pub(super) fn dimensions(
    selectors: &[ControlSelector],
    inputs: DimensionInputs<'_, '_>,
) -> Option<Vec<Vec<Assignment>>> {
    let mut plan = ScopePlan::default();
    for selector in selectors {
        inputs.budget.charge(
            1 + inputs
                .nodes
                .get(&selector.name)
                .map_or(0, |info| info.path.len()),
        )?;
        let key = SelectorKey::from(selector);
        let chain = map_chain(&key, inputs);
        insert_key(&mut plan, &chain, key);
    }
    sort_maps(&mut plan, inputs);
    Some(vec![scope_outcomes(&plan, inputs, &Assignment::new())?])
}

fn map_chain<'a>(key: &SelectorKey, inputs: DimensionInputs<'_, 'a>) -> Vec<&'a MapNode> {
    if !inputs.map_aggregates.contains_key(key) {
        return vec![];
    }
    inputs
        .nodes
        .get(&key.name)
        .map(|info| {
            info.path
                .iter()
                .filter_map(|part| {
                    let DiagnosticPathSegment::Node { name } = part else {
                        return None;
                    };
                    if name == &key.name {
                        return None;
                    }
                    match inputs.nodes.get(name)?.node {
                        GraphNode::Map(map) => Some(map),
                        _ => None,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn insert_key<'a>(plan: &mut ScopePlan<'a>, chain: &[&'a MapNode], key: SelectorKey) {
    plan.keys.insert(key.clone());
    let Some((map, rest)) = chain.split_first() else {
        let keys = plan.local.entry(key.name.clone()).or_default();
        if !keys.contains(&key) {
            keys.push(key);
        }
        return;
    };
    let index = plan
        .maps
        .iter()
        .position(|(existing, _)| existing.name == map.name)
        .unwrap_or_else(|| {
            plan.maps.push((map, ScopePlan::default()));
            plan.maps.len() - 1
        });
    insert_key(&mut plan.maps[index].1, rest, key);
}

fn sort_maps(plan: &mut ScopePlan<'_>, inputs: DimensionInputs<'_, '_>) {
    plan.maps
        .sort_by_key(|(map, _)| inputs.nodes[&map.name].ordinal);
    for (_, child) in &mut plan.maps {
        sort_maps(child, inputs);
    }
}

fn scope_outcomes(
    plan: &ScopePlan<'_>,
    inputs: DimensionInputs<'_, '_>,
    inherited: &Assignment,
) -> Option<Vec<Assignment>> {
    let available = plan.keys.iter().chain(inherited.keys()).cloned().collect();
    let context = AggregateComponentContext {
        inputs,
        scope_nodes: &plan.local,
        shared_assignment: inherited,
        available_keys: &available,
    };
    let names = plan.local.keys().cloned().collect::<BTreeSet<_>>();
    let local_keys = plan.local.values().flatten().cloned().collect();
    let dimensions = names
        .iter()
        .map(|name| item_node_dimension(&context, name))
        .collect::<Option<Vec<_>>>()?;
    let outcomes = enumerate_assignments(dimensions, inputs.budget)?;
    let mut outcomes = fold_child_maps(plan, inputs, (inherited, outcomes))?;
    outcomes.retain(|outcome| item_assignment_matches(&context, &names, &local_keys, outcome));
    for outcome in &mut outcomes {
        outcome.retain(|key, _| key.source != ControlSourceKey::Execution);
    }
    normalize_outcome_labels(&mut outcomes, inputs.label_classes)?;
    Some(
        outcomes
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    )
}

fn map_outcomes(
    body: &ScopePlan<'_>,
    inputs: DimensionInputs<'_, '_>,
    inherited: &Assignment,
) -> Option<Vec<Assignment>> {
    let keys = body.keys.iter().cloned().collect::<Vec<_>>();
    let outcomes = scope_outcomes(body, inputs, inherited)?;
    aggregate_assignments_from_outcomes(AggregateCounts {
        keys: &keys,
        outcomes: &outcomes,
        saturation: inputs.saturation,
        budget: inputs.budget,
    })
}

fn fold_child_maps(
    plan: &ScopePlan<'_>,
    inputs: DimensionInputs<'_, '_>,
    facts: (&Assignment, Vec<Assignment>),
) -> Option<Vec<Assignment>> {
    let (inherited, mut outcomes) = facts;
    for (map, body) in &plan.maps {
        let mut combined = Vec::new();
        for prefix in outcomes {
            inputs.budget.charge(
                1 + inherited.len()
                    + prefix.len()
                    + inherited
                        .values()
                        .chain(prefix.values())
                        .map(|c| c.counts.len())
                        .sum::<usize>(),
            )?;
            let mut facts = inherited.clone();
            facts.extend(prefix.clone());
            let counts = if inputs
                .execution_correlations
                .get(&map.name)
                .is_some_and(|correlation| !correlation.presence.evaluate(&facts))
            {
                vec![empty_outcome(
                    &body.keys.iter().cloned().collect::<Vec<_>>(),
                )]
            } else {
                map_outcomes(body, inputs, &facts)?
            };
            let joined = enumerate_assignments(vec![vec![prefix], counts], inputs.budget)?;
            if combined.len().checked_add(joined.len())? as u64 > FULL_V1_MAX_GUARD_ASSIGNMENTS {
                return None;
            }
            combined.extend(joined);
        }
        outcomes = combined;
    }
    Some(outcomes)
}
