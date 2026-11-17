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
    // Static witness for the subset of branches retained at a valid join.
    group.branches.as_slice().get(index)?;
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
    if is_map_item_predicate_field(field) {
        return Some(
            ["complete", "blocked"]
                .into_iter()
                .filter_map(enum_label)
                .collect(),
        );
    }
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

// Private per-item normal-completion witness. It is derived jointly from the body's real controls,
// then counted with the same cardinality and saturation as authored aggregate selectors.
pub(in crate::graph_verifier) fn is_map_item_predicate(key: &SelectorKey) -> bool {
    key.source == ControlSourceKey::Group && is_map_item_predicate_field(key.field.as_ref())
}

fn is_map_item_predicate_field(field: Option<&FieldName>) -> bool {
    field.is_some_and(|field| field.as_str() == "__map_body_completed")
}

pub(in crate::graph_verifier) fn map_completion_predicate(
    map: &MapNode,
) -> Option<CompletionPredicate> {
    map_item_guard(map, FieldName::new("__map_body_completed").ok()?)
        .map(CompletionPredicate::Guard)
}

fn map_item_guard(map: &MapNode, field: FieldName) -> Option<Guard> {
    Some(Guard::Not {
        guard: Box::new(Guard::KOfMap {
            count: PositiveInteger::new(1).ok()?,
            value: ControlSelector {
                name: map.body.name().clone(),
                source: ControlSource::Group,
                field: Some(field),
            },
            labels: openengine_cluster_protocol::NonEmptyEnumSet::new(vec![enum_label("blocked")?])
                .ok()?,
        }),
    })
}
