use super::*;

// Private proof witnesses identify retained branches. They never enter authored controls or IR.
const RETENTION_PREFIX: &str = "__retained.";

pub(in crate::graph_verifier) fn retention_index(field: Option<&FieldName>) -> Option<usize> {
    field?.as_str().strip_prefix(RETENTION_PREFIX)?.parse().ok()
}

pub(in crate::graph_verifier) fn retention_guard(
    group: &ParNode,
    index: usize,
) -> Option<CompletionPredicate> {
    Some(CompletionPredicate::Guard(Guard::In {
        value: ControlSelector {
            name: group.name.clone(),
            source: ControlSource::Group,
            field: Some(FieldName::new(format!("{RETENTION_PREFIX}{index}")).ok()?),
        },
        labels: openengine_cluster_protocol::NonEmptyEnumSet::new(vec![enum_label("retained")?])
            .ok()?,
    }))
}

pub(in crate::graph_verifier) fn proof_group_domain(
    node: &GraphNode,
    field: Option<&FieldName>,
) -> Option<Vec<EnumLabel>> {
    if let Some(index) = retention_index(field) {
        let GraphNode::Par(group) = node else {
            return None;
        };
        group.branches.as_slice().get(index)?;
        return Some(
            ["omitted", "retained"]
                .into_iter()
                .filter_map(enum_label)
                .collect(),
        );
    }
    group_domain(node, field)
}

// Filter these witnesses jointly before any guard proof, including per-item aggregate expansion.
// Unqueried branches need no witness: their eligible count supplies the remaining winner slots.
pub(in crate::graph_verifier) fn retained_selection_matches(
    assignment: &Assignment,
    name: &NodeName,
    retention: &ParallelRetention,
) -> bool {
    let requested = assignment
        .iter()
        .filter_map(|(key, control)| {
            (key.name == *name
                && key.source == ControlSourceKey::Group
                && control.counts.values().any(|count| *count > 0))
            .then(|| {
                retention_index(key.field.as_ref()).map(|index| {
                    let selected = control
                        .counts
                        .iter()
                        .any(|(label, count)| label.as_str() == "retained" && *count > 0);
                    (index, selected)
                })
            })
            .flatten()
        })
        .collect::<BTreeMap<_, _>>();
    if requested.is_empty() {
        return true;
    }
    let eligible = retention
        .branches
        .iter()
        .map(|branch| branch.evaluate(assignment))
        .collect::<Vec<_>>();
    let completed = eligible.iter().filter(|eligible| **eligible).count() as u64;
    let selected = requested.values().filter(|selected| **selected).count() as u64;
    if completed < retention.required {
        return selected == 0;
    }
    let selected_eligible = requested
        .iter()
        .all(|(index, selected)| !selected || eligible.get(*index) == Some(&true));
    let remaining = eligible
        .iter()
        .enumerate()
        .filter(|(index, eligible)| **eligible && !requested.contains_key(index))
        .count() as u64;
    selected_eligible
        && selected <= retention.required
        && selected + remaining >= retention.required
}
