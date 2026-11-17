use super::*;

pub(super) fn execution_key(name: &NodeName) -> SelectorKey {
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
