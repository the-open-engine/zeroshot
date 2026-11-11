//! Coherent final-round selector snapshots; never carry an exiting round into another round.
use super::*;

#[derive(Clone, PartialEq)]
pub(in crate::full_v1_reducer) struct LoopExit {
    pub sizes: MapSizes,
    pub current: bool,
    pub convergence: Guard,
}

impl Engine<'_> {
    pub(in crate::full_v1_reducer) fn record_loop_exit(
        &self,
        group: &openengine_cluster_protocol::LoopNode,
        scope: (&mut Context, Traversal<'_>),
        current: bool,
    ) {
        let (context, traversal) = scope;
        if !self.loop_may_converge(group, context, traversal) {
            return;
        }
        let names = descendant_names(&group.body);
        let selected = context.map_sizes.iter().filter(|((name, indices), _)| {
            names.contains(name) && indices.starts_with(traversal.map_indices)
        });
        let mut sizes = MapSizes::new();
        for (key, cardinality) in selected {
            if self.charge_cardinality(key, cardinality).is_none() {
                return;
            }
            sizes.insert(key.clone(), cardinality.clone());
        }
        if sizes.is_empty() {
            return;
        }
        if self.selector_work.charge(2).is_none() {
            return;
        }
        let exit = LoopExit {
            sizes,
            current,
            convergence: convergence_guard(&group.name),
        };
        let alternatives = context
            .loop_exits
            .entry((group.name.clone(), traversal.map_indices.to_vec()))
            .or_default();
        self.retain_loop_exit(alternatives, exit);
    }

    fn retain_loop_exit(&self, alternatives: &mut Vec<LoopExit>, exit: LoopExit) {
        for prior in alternatives.iter() {
            for (key, size) in &prior.sizes {
                if self.charge_cardinality(key, size).is_none() {
                    return;
                }
            }
            if *prior == exit {
                return;
            }
        }
        alternatives.push(exit);
    }

    fn charge_cardinality(&self, key: &(NodeName, Vec<u64>), value: &MapCardinality) -> Option<()> {
        self.selector_work
            .charge(1 + key.1.len() + value.writes.len())?;
        for write in &value.writes {
            self.selector_work
                .charge(write.lengths.as_ref().map_or(0, BTreeSet::len))?;
            if let Some(guard) = &write.condition {
                self.selector_work.charge_guard(guard)?;
            }
        }
        Some(())
    }
}

pub(in crate::full_v1_reducer) fn convergence_guard(name: &NodeName) -> Guard {
    // Register the selector with its actual map owner; within that item this is one outcome.
    Guard::KOfMap {
        count: PositiveInteger::new(1).expect("positive static count"),
        value: ControlSelector {
            name: name.clone(),
            source: ControlSource::Group,
            field: Some("terminated".parse().expect("static field")),
        },
        labels: openengine_cluster_protocol::NonEmptyEnumSet::new(vec![
            "converged".parse().expect("static label"),
        ])
        .expect("nonempty"),
    }
}
