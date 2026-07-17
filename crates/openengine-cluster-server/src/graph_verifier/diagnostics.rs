use super::*;

pub(super) fn guard_node_count(guard: &Guard) -> u64 {
    1 + match guard {
        Guard::All { guards } | Guard::Any { guards } => {
            guards.as_slice().iter().map(guard_node_count).sum()
        }
        Guard::Not { guard } => guard_node_count(guard),
        Guard::In { .. } | Guard::KOfN { .. } | Guard::KOfMap { .. } => 0,
    }
}

pub(super) fn guard_selectors(guard: &Guard) -> Vec<&ControlSelector> {
    let mut selectors = Vec::new();
    collect_guard_selectors(guard, &mut selectors);
    selectors
}

pub(super) fn guard_selector_uses(guard: &Guard) -> Vec<(&ControlSelector, bool)> {
    let mut selectors = Vec::new();
    collect_guard_selector_uses(guard, false, &mut selectors);
    selectors
}

pub(super) fn collect_guard_selector_uses<'a>(
    guard: &'a Guard,
    map_aggregate: bool,
    selectors: &mut Vec<(&'a ControlSelector, bool)>,
) {
    match guard {
        Guard::In { value, .. } => selectors.push((value, map_aggregate)),
        Guard::KOfMap { value, .. } => selectors.push((value, true)),
        Guard::All { guards } | Guard::Any { guards } => {
            for guard in guards.as_slice() {
                collect_guard_selector_uses(guard, map_aggregate, selectors);
            }
        }
        Guard::Not { guard } => collect_guard_selector_uses(guard, map_aggregate, selectors),
        Guard::KOfN { values, .. } => {
            selectors.extend(values.as_slice().iter().map(|value| (value, map_aggregate)));
        }
    }
}

pub(super) fn collect_guard_selectors<'a>(
    guard: &'a Guard,
    selectors: &mut Vec<&'a ControlSelector>,
) {
    match guard {
        Guard::In { value, .. } | Guard::KOfMap { value, .. } => selectors.push(value),
        Guard::All { guards } | Guard::Any { guards } => {
            for guard in guards.as_slice() {
                collect_guard_selectors(guard, selectors);
            }
        }
        Guard::Not { guard } => collect_guard_selectors(guard, selectors),
        Guard::KOfN { values, .. } => selectors.extend(values.as_slice()),
    }
}

pub(super) fn sort_diagnostics(diagnostics: &mut [GraphDiagnostic]) {
    diagnostics.sort_by(|left, right| {
        compare_paths(&left.path, &right.path)
            .then_with(|| diagnostic_code_rank(left.code).cmp(&diagnostic_code_rank(right.code)))
            .then_with(|| left.message.cmp(&right.message))
            .then_with(|| left.related_nodes.cmp(&right.related_nodes))
    });
}

pub(super) fn compare_paths(
    left: &[DiagnosticPathSegment],
    right: &[DiagnosticPathSegment],
) -> Ordering {
    for (left, right) in left.iter().zip(right) {
        let ordering = path_segment_key(left).cmp(&path_segment_key(right));
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

pub(super) fn path_segment_key(segment: &DiagnosticPathSegment) -> (u8, String, u32) {
    match segment {
        DiagnosticPathSegment::Field { name } => (0, name.as_str().to_owned(), 0),
        DiagnosticPathSegment::Index { index } => (1, String::new(), *index),
        DiagnosticPathSegment::Node { name } => (2, name.as_str().to_owned(), 0),
    }
}

const fn diagnostic_code_rank(code: GraphDiagnosticCode) -> u8 {
    code as u8
}

pub(super) fn field_name(value: &str) -> FieldName {
    FieldName::new(value).expect("static diagnostic field name must be valid")
}

pub(super) fn enum_label(value: &str) -> EnumLabel {
    EnumLabel::new(value).expect("static control label must be valid")
}

pub(super) fn field_segment(value: &str) -> DiagnosticPathSegment {
    DiagnosticPathSegment::Field {
        name: field_name(value),
    }
}

pub(super) fn index_segment(value: usize) -> DiagnosticPathSegment {
    DiagnosticPathSegment::Index {
        index: u32::try_from(value).expect("graph collection index is wire-bounded"),
    }
}

pub(super) fn node_segment(value: &NodeName) -> DiagnosticPathSegment {
    DiagnosticPathSegment::Node {
        name: value.clone(),
    }
}

pub(super) fn with_field(
    path: &[DiagnosticPathSegment],
    field: &str,
) -> Vec<DiagnosticPathSegment> {
    let mut result = path.to_vec();
    result.push(field_segment(field));
    result
}

pub(super) fn field_path(
    path: &[DiagnosticPathSegment],
    fields: &[&str],
) -> Vec<DiagnosticPathSegment> {
    let mut result = path.to_vec();
    result.extend(fields.iter().map(|field| field_segment(field)));
    result
}

pub(super) fn indexed_field_path(
    path: &[DiagnosticPathSegment],
    field: &str,
    index: usize,
) -> Vec<DiagnosticPathSegment> {
    let mut result = with_field(path, field);
    result.push(index_segment(index));
    result
}

pub(super) fn child_path(
    path: &[DiagnosticPathSegment],
    field: &str,
    index: usize,
    name: &NodeName,
) -> Vec<DiagnosticPathSegment> {
    let mut result = indexed_field_path(path, field, index);
    result.push(node_segment(name));
    result
}

pub(super) fn named_child_path(
    path: &[DiagnosticPathSegment],
    field: &str,
    name: &NodeName,
) -> Vec<DiagnosticPathSegment> {
    let mut result = with_field(path, field);
    result.push(node_segment(name));
    result
}

pub(super) fn guard_path(
    path: &[DiagnosticPathSegment],
    collection: &str,
    index: usize,
    field: &str,
) -> Vec<DiagnosticPathSegment> {
    let mut result = indexed_field_path(path, collection, index);
    result.push(field_segment(field));
    result
}

pub(super) fn choice_branch_node_path(
    path: &[DiagnosticPathSegment],
    index: usize,
    name: &NodeName,
) -> Vec<DiagnosticPathSegment> {
    let mut result = indexed_field_path(path, "branches", index);
    result.push(field_segment("node"));
    result.push(node_segment(name));
    result
}
