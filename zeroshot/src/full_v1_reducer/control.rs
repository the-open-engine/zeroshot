use super::*;

impl Engine<'_> {
    fn candidate_count(
        &self,
        context: &Context,
        indices: &[u64],
        query: (
            &ControlSelector,
            &[openengine_cluster_protocol::EnumLabel],
            bool,
        ),
    ) -> Option<u64> {
        let (selector, labels, aggregate) = query;
        let depth = *self.map_depths.get(&selector.name)?;
        let available = if aggregate && depth > indices.len() {
            self.aggregate_owner_at_scope(&selector.name, indices)
                .is_some_and(|owner| {
                    let depth = self.map_depths[owner];
                    context
                        .completed
                        .contains(&(owner.clone(), indices[..depth].to_vec()))
                })
        } else {
            indices.get(..depth).is_some_and(|scope| {
                context
                    .completed
                    .contains(&(selector.name.clone(), scope.to_vec()))
            })
        };
        available.then(|| self.control_count(context, indices, (selector, labels)))
    }

    fn aggregate_owner_at_scope<'a>(
        &'a self,
        name: &NodeName,
        indices: &[u64],
    ) -> Option<&'a NodeName> {
        let mut owner = self.map_owners.get(name)?;
        while self.map_depths[owner] > indices.len() {
            owner = self.map_owners.get(owner)?;
        }
        Some(owner)
    }

    pub(super) fn candidate_guard(
        &self,
        guard: &Guard,
        context: &Context,
        indices: &[u64],
    ) -> openengine_cluster_protocol::GuardTruth {
        openengine_cluster_protocol::evaluate_partial_guard(
            guard,
            &|selector, labels, aggregate| {
                self.candidate_count(context, indices, (selector, labels, aggregate))
            },
        )
    }

    pub(super) fn guard(&self, guard: &Guard, context: &Context, map_indices: &[u64]) -> bool {
        openengine_cluster_protocol::evaluate_partial_guard(guard, &|selector, labels, _| {
            Some(self.control_count(context, map_indices, (selector, labels)))
        }) == openengine_cluster_protocol::GuardTruth::True
    }

    fn control_count(
        &self,
        context: &Context,
        map_indices: &[u64],
        query: (&ControlSelector, &[EnumLabel]),
    ) -> u64 {
        let (selector, labels) = query;
        let depth = self.map_depths.get(&selector.name).copied().unwrap_or(0);
        let aggregate = depth > map_indices.len();
        context
            .controls
            .iter()
            .filter(|(key, _)| {
                key.node == selector.name
                    && key.source == selector.source
                    && key.field.as_deref() == selector.field.as_ref().map(|field| field.as_str())
                    && if aggregate {
                        key.map_indices.starts_with(map_indices) && key.map_indices.len() == depth
                    } else {
                        map_indices
                            .get(..depth)
                            .is_some_and(|indices| key.map_indices == indices)
                    }
            })
            .filter(|(_, value)| labels.iter().any(|label| label.as_str() == *value))
            .count() as u64
    }

    pub(super) fn set_control(&self, context: &mut Context, update: ControlUpdate<'_>) {
        let depth = self.map_depths.get(update.node).copied().unwrap_or(0);
        context.controls.insert(
            ControlKey {
                node: update.node.clone(),
                source: update.source,
                field: update.field.map(str::to_owned),
                map_indices: update.map_indices.iter().take(depth).copied().collect(),
            },
            update.label.to_owned(),
        );
    }

    pub(super) fn set_group_control(&self, context: &mut Context, update: GroupControlUpdate<'_>) {
        self.set_control(
            context,
            ControlUpdate {
                node: update.node,
                source: ControlSource::Group,
                field: Some(update.field),
                label: update.label,
                map_indices: update.map_indices,
            },
        );
    }

    pub(super) fn continue_decision(&mut self, node: &NodeName, mode: EvalMode) {
        if mode == EvalMode::Decide {
            self.decisions
                .push(Decision::Continue { node: node.clone() });
        }
    }

    pub(super) fn record_void_cutoff(
        &mut self,
        execution: ExecutionId,
        position: HistoryPosition,
        reason: ExecutionVoidReason,
    ) -> ExecutionVoidReason {
        let cutoff = self
            .void_cutoffs
            .entry(execution)
            .or_insert(VoidCutoff { position, reason });
        if position < cutoff.position {
            *cutoff = VoidCutoff { position, reason };
        }
        cutoff.reason
    }

    pub(super) fn void_descendants(&mut self, node: &GraphNode, scope: VoidScope<'_>) {
        if scope.mode == EvalMode::Probe {
            return;
        }
        let descendants = descendant_names(node);
        let mut losers = self
            .executions
            .iter()
            .filter(|execution| {
                self.consumed_executions.contains(&execution.execution)
                    && execution.dispatch_position <= scope.cutoff
                    && descendants.contains(&execution.occurrence.node)
                    && execution
                        .occurrence
                        .map_indices
                        .starts_with(scope.map_indices)
                    && scope.excluded_scope.is_none_or(|excluded| {
                        !execution.occurrence.map_indices.starts_with(excluded)
                    })
                    && !matches!(execution.state, DurableExecutionState::Settled { .. })
            })
            .map(|execution| {
                (
                    execution.dispatch_position,
                    execution.execution,
                    matches!(execution.state, DurableExecutionState::Active),
                )
            })
            .collect::<Vec<_>>();
        losers.sort_unstable();
        for (_, execution, active) in losers {
            let reason = self.record_void_cutoff(execution, scope.cutoff, scope.reason);
            if active && scope.mode == EvalMode::Decide {
                self.push_void_decision(execution, reason);
            }
        }
    }
}
