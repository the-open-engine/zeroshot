use super::*;

impl Engine<'_> {
    fn candidate_count(
        &self,
        context: &Context,
        indices: &[u64],
        query: (&ControlSelector, &[EnumLabel], bool, bool),
    ) -> Option<u64> {
        let (selector, labels, aggregate, frozen) = query;
        // Private proof controls are derived from correlated outcomes, not runtime control labels.
        if selector.source == ControlSource::Group
            && selector
                .field
                .as_ref()
                .is_some_and(|field| !matches!(field.as_str(), "joined" | "raced" | "terminated"))
        {
            return None;
        }
        if !frozen && self.provisional(context, &selector.name, indices) {
            return None;
        }
        let depth = *self.map_depths.get(&selector.name)?;
        let available = if aggregate {
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
        available.then(|| {
            self.control_values(selector, context, indices)
                .iter()
                .filter(|value| labels.iter().any(|label| label.as_str() == *value))
                .count() as u64
        })
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
                self.candidate_count(context, indices, (selector, labels, aggregate, false))
            },
        )
    }

    pub(super) fn candidate_viable(
        &self,
        guard: &Guard,
        context: &Context,
        scope: (&GraphNode, Traversal<'_>),
    ) -> bool {
        let (branch, traversal) = scope;
        let mut snapshot = ResidualSnapshot::new(self, context);
        if self.selector_work.charge(self.map_depths.len()).is_none() {
            return true;
        }
        let names = descendant_names(branch);
        let alternatives = context
            .loop_exits
            .iter()
            .filter(|((name, scope), _)| {
                (traversal.map_indices.starts_with(scope)
                    || scope.starts_with(traversal.map_indices))
                    && names.contains(name)
            })
            .map(|(key, exits)| (key, exits.as_slice()))
            .collect::<Vec<_>>();
        snapshot
            .possible_exits((branch, guard, traversal.map_indices), &alternatives)
            .unwrap_or(true)
    }

    pub(super) fn loop_may_reenter(
        &self,
        selected: (&openengine_cluster_protocol::LoopNode, Option<&Guard>),
        context: &Context,
        traversal: Traversal<'_>,
    ) -> bool {
        let (group, write) = selected;
        let snapshot = ResidualSnapshot::new(self, context);
        openengine_cluster_server::graph_verifier::loop_reentry_possible(
            &group.body,
            (group.until.as_ref(), write),
            &snapshot,
            traversal.map_indices,
        )
    }

    pub(super) fn loop_may_converge(
        &self,
        group: &openengine_cluster_protocol::LoopNode,
        context: &Context,
        traversal: Traversal<'_>,
    ) -> bool {
        let Some(until) = &group.until else {
            return false;
        };
        if self.selector_work.charge_guard(until).is_none() {
            return true;
        }
        let negated = Guard::Not {
            guard: Box::new(until.clone()),
        };
        let snapshot = ResidualSnapshot::new(self, context);
        openengine_cluster_server::graph_verifier::loop_reentry_possible(
            &group.body,
            (Some(&negated), None),
            &snapshot,
            traversal.map_indices,
        )
    }

    pub(super) fn provisional(&self, context: &Context, name: &NodeName, indices: &[u64]) -> bool {
        self.provisional_except(context, (name, indices), &[])
    }

    fn provisional_except(
        &self,
        context: &Context,
        subject: (&NodeName, &[u64]),
        fixed: &[(&(NodeName, Vec<u64>), &future::LoopExit)],
    ) -> bool {
        let (name, indices) = subject;
        context.revisitable.iter().any(|(loop_name, scope)| {
            !fixed
                .iter()
                .any(|(key, exit)| exit.current && key.0 == *loop_name && key.1 == *scope)
                && indices.starts_with(scope)
                && find_descendant(self.root, loop_name)
                    .is_some_and(|node| descendant_names(node).contains(name))
        })
    }

    pub(super) fn guard(
        &self,
        guard: &Guard,
        context: &Context,
        map_indices: &[u64],
    ) -> Result<bool, ReducerError> {
        match guard {
            Guard::In { value, labels } => Ok(self
                .control_values(value, context, map_indices)
                .iter()
                .any(|actual| labels.values().iter().any(|label| label.as_str() == actual))),
            Guard::All { guards } => guards.as_slice().iter().try_fold(true, |matches, guard| {
                Ok(matches && self.guard(guard, context, map_indices)?)
            }),
            Guard::Any { guards } => guards.as_slice().iter().try_fold(false, |matches, guard| {
                Ok(matches || self.guard(guard, context, map_indices)?)
            }),
            Guard::Not { guard } => Ok(!self.guard(guard, context, map_indices)?),
            Guard::KOfN {
                count,
                values,
                labels,
            } => Ok(values
                .as_slice()
                .iter()
                .filter(|selector| {
                    self.control_values(selector, context, map_indices)
                        .iter()
                        .any(|actual| labels.values().iter().any(|label| label.as_str() == actual))
                })
                .count() as u64
                >= count.get()),
            Guard::KOfMap {
                count,
                value,
                labels,
            } => Ok(self
                .control_values(value, context, map_indices)
                .iter()
                .filter(|actual| {
                    labels
                        .values()
                        .iter()
                        .any(|label| label.as_str() == *actual)
                })
                .count() as u64
                >= count.get()),
        }
    }

    pub(super) fn control_values(
        &self,
        selector: &ControlSelector,
        context: &Context,
        map_indices: &[u64],
    ) -> Vec<String> {
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
            .map(|(_, value)| value.clone())
            .collect()
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

    pub(super) fn void_active_descendants(&mut self, node: &GraphNode, scope: VoidScope<'_>) {
        let descendants = descendant_names(node);
        let mut losers = self
            .executions
            .iter()
            .filter(|execution| {
                self.consumed_executions.contains(&execution.execution)
                    && descendants.contains(&execution.occurrence.node)
                    && execution
                        .occurrence
                        .map_indices
                        .starts_with(scope.map_indices)
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
            if active && scope.emit_decisions {
                self.push_void_decision(execution, reason);
            }
        }
    }

    pub(super) fn void_map_losers(&mut self, body: &GraphNode, scope: MapVoidScope<'_>) {
        let descendants = descendant_names(body);
        let mut losers = self
            .executions
            .iter()
            .filter(|execution| {
                self.consumed_executions.contains(&execution.execution)
                    && descendants.contains(&execution.occurrence.node)
                    && execution
                        .occurrence
                        .map_indices
                        .starts_with(scope.common.map_indices)
                    && !execution
                        .occurrence
                        .map_indices
                        .starts_with(scope.winner_scope)
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
            let reason =
                self.record_void_cutoff(execution, scope.common.cutoff, scope.common.reason);
            if active && scope.common.emit_decisions {
                self.push_void_decision(execution, reason);
            }
        }
    }
}

fn find_descendant<'a>(node: &'a GraphNode, name: &NodeName) -> Option<&'a GraphNode> {
    if node.name() == name {
        return Some(node);
    }
    openengine_cluster_server::graph_verifier::graph_node_children(node)
        .into_iter()
        .find_map(|child| find_descendant(child, name))
}

type LoopExitAlternatives<'a> = (&'a (NodeName, Vec<u64>), &'a [future::LoopExit]);

struct ResidualSnapshot<'a, 'run> {
    engine: &'a Engine<'run>,
    context: &'a Context,
    exits: Vec<(&'a (NodeName, Vec<u64>), &'a future::LoopExit)>,
}

impl<'a, 'run> ResidualSnapshot<'a, 'run> {
    fn new(engine: &'a Engine<'run>, context: &'a Context) -> Self {
        Self {
            engine,
            context,
            exits: vec![],
        }
    }
    fn provisional(&self, name: &NodeName, scope: &[u64]) -> bool {
        self.engine
            .provisional_except(self.context, (name, scope), &self.exits)
    }
    fn map_size(&self, key: &(NodeName, Vec<u64>)) -> Option<&MapCardinality> {
        self.exits
            .iter()
            .rev()
            .find_map(|(_, exit)| exit.sizes.get(key))
            .or_else(|| self.context.map_sizes.get(key))
    }
    fn projected_scopes<'s>(
        &'s self,
        name: &'s NodeName,
        scope: &'s [u64],
    ) -> impl Iterator<Item = &'s Vec<u64>> {
        std::iter::once(&self.context.map_sizes)
            .chain(self.exits.iter().map(|(_, exit)| &exit.sizes))
            .flat_map(move |sizes| {
                sizes
                    .range((name.clone(), scope.to_vec())..)
                    .take_while(move |((node, indices), _)| {
                        node == name && indices.starts_with(scope)
                    })
                    .map(|((_, indices), _)| indices)
            })
    }

    fn possible_exits(
        &mut self,
        query: (&GraphNode, &Guard, &[u64]),
        alternatives: &[LoopExitAlternatives<'a>],
    ) -> Option<bool> {
        self.engine.selector_work.charge(1 + alternatives.len())?;
        let mut positions = vec![0_usize; alternatives.len()];
        loop {
            self.engine.selector_work.charge(1 + alternatives.len())?;
            self.exits = positions
                .iter()
                .zip(alternatives)
                .filter_map(|(position, (key, exits))| {
                    position.checked_sub(1).map(|index| (*key, &exits[index]))
                })
                .collect();
            if openengine_cluster_server::graph_verifier::guard_has_possible_future(
                query.0, query.1, self, query.2,
            ) {
                return Some(true);
            }
            if !advance_exit_selection(&mut positions, alternatives) {
                return Some(false);
            }
        }
    }
}

impl openengine_cluster_server::graph_verifier::ControlSnapshot for ResidualSnapshot<'_, '_> {
    fn proof_work(&self) -> openengine_cluster_server::graph_verifier::ProofWork {
        self.engine.selector_work.clone()
    }
    fn is_fixed(&self, node: &NodeName, scope: &[u64]) -> bool {
        let depth = self.engine.map_depths.get(node).copied().unwrap_or(0);
        !self.provisional(node, scope)
            && scope.get(..depth).is_some_and(|scope| {
                self.context
                    .completed
                    .contains(&(node.clone(), scope.to_vec()))
            })
    }

    fn retained(&self, node: &NodeName, scope: &[u64], index: usize) -> Option<bool> {
        let depth = *self.engine.map_depths.get(node)?;
        self.context
            .retained
            .get(&(node.clone(), scope.get(..depth)?.to_vec()))
            .map(|selected| selected.contains(&index))
    }

    fn fixed_count(
        &self,
        query: (&ControlSelector, &[EnumLabel], bool),
        scope: &[u64],
    ) -> Option<u64> {
        self.engine.candidate_count(
            self.context,
            scope,
            (
                query.0,
                query.1,
                query.2,
                !self.provisional(&query.0.name, scope),
            ),
        )
    }

    fn label(&self, selector: &ControlSelector, scope: &[u64]) -> Option<EnumLabel> {
        let depth = *self.engine.map_depths.get(&selector.name)?;
        self.context
            .controls
            .get(&ControlKey {
                node: selector.name.clone(),
                source: selector.source,
                field: selector
                    .field
                    .as_ref()
                    .map(|field| field.as_str().to_owned()),
                map_indices: scope.get(..depth)?.to_vec(),
            })
            .and_then(|label| label.parse().ok())
    }

    fn selection_guards(&self) -> Box<dyn Iterator<Item = &Guard> + '_> {
        Box::new(
            self.context
                .map_sizes
                .keys()
                .chain(self.exits.iter().flat_map(|(_, exit)| exit.sizes.keys()))
                .filter_map(|key| self.map_size(key))
                .flat_map(|cardinality| {
                    cardinality
                        .writes
                        .iter()
                        .filter_map(|write| write.condition.as_ref())
                })
                .chain(self.exits.iter().map(|(_, exit)| &exit.convergence)),
        )
    }

    fn scoped_guards<'s>(&'s self, scope: &'s [u64]) -> Box<dyn Iterator<Item = &'s Guard> + 's> {
        Box::new(
            self.exits
                .iter()
                .filter(move |(key, _)| key.1 == scope)
                .map(|(_, exit)| &exit.convergence),
        )
    }

    fn map_progress(
        &self,
        map: &MapNode,
        scope: &[u64],
    ) -> openengine_cluster_server::graph_verifier::MapProgress {
        let provisional = self.provisional(&map.name, scope);
        let cardinality = (0..=scope.len())
            .rev()
            .find_map(|depth| self.map_size(&(map.name.clone(), scope[..depth].to_vec())))
            .cloned()
            .unwrap_or_default();
        let names = descendant_names(&map.body);
        let known_items = if provisional {
            vec![]
        } else {
            names
                .iter()
                .flat_map(|name| {
                    let start = (name.clone(), scope.to_vec());
                    self.context
                        .completed
                        .range(start.clone()..)
                        .take_while(move |(node, indices)| {
                            node == name && indices.starts_with(scope)
                        })
                        .map(|(_, indices)| indices)
                        .chain(self.projected_scopes(name, scope))
                })
                .filter_map(|indices| indices.get(scope.len()).copied())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        };
        openengine_cluster_server::graph_verifier::MapProgress {
            cardinality,
            known_items,
        }
    }
}

fn advance_exit_selection(
    positions: &mut [usize],
    alternatives: &[LoopExitAlternatives<'_>],
) -> bool {
    for (position, (_, exits)) in positions.iter_mut().zip(alternatives).rev() {
        if *position < exits.len() {
            *position += 1;
            return true;
        }
        *position = 0;
    }
    false
}
