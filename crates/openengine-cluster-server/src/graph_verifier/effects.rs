use super::*;

pub(super) fn terminal_effects(name: &NodeName, incoming: &Flow) -> Effects {
    Effects {
        definite_nodes: BTreeSet::from([name.clone()]),
        exit_failed: incoming.failed.clone(),
        ..Effects::default()
    }
}

pub(super) fn node_identity(node: &GraphNode) -> usize {
    std::ptr::from_ref(node).cast::<()>() as usize
}

pub(super) fn parallel_required_completions(join: &Join, branch_count: usize) -> u64 {
    match join {
        Join::All {} => branch_count as u64,
        Join::Any {} | Join::First { .. } => 1,
        Join::Quorum { count } => count.get(),
    }
}

pub(super) fn merge_parallel_scenarios(
    branches: &[Effects],
    required: u64,
    scenarios: &[BTreeSet<usize>],
) -> Effects {
    let mut merged = Effects::default();
    for branch in branches {
        merged
            .possible_writes
            .extend(branch.possible_writes.clone());
        merge_possible_write_types(
            &mut merged.possible_write_types,
            &branch.possible_write_types,
        );
    }

    if !scenarios.is_empty() {
        merged.definite_nodes = parallel_definite_nodes(branches, required, scenarios);
        merged.definite_writes = parallel_definite_writes(branches, required, scenarios);
        merged.outcome_writes = parallel_outcome_writes(branches, required, scenarios);
        merged.exit_failed = scenarios
            .iter()
            .flat_map(|scenario| scenario.iter())
            .flat_map(|index| branches[*index].exit_failed.clone())
            .collect();
        merged.falls_through = true;
    }
    merged
}

pub(super) fn parallel_definite_nodes(
    branches: &[Effects],
    required: u64,
    scenarios: &[BTreeSet<usize>],
) -> BTreeSet<NodeName> {
    let candidates = branches
        .iter()
        .flat_map(|branch| branch.definite_nodes.iter().cloned())
        .collect::<BTreeSet<_>>();
    candidates
        .into_iter()
        .filter(|name| {
            scenarios.iter().all(|scenario| {
                parallel_fact_is_definite(scenario, required, |index| {
                    branches[index].definite_nodes.contains(name)
                })
            })
        })
        .collect()
}

pub(super) fn parallel_definite_writes(
    branches: &[Effects],
    required: u64,
    scenarios: &[BTreeSet<usize>],
) -> Writes {
    let candidates = branches
        .iter()
        .flat_map(|branch| branch.definite_writes.keys().cloned())
        .collect::<BTreeSet<_>>();
    candidates
        .into_iter()
        .filter_map(|path| {
            let guaranteed = scenarios.iter().all(|scenario| {
                parallel_fact_is_definite(scenario, required, |index| {
                    branches[index].definite_writes.contains_key(&path)
                })
            });
            let providers = scenarios
                .iter()
                .flat_map(|scenario| scenario.iter())
                .filter_map(|index| branches[*index].definite_writes.get(&path))
                .collect::<Vec<_>>();
            guaranteed
                .then(|| intersect_write_fact_set(&providers))
                .flatten()
                .map(|write| (path, write))
        })
        .collect()
}

pub(super) fn parallel_outcome_writes(
    branches: &[Effects],
    required: u64,
    scenarios: &[BTreeSet<usize>],
) -> OutcomeWrites {
    let candidates = branches
        .iter()
        .flat_map(|branch| {
            branch
                .outcome_writes
                .iter()
                .flat_map(|(name, writes)| writes.keys().cloned().map(|path| (name.clone(), path)))
        })
        .collect::<BTreeSet<_>>();
    let mut merged = OutcomeWrites::new();
    for (name, path) in candidates {
        let guaranteed = scenarios.iter().all(|scenario| {
            parallel_fact_is_definite(scenario, required, |index| {
                branches[index]
                    .outcome_writes
                    .get(&name)
                    .is_some_and(|writes| writes.contains_key(&path))
            })
        });
        let providers = scenarios
            .iter()
            .flat_map(|scenario| scenario.iter())
            .filter_map(|index| {
                branches[*index]
                    .outcome_writes
                    .get(&name)
                    .and_then(|writes| writes.get(&path))
            })
            .collect::<Vec<_>>();
        if guaranteed {
            if let Some(write) = intersect_write_fact_set(&providers) {
                merged.entry(name.clone()).or_default().insert(path, write);
            }
        }
    }
    merged
}

pub(super) fn parallel_fact_is_definite(
    completing: &BTreeSet<usize>,
    required: u64,
    provides: impl Fn(usize) -> bool,
) -> bool {
    (completing.iter().filter(|index| !provides(**index)).count() as u64) < required
}

pub(super) fn intersect_write_fact_set(writes: &[&WriteFact]) -> Option<WriteFact> {
    let (first, rest) = writes.split_first()?;
    rest.iter().try_fold((*first).clone(), |common, write| {
        intersect_write_facts(&common, write)
    })
}

pub(super) fn merge_alternatives(branches: &[Effects]) -> Effects {
    let completing = branches
        .iter()
        .filter(|branch| branch.falls_through)
        .collect::<Vec<_>>();
    let Some(first) = completing.first() else {
        let mut possible_write_types = BTreeMap::new();
        for branch in branches {
            merge_possible_write_types(&mut possible_write_types, &branch.possible_write_types);
        }
        return Effects {
            possible_writes: branches
                .iter()
                .flat_map(|branch| branch.possible_writes.clone())
                .collect(),
            possible_write_types,
            ..Effects::default()
        };
    };
    let mut definite_nodes = first.definite_nodes.clone();
    let mut definite_writes = first.definite_writes.clone();
    let mut outcome_writes = first.outcome_writes.clone();
    let mut possible_writes = BTreeMap::new();
    let mut possible_write_types = BTreeMap::new();
    let mut exit_failed = BTreeSet::new();
    let completion =
        CompletionPredicate::any(completing.iter().map(|branch| branch.completion.clone()));
    for branch in branches {
        possible_writes.extend(branch.possible_writes.clone());
        merge_possible_write_types(&mut possible_write_types, &branch.possible_write_types);
        if branch.falls_through {
            retain_common_effects(
                &mut definite_nodes,
                &mut definite_writes,
                &mut outcome_writes,
                branch,
            );
            exit_failed.extend(branch.exit_failed.clone());
        }
    }
    Effects {
        definite_nodes,
        definite_writes,
        possible_writes,
        possible_write_types,
        outcome_writes,
        exit_failed,
        falls_through: true,
        completion,
    }
}

pub(super) fn retain_common_effects(
    definite_nodes: &mut BTreeSet<NodeName>,
    definite_writes: &mut Writes,
    outcome_writes: &mut OutcomeWrites,
    branch: &Effects,
) {
    definite_nodes.retain(|name| branch.definite_nodes.contains(name));
    definite_writes.retain(|path, write| {
        let Some(other) = branch.definite_writes.get(path) else {
            return false;
        };
        let Some(common) = intersect_write_facts(write, other) else {
            return false;
        };
        *write = common;
        true
    });
    outcome_writes.retain(|name, writes| {
        let Some(other) = branch.outcome_writes.get(name) else {
            return false;
        };
        writes.retain(|path, write| {
            let Some(other) = other.get(path) else {
                return false;
            };
            let Some(common) = intersect_write_facts(write, other) else {
                return false;
            };
            *write = common;
            true
        });
        !writes.is_empty()
    });
}

pub(super) fn intersect_write_facts(left: &WriteFact, right: &WriteFact) -> Option<WriteFact> {
    let value_type = common_supertype(&left.value_type, &right.value_type)?;
    let mut guaranteed_paths = BTreeMap::new();
    for (path, left_type) in &left.guaranteed_paths {
        let Some(right_type) = right.guaranteed_paths.get(path) else {
            continue;
        };
        if let Some(value_type) = common_supertype(left_type, right_type) {
            guaranteed_paths.insert(path.clone(), value_type);
        }
    }
    Some(WriteFact {
        value_type,
        guaranteed_paths,
    })
}

pub(super) fn common_supertype(left: &PayloadType, right: &PayloadType) -> Option<PayloadType> {
    if left.is_subtype_of(right) {
        Some(right.clone())
    } else if right.is_subtype_of(left) {
        Some(left.clone())
    } else {
        None
    }
}

pub(super) fn apply_write_facts(defined: &mut BTreeMap<FieldPath, PayloadType>, writes: &Writes) {
    for (target, write) in writes {
        defined.retain(|path, _| !path_is_prefix(target, path));
        defined.extend(write.guaranteed_paths.clone());
    }
}

pub(super) fn merge_outcome_writes(target: &mut OutcomeWrites, source: &OutcomeWrites) {
    for (name, writes) in source {
        target
            .entry(name.clone())
            .or_default()
            .extend(writes.clone());
    }
}

pub(super) fn merge_possible_write_types(
    target: &mut BTreeMap<FieldPath, Vec<PayloadType>>,
    source: &BTreeMap<FieldPath, Vec<PayloadType>>,
) {
    for (path, source_types) in source {
        let target_types = target.entry(path.clone()).or_default();
        for source_type in source_types {
            if !target_types.contains(source_type) {
                target_types.push(source_type.clone());
            }
        }
    }
}

pub(super) fn retain_promoted_writes(effects: &mut Effects, allowed: &BTreeSet<FieldPath>) {
    effects
        .definite_writes
        .retain(|path, _| allowed.contains(path));
    effects
        .possible_writes
        .retain(|path, _| allowed.contains(path));
    effects
        .possible_write_types
        .retain(|path, _| allowed.contains(path));
    for writes in effects.outcome_writes.values_mut() {
        writes.retain(|path, _| allowed.contains(path));
    }
    effects
        .outcome_writes
        .retain(|_, writes| !writes.is_empty());
}

pub(super) fn compatible_types(left: &PayloadType, right: &PayloadType) -> bool {
    left.is_subtype_of(right) || right.is_subtype_of(left)
}

pub(super) fn collect_descendant_names(branches: &NonEmptyVec<GraphNode>) -> BTreeSet<NodeName> {
    let mut names = BTreeSet::new();
    for branch in branches.as_slice() {
        collect_names(branch, &mut names);
    }
    names
}

pub(super) fn collect_names(node: &GraphNode, names: &mut BTreeSet<NodeName>) {
    names.insert(node.name().clone());
    for child in child_nodes(node) {
        collect_names(child, names);
    }
}

fn child_nodes(node: &GraphNode) -> Vec<&GraphNode> {
    match node {
        GraphNode::Seq(group) => group.children.as_slice().iter().collect(),
        GraphNode::Choice(group) => group
            .branches
            .as_slice()
            .iter()
            .map(|branch| &branch.node)
            .chain(group.otherwise.iter().map(Box::as_ref))
            .collect(),
        GraphNode::Par(group) => group.branches.as_slice().iter().collect(),
        GraphNode::Loop(group) => vec![&group.body],
        GraphNode::Map(group) => vec![&group.body],
        GraphNode::Step(_)
        | GraphNode::Verifier(_)
        | GraphNode::Succeed(_)
        | GraphNode::Fail(_) => Vec::new(),
    }
}

pub(super) fn find_map_owner<'a>(
    node: &'a GraphNode,
    target: &NodeName,
    owner: Option<&'a NodeName>,
) -> Option<&'a NodeName> {
    if node.name() == target {
        return owner;
    }
    match node {
        GraphNode::Seq(group) => group
            .children
            .as_slice()
            .iter()
            .find_map(|child| find_map_owner(child, target, owner)),
        GraphNode::Choice(group) => group
            .branches
            .as_slice()
            .iter()
            .find_map(|branch| find_map_owner(&branch.node, target, owner))
            .or_else(|| {
                group
                    .otherwise
                    .as_ref()
                    .and_then(|node| find_map_owner(node, target, owner))
            }),
        GraphNode::Par(group) => group
            .branches
            .as_slice()
            .iter()
            .find_map(|branch| find_map_owner(branch, target, owner)),
        GraphNode::Loop(group) => find_map_owner(&group.body, target, owner),
        GraphNode::Map(group) => find_map_owner(&group.body, target, Some(&group.name)),
        GraphNode::Step(_)
        | GraphNode::Verifier(_)
        | GraphNode::Succeed(_)
        | GraphNode::Fail(_) => None,
    }
}
