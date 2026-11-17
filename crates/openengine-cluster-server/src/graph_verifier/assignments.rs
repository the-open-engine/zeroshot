use super::*;

/// Controls whose labels say whether their node succeeded.
pub(super) fn observes_outcome(key: &SelectorKey) -> bool {
    matches!(
        key.source,
        ControlSourceKey::Signal | ControlSourceKey::Error
    ) || is_parallel_group_control(key)
}

pub(super) fn control_label_is_failure(key: &SelectorKey, label: &EnumLabel) -> bool {
    key.source == ControlSourceKey::Error
        || (is_parallel_group_control(key)
            && matches!(label.as_str(), "quorum_unreachable" | "no_satisfier"))
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
    pub(super) nodes: &'a BTreeMap<NodeName, NodeInfo<'graph>>,
    pub(super) map_aggregates: &'a BTreeMap<SelectorKey, MapAggregate>,
    pub(super) join_correlations: &'a BTreeMap<NodeName, ParallelJoinCorrelation>,
    pub(super) execution_correlations: &'a BTreeMap<NodeName, MapExecutionCorrelation>,
}

mod aggregate;
mod controls;
mod diagram;
mod outcomes;
mod retention;
mod scoped;
mod search;

use aggregate::*;
pub(super) use controls::*;
pub(super) use diagram::*;
pub(super) use scoped::{scope_plan, ScopePlan};
pub(super) use search::*;
pub(super) use outcomes::*;
pub(super) use retention::{
    retention_index, retention_guard, proof_group_domain, is_map_item_predicate,
    map_completion_predicate,
};

/// Classes preserve presence and outcome refinement; higher counts saturate at guard thresholds.
pub(super) type CountLimits = BTreeMap<SelectorKey, BTreeMap<EnumLabel, u64>>;
