//! Condition the shared outcome/count expansion at each real map scope. Nested maps fold into
//! their parent item before that item contributes to its enclosing collection.
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
    runtime: Option<RuntimeScope<'_>>,
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
    Some(vec![scope_outcomes(
        &plan,
        inputs,
        runtime,
        inputs.fixed_controls,
    )?])
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
    runtime: Option<RuntimeScope<'_>>,
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
        .map(|name| scope_node_dimension(&context, name, runtime))
        .collect::<Option<Vec<_>>>()?;
    let outcomes = enumerate_assignments(dimensions, inputs.budget)?;
    let mut outcomes = fold_child_maps(plan, inputs, runtime, (inherited, outcomes))?;
    let constraints = runtime
        .map(|runtime| {
            runtime
                .snapshot
                .scoped_guards(runtime.indices)
                .map(|guard| {
                    inputs.budget.charge(
                        (1 + guard_node_count(guard) as usize)
                            .checked_mul(outcomes.len().max(1))?,
                    )?;
                    Some(guard)
                })
                .collect::<Option<Vec<_>>>()
        })
        .unwrap_or_else(|| Some(vec![]))?;
    outcomes.retain(|outcome| {
        item_assignment_matches(&context, &names, &local_keys, outcome)
            && constraints
                .iter()
                .all(|guard| evaluate_guard(guard, outcome))
    });
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
    plan: (&MapNode, &ScopePlan<'_>),
    inputs: DimensionInputs<'_, '_>,
    runtime: Option<RuntimeScope<'_>>,
    inherited: &Assignment,
) -> Option<Vec<Assignment>> {
    let (map, body) = plan;
    let progress = runtime.map_or_else(
        || MapProgress {
            cardinality: MapCardinality::default(),
            known_items: vec![],
        },
        |runtime| runtime.snapshot.map_progress(map, runtime.indices),
    );
    let keys = body.keys.iter().cloned().collect::<Vec<_>>();
    let mut counts = vec![empty_outcome(&keys)];
    let known = progress
        .known_items
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    for index in &known {
        let mut indices = runtime.map_or_else(Vec::new, |runtime| runtime.indices.to_vec());
        indices.push(*index);
        let outcomes = scope_outcomes(
            body,
            inputs,
            runtime.map(|runtime| RuntimeScope {
                indices: &indices,
                ..runtime
            }),
            inherited,
        )?;
        counts = add_count_dimensions(&counts, &outcomes, inputs)?;
    }
    let lengths = selected_lengths(&progress.cardinality, inherited, inputs.budget)?;
    if lengths.as_ref().is_some_and(BTreeSet::is_empty) {
        return Some(vec![]);
    }
    if lengths
        .as_ref()
        .is_some_and(|lengths| lengths.iter().all(|length| *length <= known.len() as u64))
    {
        return Some(counts);
    }
    let fresh = (0..).find(|index| !known.contains(index))?;
    let mut indices = runtime.map_or_else(Vec::new, |runtime| runtime.indices.to_vec());
    indices.push(fresh);
    let outcomes = scope_outcomes(
        body,
        inputs,
        runtime.map(|runtime| RuntimeScope {
            indices: &indices,
            ..runtime
        }),
        inherited,
    )?;
    let future = remaining_map_counts(inputs, (&keys, &outcomes), lengths, known.len())?;
    add_count_dimensions(&counts, &future, inputs)
}

fn selected_lengths(
    cardinality: &MapCardinality,
    inherited: &Assignment,
    budget: &ProofWork,
) -> Option<Option<BTreeSet<u64>>> {
    let mut lengths = cardinality.length.map(|length| BTreeSet::from([length]));
    for write in &cardinality.writes {
        budget.charge(1 + write.lengths.as_ref().map_or(0, BTreeSet::len))?;
        if write
            .condition
            .as_ref()
            .is_none_or(|guard| evaluate_guard(guard, inherited))
        {
            lengths = write.lengths.clone();
        }
    }
    Some(lengths)
}

fn remaining_map_counts(
    inputs: DimensionInputs<'_, '_>,
    domains: (&[SelectorKey], &[Assignment]),
    lengths: Option<BTreeSet<u64>>,
    known: usize,
) -> Option<Vec<Assignment>> {
    let remaining = lengths.map_or_else(
        || vec![None],
        |lengths| {
            lengths
                .into_iter()
                .map(|length| Some(length.saturating_sub(known as u64)))
                .collect()
        },
    );
    let mut results = BTreeSet::new();
    for exact_occurrences in remaining {
        inputs.budget.charge(1)?;
        results.extend(aggregate_assignments_from_outcomes(AggregateCounts {
            keys: domains.0,
            outcomes: domains.1,
            saturation: inputs.saturation,
            budget: inputs.budget,
            exact_occurrences,
        })?);
    }
    Some(results.into_iter().collect())
}

fn scope_node_dimension(
    context: &AggregateComponentContext<'_, '_>,
    name: &NodeName,
    runtime: Option<RuntimeScope<'_>>,
) -> Option<Vec<Assignment>> {
    if context.inputs.nodes.contains_key(name) {
        return item_node_dimension(context, name, runtime);
    }
    Some(vec![
        context
            .scope_nodes
            .get(name)?
            .iter()
            .filter_map(|key| {
                context
                    .inputs
                    .fixed_controls
                    .get(key)
                    .map(|value| (key.clone(), value.clone()))
            })
            .collect(),
    ])
}

fn fold_child_maps(
    plan: &ScopePlan<'_>,
    inputs: DimensionInputs<'_, '_>,
    runtime: Option<RuntimeScope<'_>>,
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
                map_outcomes((map, body), inputs, runtime, &facts)?
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
