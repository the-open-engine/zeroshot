//! Per-item write witnesses use the existing correlated outcome and saturated count machinery.
use super::*;

impl Analyzer<'_> {
    pub(in crate::graph_verifier) fn prepare_write_predicates(
        &mut self,
        guards: &[&Guard],
    ) -> Option<()> {
        let mut pending = Vec::new();
        self.queue_write_guards(guards.iter().copied(), &mut pending)?;
        let mut seen = BTreeSet::new();
        while let Some(guard) = pending.pop() {
            for selector in guard_selectors(&guard) {
                let Some(index) = map_write_index(selector.field.as_ref()) else {
                    continue;
                };
                if selector.source == ControlSource::Group
                    && seen.insert((selector.name.clone(), index))
                {
                    self.register_write_predicate(selector, index, &mut pending)?;
                }
            }
        }
        Some(())
    }

    fn queue_write_guards<'g>(
        &self,
        guards: impl IntoIterator<Item = &'g Guard>,
        pending: &mut Vec<Guard>,
    ) -> Option<()> {
        for guard in guards {
            self.proof_work
                .charge(1 + guard_node_count(guard) as usize)?;
            pending.push(guard.clone());
        }
        Some(())
    }

    fn register_write_predicate(
        &mut self,
        selector: &ControlSelector,
        index: usize,
        pending: &mut Vec<Guard>,
    ) -> Option<()> {
        let Some(correlation) = self.map_execution_correlations.get(&selector.name) else {
            return Some(());
        };
        let GraphNode::Map(map) = self.nodes.get(&correlation.owner)?.node else {
            return None;
        };
        let path = map.promoted_state_paths.get(index)?;
        let write = self.write_predicate(&map.body, path)?;
        let completion = self
            .node_completion
            .get(map.body.name())
            .cloned()
            .unwrap_or_default();
        let predicate = CompletionPredicate::all([completion, write]);
        let mut dependencies = Vec::new();
        predicate.collect_guards(&mut dependencies);
        self.queue_write_guards(dependencies, pending)?;
        self.map_execution_correlations
            .get_mut(&selector.name)?
            .item_predicates
            .insert(selector.field.clone()?, predicate);
        Some(())
    }

    fn write_predicate(&self, node: &GraphNode, path: &FieldPath) -> Option<CompletionPredicate> {
        self.proof_work.charge(1 + path.segments().len())?;
        match node {
            GraphNode::Step(step) => {
                return self.leaf_write_predicate(node, &step.write_bindings, path);
            }
            GraphNode::Verifier(verifier) => {
                return self.leaf_write_predicate(node, &verifier.write_bindings, path);
            }
            GraphNode::Map(map) => return self.map_collection_write_predicate(map, path),
            _ => {}
        }
        self.promoted_write_predicate(node, path)
    }

    fn promoted_write_predicate(
        &self,
        node: &GraphNode,
        path: &FieldPath,
    ) -> Option<CompletionPredicate> {
        let paths = match node {
            GraphNode::Seq(group) => &group.promoted_state_paths,
            GraphNode::Choice(group) => &group.promoted_state_paths,
            GraphNode::Par(group) => &group.promoted_state_paths,
            GraphNode::Loop(group) => &group.promoted_state_paths,
            _ => return Some(CompletionPredicate::Never),
        };
        let mut writes = Vec::new();
        for promoted in paths
            .iter()
            .filter(|promoted| write_path_overlap(promoted, path))
        {
            let required = if promoted.segments().len() > path.segments().len() {
                promoted
            } else {
                path
            };
            writes.push(self.child_write_predicate(node, required)?);
        }
        Some(CompletionPredicate::any(writes))
    }

    fn map_collection_write_predicate(
        &self,
        map: &MapNode,
        path: &FieldPath,
    ) -> Option<CompletionPredicate> {
        Some(CompletionPredicate::any(
            map.promoted_state_paths
                .iter()
                .enumerate()
                .filter(|(_, promoted)| write_path_overlap(promoted, path))
                .map(|(index, _)| map_write_guard(map, index).map(CompletionPredicate::Guard))
                .collect::<Option<Vec<_>>>()?,
        ))
    }

    fn leaf_write_predicate(
        &self,
        node: &GraphNode,
        bindings: &[openengine_cluster_protocol::WriteBinding],
        path: &FieldPath,
    ) -> Option<CompletionPredicate> {
        if !bindings
            .iter()
            .any(|binding| write_path_overlap(&binding.target, path))
        {
            return Some(CompletionPredicate::Never);
        }
        Some(CompletionPredicate::not(CompletionPredicate::Guard(
            Guard::In {
                value: ControlSelector {
                    name: node.name().clone(),
                    source: ControlSource::Error,
                    field: None,
                },
                labels: openengine_cluster_protocol::NonEmptyEnumSet::new(error_labels()).ok()?,
            },
        )))
    }

    fn child_write_predicate(
        &self,
        node: &GraphNode,
        path: &FieldPath,
    ) -> Option<CompletionPredicate> {
        let routes = match node {
            GraphNode::Choice(choice) => choice_branch_completion_predicates(choice)
                .into_iter()
                .chain(std::iter::once(choice_otherwise_completion_predicate(
                    choice,
                )))
                .collect(),
            GraphNode::Par(parallel) => (0..parallel.branches.as_slice().len())
                .map(|index| retention_guard(parallel, index))
                .collect::<Option<Vec<_>>>()?,
            _ => vec![CompletionPredicate::Always; child_nodes(node).len()],
        };
        Some(CompletionPredicate::any(
            child_nodes(node)
                .into_iter()
                .zip(routes)
                .map(|(child, route)| {
                    Some(CompletionPredicate::all([
                        route,
                        self.write_predicate(child, path)?,
                    ]))
                })
                .collect::<Option<Vec<_>>>()?,
        ))
    }
}

fn write_path_overlap(left: &FieldPath, right: &FieldPath) -> bool {
    left.segments().starts_with(right.segments()) || right.segments().starts_with(left.segments())
}
