use super::*;
use openengine_cluster_server::graph_verifier::{FULL_V1_MAX_NODE_EXECUTIONS, FULL_V1_MAX_LOOP_ENTRIES};

impl Engine<'_> {
    pub(super) fn pending_leaf(&mut self, node: &NodeName, traversal: Traversal<'_>) -> bool {
        let key = (
            node.clone(),
            traversal.map_indices.to_vec(),
            traversal.loop_iterations.to_vec(),
        );
        if self.pending_frontier.contains(&key) {
            return true;
        }
        if self.pending_frontier.len() >= self.frontier_limit {
            return false;
        }
        self.pending_frontier.insert(key);
        true
    }

    pub(super) fn item_in_frontier(
        &self,
        body: &GraphNode,
        scope: &[u64],
        iterations: &[u64],
    ) -> bool {
        descendant_names(body).iter().any(|name| {
            self.pending_frontier
                .range((name.clone(), scope.to_vec(), Vec::new())..)
                .take_while(|(node, indices, _)| node == name && indices.starts_with(scope))
                .any(|(_, _, loops)| loops.starts_with(iterations))
        })
    }

    pub(super) fn charge_active_loops(&mut self) -> Result<(), ReducerError> {
        for (node, indices, iterations) in self.active_loops.clone() {
            self.charge_loop(&node, &indices, &iterations)?;
        }
        Ok(())
    }

    pub(super) fn charge_visit(&mut self, node: &NodeName) -> Result<(), ReducerError> {
        if self.logical_visits >= FULL_V1_MAX_NODE_EXECUTIONS {
            return Err(ReducerError::WorkLimit {
                node: node.clone(),
                bound: FULL_V1_MAX_NODE_EXECUTIONS,
                kind: "executable-node visits",
            });
        }
        self.logical_visits += 1;
        Ok(())
    }

    pub(super) fn charge_loop(
        &mut self,
        node: &NodeName,
        scope: &[u64],
        iterations: &[u64],
    ) -> Result<(), ReducerError> {
        self.loop_entries
            .record(node, scope, iterations)
            .map_err(|_| ReducerError::WorkLimit {
                node: node.clone(),
                bound: FULL_V1_MAX_LOOP_ENTRIES,
                kind: "loop-body entries",
            })
    }

    pub(super) fn probe(
        &mut self,
        node: &GraphNode,
        context: &mut Context,
        traversal: Traversal<'_>,
    ) -> Result<Status, ReducerError> {
        match self.eval(node, context, traversal) {
            Err(ReducerError::ProspectiveWorkLimit) => Ok(Status::Pending),
            other => other,
        }
    }

    pub(super) fn last_service(&self, node: &GraphNode, scope: &[u64]) -> HistoryPosition {
        let names = descendant_names(node);
        self.executions
            .iter()
            .filter(|execution| {
                names.contains(&execution.occurrence.node)
                    && execution.occurrence.map_indices.starts_with(scope)
            })
            .map(|execution| execution.dispatch_position)
            .max()
            .unwrap_or(HistoryPosition::ZERO)
    }

    // Visit markers survive loop rounds; an old index alone is never current-round history.
    pub(super) fn item_has_history(
        &self,
        body: &GraphNode,
        scope: &[u64],
        context: &Context,
    ) -> bool {
        descendant_names(body).iter().any(|name| {
            self.by_occurrence
                .range(
                    StructuralOccurrence {
                        node: name.clone(),
                        map_indices: scope.to_vec(),
                    }..,
                )
                .take_while(|(occurrence, _)| {
                    occurrence.node == *name && occurrence.map_indices.starts_with(scope)
                })
                .any(|(occurrence, executions)| {
                    executions
                        .iter()
                        .filter(|execution| execution.attempt.get() == 1)
                        .count() as u64
                        >= next_visit(name, &occurrence.map_indices, &context.controls)
                })
        })
    }

    pub(super) fn check_map_work(
        &self,
        group: &MapNode,
        items: &[Value],
        runtime: (&Context, Traversal<'_>),
    ) -> Result<(), ReducerError> {
        let (context, traversal) = runtime;
        // A mandatory leaf per item proves overflow before any of that map's work starts.
        if matches!(*group.body, GraphNode::Step(_) | GraphNode::Verifier(_)) {
            let mut fresh = 0_u64;
            for index in 0..items.len() {
                let mut scope = traversal.map_indices.to_vec();
                scope.push(index as u64);
                if !self.item_has_history(&group.body, &scope, context) {
                    fresh += 1;
                }
            }
            if self.logical_visits.saturating_add(fresh) > FULL_V1_MAX_NODE_EXECUTIONS {
                return Err(ReducerError::WorkLimit {
                    node: group.name.clone(),
                    bound: FULL_V1_MAX_NODE_EXECUTIONS,
                    kind: "executable-node visits",
                });
            }
        }
        Ok(())
    }
}

pub(super) fn collect_map_owners(
    node: &GraphNode,
    owner: Option<&NodeName>,
    owners: &mut BTreeMap<NodeName, NodeName>,
) {
    if let Some(owner) = owner {
        owners.insert(node.name().clone(), owner.clone());
    }
    let owner = if matches!(node, GraphNode::Map(_)) {
        Some(node.name())
    } else {
        owner
    };
    for child in openengine_cluster_server::graph_verifier::graph_node_children(node) {
        collect_map_owners(child, owner, owners);
    }
}

/// Contiguous rounds share one span. Rebuilt from traversal and authentic ledger prefixes.
#[derive(Clone, Default)]
pub(super) struct LoopWork {
    spans: BTreeMap<(NodeName, Vec<u64>, Vec<u64>), u64>,
    total: u64,
}
impl LoopWork {
    pub(super) fn contains(&self, node: &NodeName, scope: &[u64], iterations: &[u64]) -> bool {
        let Some((round, parents)) = iterations.split_last() else {
            return false;
        };
        self.spans
            .get(&(node.clone(), scope.to_vec(), parents.to_vec()))
            .is_some_and(|accepted| accepted >= round)
    }

    pub(super) fn record(
        &mut self,
        node: &NodeName,
        scope: &[u64],
        iterations: &[u64],
    ) -> Result<(), ()> {
        let (round, parents) = iterations.split_last().ok_or(())?;
        let key = (node.clone(), scope.to_vec(), parents.to_vec());
        self.span(key, *round)
    }
    fn span(&mut self, key: (NodeName, Vec<u64>, Vec<u64>), round: u64) -> Result<(), ()> {
        let previous = self.spans.get(&key).copied().unwrap_or(0);
        let total = self.total.saturating_add(round.saturating_sub(previous));
        if total > FULL_V1_MAX_LOOP_ENTRIES {
            return Err(());
        }
        self.spans.insert(key, previous.max(round));
        self.total = total;
        Ok(())
    }
    pub(super) fn merge(&mut self, other: &Self) -> Result<(), ()> {
        for (key, round) in &other.spans {
            self.span(key.clone(), *round)?;
        }
        Ok(())
    }
}
