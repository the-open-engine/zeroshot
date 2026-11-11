use super::*;

pub(super) fn outcome_refinement(assignments: &[&Assignment]) -> OutcomeRefinement {
    let names = assignments
        .iter()
        .flat_map(|assignment| assignment.keys())
        .filter(|key| {
            matches!(
                key.source,
                ControlSourceKey::Signal | ControlSourceKey::Error
            ) || is_parallel_group_control(key)
        })
        .map(|key| key.name.clone())
        .collect::<BTreeSet<_>>();
    let mut refinement = OutcomeRefinement::default();
    for name in names {
        if assignments
            .iter()
            .any(|assignment| assignment_has_failure(assignment, &name))
        {
            refinement.failed.insert(name);
        } else {
            refinement.success.insert(name);
        }
    }
    refinement
}

pub(super) fn assignment_has_failure(assignment: &Assignment, name: &NodeName) -> bool {
    let mut has_signal = false;
    let mut signal_occurrences = 0_u64;
    for (key, value) in assignment.iter().filter(|(key, _)| key.name == *name) {
        match key.source {
            ControlSourceKey::Error if value.counts.values().any(|count| *count > 0) => {
                return true;
            }
            ControlSourceKey::Group
                if is_parallel_group_control(key)
                    && value.counts.iter().any(|(label, count)| {
                        *count > 0
                            && matches!(label.as_str(), "quorum_unreachable" | "no_satisfier")
                    }) =>
            {
                return true;
            }
            ControlSourceKey::Signal => {
                has_signal = true;
                signal_occurrences =
                    signal_occurrences.saturating_add(value.counts.values().copied().sum::<u64>());
            }
            ControlSourceKey::Error | ControlSourceKey::Group | ControlSourceKey::Execution => {}
        }
    }
    has_signal && signal_occurrences == 0
}

fn is_parallel_group_control(key: &SelectorKey) -> bool {
    key.source == ControlSourceKey::Group
        && key
            .field
            .as_ref()
            .is_some_and(|field| matches!(field.as_str(), "joined" | "raced"))
}

#[derive(Clone, Copy)]
pub(super) struct DimensionInputs<'a, 'graph> {
    pub(super) label_classes: &'a BTreeMap<SelectorKey, BTreeMap<EnumLabel, EnumLabel>>,
    pub(super) count_limits: &'a CountLimits,
    pub(super) budget: &'a ProofWork,
    pub(super) nodes: &'a BTreeMap<NodeName, NodeInfo<'graph>>,
    pub(super) map_aggregates: &'a BTreeMap<SelectorKey, MapAggregate>,
    pub(super) join_correlations: &'a BTreeMap<NodeName, ParallelJoinCorrelation>,
    pub(super) execution_correlations: &'a BTreeMap<NodeName, MapExecutionCorrelation>,
}

pub(super) fn build_dimensions(
    selectors: &[ControlSelector],
    inputs: DimensionInputs<'_, '_>,
) -> Option<Vec<Vec<Assignment>>> {
    scoped::dimensions(selectors, inputs)
}

pub(super) fn enumerate_assignments(
    dimensions: Vec<Vec<Assignment>>,
    budget: &ProofWork,
) -> Option<Vec<Assignment>> {
    let mut assignments = vec![Assignment::new()];
    for dimension in dimensions {
        let mut next = Vec::new();
        for prefix in &assignments {
            for choice in &dimension {
                budget.charge(
                    1 + prefix.len()
                        + choice.len()
                        + prefix
                            .values()
                            .chain(choice.values())
                            .map(|control| control.counts.len())
                            .sum::<usize>(),
                )?;
                if !assignments_are_compatible(prefix, choice) {
                    continue;
                }
                if next.len() as u64 >= FULL_V1_MAX_GUARD_ASSIGNMENTS {
                    return None;
                }
                let mut assignment = prefix.clone();
                assignment.extend(choice.clone());
                next.push(assignment);
            }
        }
        assignments = next;
    }
    Some(assignments)
}

fn assignments_are_compatible(left: &Assignment, right: &Assignment) -> bool {
    right
        .iter()
        .all(|(key, value)| left.get(key).is_none_or(|existing| existing == value))
}

mod aggregate;
mod outcomes;
mod retention;
mod scoped;

use aggregate::*;
pub(super) use outcomes::*;
pub(super) use retention::{
    retention_index, retention_guard, proof_group_domain, retained_selection_matches,
    is_map_item_predicate, map_completion_predicate,
};

#[path = "proof_work.rs"]
mod proof_work;
pub(super) use proof_work::ProofWork;

use aggregate::project_retained_controls;

/// Each observable label class retains only the largest count a guard can distinguish.
pub(super) type CountLimits = BTreeMap<SelectorKey, BTreeMap<EnumLabel, u64>>;
