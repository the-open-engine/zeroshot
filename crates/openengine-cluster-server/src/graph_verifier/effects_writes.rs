use super::*;

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
        defined.retain(|path, _| !paths_overlap(target, path));
        defined.extend(write.guaranteed_paths.clone());
    }
}

pub(super) fn compose_writes(target: &mut Writes, source: &Writes) {
    for (path, write) in source {
        target.retain(|prior, _| !paths_overlap(prior, path));
        target.insert(path.clone(), write.clone());
    }
}

pub(super) fn parallel_definition_effect(
    name: &NodeName,
    before: &BTreeMap<FieldPath, PayloadType>,
    writes: &Writes,
) -> Option<ParallelDefinitionEffect> {
    let mut after = before.clone();
    apply_write_facts(&mut after, writes);
    let targets = writes.keys().cloned().collect::<BTreeSet<_>>();
    let paths = before
        .keys()
        .chain(after.keys())
        .filter(|path| targets.iter().any(|target| paths_overlap(target, path)))
        .cloned()
        .collect::<BTreeSet<_>>();
    let changed = paths.iter().any(|path| before.get(path) != after.get(path));
    let transitions = paths
        .into_iter()
        .map(|path| {
            (
                path.clone(),
                DefinitionTransition {
                    before: before.get(&path).cloned(),
                    after: after.get(&path).cloned(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    changed.then(|| ParallelDefinitionEffect {
        name: name.clone(),
        targets,
        transitions,
    })
}

pub(super) fn retain_parallel_definition_effects(
    existing: &mut Vec<ParallelDefinitionEffect>,
    writes: &Writes,
    incoming: &[ParallelDefinitionEffect],
) {
    let conditional = incoming
        .iter()
        .flat_map(|effect| effect.targets.iter())
        .collect::<BTreeSet<_>>();
    for effect in existing.iter_mut() {
        effect.targets.retain(|prior| {
            !writes
                .keys()
                .any(|target| !conditional.contains(target) && paths_overlap(prior, target))
        });
        effect.transitions.retain(|path, _| {
            effect
                .targets
                .iter()
                .any(|target| path_is_prefix(target, path))
        });
    }
    existing.retain(|effect| !effect.targets.is_empty() && !effect.transitions.is_empty());
}

pub(super) fn writes_without_parallel_owners(
    writes: &Writes,
    conditional: &[ParallelDefinitionEffect],
) -> Writes {
    writes
        .iter()
        .filter(|(target, _)| {
            !conditional
                .iter()
                .flat_map(|effect| &effect.targets)
                .any(|owned| paths_overlap(owned, target))
        })
        .map(|(path, write)| (path.clone(), write.clone()))
        .collect()
}

pub(super) fn merge_parallel_definition_effects(
    existing: &mut Vec<ParallelDefinitionEffect>,
    writes: &Writes,
    incoming: &[ParallelDefinitionEffect],
) {
    retain_parallel_definition_effects(existing, writes, incoming);
    existing.extend(incoming.iter().cloned());
}

pub(super) fn merge_outcome_writes(
    target: &mut OutcomeWrites,
    target_order: &mut Vec<NodeName>,
    source: &OutcomeWrites,
    source_order: &[NodeName],
) {
    for (name, writes) in source {
        target
            .entry(name.clone())
            .or_default()
            .extend(writes.clone());
    }
    for name in source_order {
        if source.contains_key(name) && !target_order.contains(name) {
            target_order.push(name.clone());
        }
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
    let is_promoted = |path: &FieldPath| {
        allowed
            .iter()
            .any(|promoted| path_is_prefix(promoted, path))
    };
    effects.definite_writes.retain(|path, _| is_promoted(path));
    effects.possible_writes.retain(|path, _| is_promoted(path));
    effects
        .possible_write_types
        .retain(|path, _| is_promoted(path));
    for writes in effects.outcome_writes.values_mut() {
        writes.retain(|path, _| is_promoted(path));
    }
    effects
        .outcome_writes
        .retain(|_, writes| !writes.is_empty());
    effects
        .outcome_order
        .retain(|name| effects.outcome_writes.contains_key(name));
    for effect in &mut effects.parallel_definition_effects {
        effect.targets.retain(|path| is_promoted(path));
        effect.transitions.retain(|path, _| {
            effect
                .targets
                .iter()
                .any(|target| path_is_prefix(target, path))
        });
    }
    effects
        .parallel_definition_effects
        .retain(|effect| !effect.targets.is_empty() && !effect.transitions.is_empty());
}

pub(super) fn compatible_types(left: &PayloadType, right: &PayloadType) -> bool {
    left.is_subtype_of(right) || right.is_subtype_of(left)
}
