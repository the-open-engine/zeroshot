//! Project compact future selectors from suspended local state. Writes retain their selected
//! choice conditions and follow the same ordering and promotion boundaries as replay.
use super::*;

pub(super) mod exits;
pub(super) use exits::LoopExit;

#[derive(Clone, Default, PartialEq)]
pub(super) struct FutureWrites {
    changes: Vec<(FieldPath, CollectionWrite)>,
    known: BTreeMap<FieldPath, u64>,
}

impl FutureWrites {
    fn add(&mut self, path: FieldPath, condition: Option<Guard>) {
        self.add_write(
            path,
            CollectionWrite {
                condition,
                lengths: None,
            },
        );
    }
    fn add_write(&mut self, path: FieldPath, write: CollectionWrite) {
        let replacement = (path, write);
        if self.changes.last() != Some(&replacement) {
            self.changes.push(replacement);
        }
    }
    fn extend(&mut self, other: Self) {
        self.known.extend(other.known);
        for (path, write) in other.changes {
            self.add_write(path, write);
        }
    }
    fn after(mut self, prefix: &Self) -> Self {
        self.changes.drain(..prefix.changes.len());
        self.known
            .retain(|path, size| prefix.known.get(path) != Some(size));
        self
    }
    fn under(mut self, condition: Option<Guard>) -> Self {
        if condition.is_some() {
            let changes = std::mem::take(&mut self.changes);
            for (path, length) in std::mem::take(&mut self.known) {
                self.add_write(
                    path,
                    CollectionWrite {
                        condition: None,
                        lengths: Some(BTreeSet::from([length])),
                    },
                );
            }
            self.changes.extend(changes);
        }
        for (_, write) in &mut self.changes {
            write.condition = together(write.condition.clone(), condition.clone());
        }
        self
    }
    fn promoted(&self, paths: &[FieldPath]) -> Self {
        let mut result = Self::default();
        result.known.extend(
            self.known
                .iter()
                .filter(|(path, _)| {
                    paths
                        .iter()
                        .any(|promoted| path.segments().starts_with(promoted.segments()))
                })
                .map(|(path, size)| (path.clone(), *size)),
        );
        for (written, replacement) in &self.changes {
            for path in promoted_write_paths(&BTreeSet::from([written.clone()]), paths) {
                let mut write = replacement.clone();
                if &path != written {
                    write.lengths = None;
                }
                result.add_write(path, write);
            }
        }
        result
    }
    fn cardinality(&self, selector: &DataSelector, length: Option<u64>) -> MapCardinality {
        let DataSelector::State { path } = selector else {
            return MapCardinality {
                length,
                writes: vec![],
            };
        };
        MapCardinality {
            length: self.known.get(path).copied().or(length),
            writes: self
                .changes
                .iter()
                .filter(|(written, _)| paths_overlap(path, written))
                .map(|(written, replacement)| {
                    let mut write = replacement.clone();
                    if written != path {
                        write.lengths = None;
                    }
                    write
                })
                .collect(),
        }
    }
}

impl Engine<'_> {
    // Queued items retain selector sizes, never their caller state or item payloads. Speculative
    // projection spends the shared count-cell engineering budget and leaves excess unknown.
    fn project_item_maps(
        &self,
        node: &GraphNode,
        occurrence: (&Value, &[u64]),
        sizes: &mut MapSizes,
    ) -> Option<()> {
        let (item, indices) = occurrence;
        self.selector_work.charge(1 + indices.len())?;
        if let GraphNode::Map(map) = node {
            if !matches!(map.over, DataSelector::Item { .. }) {
                return Some(());
            }
            let items = select_data(&map.over, &Value::Null, Some(item))
                .ok()?
                .as_array()?;
            sizes.insert(
                (map.name.clone(), indices.to_vec()),
                MapCardinality::fixed(items.len() as u64),
            );
            for (index, child) in items.iter().enumerate() {
                let mut scope = indices.to_vec();
                scope.push(index as u64);
                self.project_item_maps(&map.body, (child, &scope), sizes)?;
            }
        } else {
            for child in openengine_cluster_server::graph_verifier::graph_node_children(node) {
                self.project_item_maps(child, (item, indices), sizes)?;
            }
        }
        Some(())
    }

    pub(super) fn project_queued_item_maps(
        &self,
        body: &GraphNode,
        items: &[Value],
        pending: (&[u64], usize),
    ) -> MapSizes {
        let (scope, start) = pending;
        let mut sizes = BTreeMap::new();
        for (index, item) in items.iter().enumerate().skip(start) {
            let mut indices = scope.to_vec();
            indices.push(index as u64);
            if self
                .project_item_maps(body, (item, &indices), &mut sizes)
                .is_none()
            {
                break;
            }
        }
        sizes
    }

    pub(super) fn mark_future_flow(
        &self,
        evaluation: GroupMutation<'_, GraphNode>,
        writes: &mut FutureWrites,
        condition: Option<Guard>,
    ) {
        let GroupMutation {
            group: node,
            context,
            traversal,
        } = evaluation;
        if let GraphNode::Map(map) = node {
            self.mark_future_map((map, condition), context, (traversal, writes));
            return;
        }
        let mut local = writes.clone();
        match node {
            GraphNode::Seq(group) => {
                for child in group.children.as_slice() {
                    self.mark_future_flow(
                        GroupMutation {
                            group: child,
                            context,
                            traversal,
                        },
                        &mut local,
                        condition.clone(),
                    );
                }
            }
            GraphNode::Loop(group) => {
                self.project_future_rounds(
                    (group, group.max_iterations.get()),
                    (context, traversal),
                    &mut local,
                );
            }
            GraphNode::Choice(_) | GraphNode::Par(_) => {
                for (child, route) in self.future_children(node, context, traversal) {
                    let mut branch = writes.clone();
                    self.mark_future_flow(
                        GroupMutation {
                            group: child,
                            context,
                            traversal,
                        },
                        &mut branch,
                        together(condition.clone(), route),
                    );
                    local.extend(branch.after(writes));
                }
            }
            _ => local.extend(leaf_write_paths(node).under(condition)),
        }
        writes.extend(match promoted_paths(node) {
            Some(paths) => local.after(writes).promoted(paths),
            None => local.after(writes),
        });
    }

    pub(super) fn project_sequence_tail(
        &self,
        selected: (&openengine_cluster_protocol::SeqNode, usize),
        context: &mut Context,
        traversal: Traversal<'_>,
    ) {
        let (group, index) = selected;
        let mut remaining =
            self.future_writes(&group.children.as_slice()[index], context, traversal);
        for later in &group.children.as_slice()[index + 1..] {
            self.mark_future_flow(
                GroupMutation {
                    group: later,
                    context,
                    traversal,
                },
                &mut remaining,
                None,
            );
        }
        self.record_pending_writes(
            (&group.name, &group.promoted_state_paths),
            (remaining, context),
            traversal,
        );
    }

    pub(super) fn record_pending_writes(
        &self,
        group: (&NodeName, &[FieldPath]),
        pending: (FutureWrites, &mut Context),
        traversal: Traversal<'_>,
    ) {
        let (remaining, context) = pending;
        let mut projected = self.settled_collection_sizes(context);
        projected.extend(remaining);
        context.pending_writes.insert(
            (group.0.clone(), traversal.map_indices.to_vec()),
            projected.promoted(group.1),
        );
    }

    // Suspension snapshots contain selector lengths only. Unknown remaining writes still carry
    // their joint route predicates; complete caller values never enter the retained item facts.
    fn settled_collection_sizes(&self, context: &Context) -> FutureWrites {
        let mut sizes = FutureWrites::default();
        let mut pending = vec![self.root];
        while let Some(node) = pending.pop() {
            if self.selector_work.charge(1).is_none() {
                for path in &context.local_writes {
                    sizes.add(path.clone(), None);
                }
                break;
            }
            pending.extend(openengine_cluster_server::graph_verifier::graph_node_children(node));
            let GraphNode::Map(map) = node else {
                continue;
            };
            let DataSelector::State { path } = &map.over else {
                continue;
            };
            if !context
                .local_writes
                .iter()
                .any(|written| path.segments().starts_with(written.segments()))
            {
                continue;
            }
            if let Ok(Value::Array(items)) = select(&context.state, path) {
                sizes.known.insert(path.clone(), items.len() as u64);
            }
        }
        sizes
    }

    pub(super) fn project_loop_reentry(
        &self,
        group: &openengine_cluster_protocol::LoopNode,
        context: &mut Context,
        traversal: Traversal<'_>,
    ) {
        context
            .loop_exits
            .remove(&(group.name.clone(), traversal.map_indices.to_vec()));
        self.record_loop_exit(group, (context, traversal), true);
        // Resolve observed routes before making this round's producers replaceable.
        let mut writes = self.future_loop_writes(group, context, traversal);
        context
            .revisitable
            .insert((group.name.clone(), traversal.map_indices.to_vec()));
        let remaining = group
            .max_iterations
            .get()
            .saturating_sub(traversal.loop_iterations.last().copied().unwrap_or(1));
        self.project_future_rounds((group, remaining), (context, traversal), &mut writes);
    }

    pub(super) fn future_loop_writes(
        &self,
        group: &openengine_cluster_protocol::LoopNode,
        context: &Context,
        traversal: Traversal<'_>,
    ) -> FutureWrites {
        self.loop_carried_writes(group, (context, traversal), &FutureWrites::default())
    }

    fn project_future_rounds(
        &self,
        selected: (&openengine_cluster_protocol::LoopNode, u64),
        scope: (&mut Context, Traversal<'_>),
        writes: &mut FutureWrites,
    ) {
        let (group, rounds) = selected;
        let (context, traversal) = scope;
        for round in 0..rounds {
            let mut local = writes.clone();
            self.mark_future_flow(
                GroupMutation {
                    group: &group.body,
                    context,
                    traversal,
                },
                &mut local,
                None,
            );
            if round + 1 == rounds || !self.loop_may_reenter((group, None), context, traversal) {
                *writes = local;
                break;
            }
            self.record_loop_exit(group, (context, traversal), false);
            let next = self.loop_carried_writes(group, (context, traversal), writes);
            if next == *writes {
                *writes = local;
                break;
            }
            *writes = next;
        }
    }

    fn loop_carried_writes(
        &self,
        group: &openengine_cluster_protocol::LoopNode,
        scope: (&Context, Traversal<'_>),
        prior: &FutureWrites,
    ) -> FutureWrites {
        let (context, traversal) = scope;
        let mut carried = FutureWrites::default();
        // Carried writes only affect future map selections. Graphs without maps have no
        // collection cardinalities to widen, while control revisitation is handled separately.
        if !self.map_owners.is_empty()
            && group.max_iterations.get() > 1
            && self.loop_may_reenter((group, None), context, traversal)
        {
            let writes = self.future_writes_after(&group.body, (context, traversal), prior);
            let Some(paths) = self.collection_paths() else {
                let mut unknown = prior.clone();
                unknown.extend(writes);
                for path in std::mem::take(&mut unknown.known).into_keys() {
                    unknown.add(path, None);
                }
                for (_, replacement) in &mut unknown.changes {
                    replacement.lengths = None;
                }
                return unknown;
            };
            for path in paths {
                let selector = DataSelector::State { path: path.clone() };
                let current = select(&context.state, &path)
                    .ok()
                    .and_then(|value| value.as_array().map(|items| items.len() as u64));
                let before = prior.cardinality(&selector, current);
                let baseline = CollectionWrite::possible_lengths(before.length, &before.writes);
                let projected = writes.cardinality(&selector, before.length);
                let baseline = writes
                    .known
                    .get(&path)
                    .map(|length| BTreeSet::from([*length]))
                    .or(baseline);
                let lengths = self.carried_collection_lengths(
                    (group, &projected.writes),
                    baseline,
                    (context, traversal),
                );
                carried.add_write(
                    path,
                    CollectionWrite {
                        condition: None,
                        lengths,
                    },
                );
            }
        }
        carried
    }

    fn collection_paths(&self) -> Option<BTreeSet<FieldPath>> {
        let mut paths = BTreeSet::new();
        let mut pending = vec![self.root];
        while let Some(node) = pending.pop() {
            self.selector_work.charge(1)?;
            if let GraphNode::Map(map) = node {
                if let DataSelector::State { path } = &map.over {
                    paths.insert(path.clone());
                }
            }
            pending.extend(openengine_cluster_server::graph_verifier::graph_node_children(node));
        }
        Some(paths)
    }

    fn carried_collection_lengths(
        &self,
        selected: (&openengine_cluster_protocol::LoopNode, &[CollectionWrite]),
        baseline: Option<BTreeSet<u64>>,
        scope: (&Context, Traversal<'_>),
    ) -> Option<BTreeSet<u64>> {
        let (group, changes) = selected;
        let (context, traversal) = scope;
        let mut lengths = BTreeSet::new();
        let mut residual = None;
        for write in changes.iter().rev() {
            self.charge_write_predicate(&residual)?;
            let witness = together(residual.clone(), write.condition.clone());
            if self.loop_may_reenter((group, witness.as_ref()), context, traversal) {
                let values = write.lengths.as_ref()?;
                self.selector_work.charge(1 + values.len())?;
                lengths.extend(values);
            }
            let Some(guard) = &write.condition else {
                return Some(lengths);
            };
            self.selector_work.charge_guard(guard)?;
            residual = together(
                residual,
                Some(Guard::Not {
                    guard: Box::new(guard.clone()),
                }),
            );
        }
        if self.loop_may_reenter((group, residual.as_ref()), context, traversal) {
            lengths.extend(baseline?);
        }
        Some(lengths)
    }

    fn charge_write_predicate(&self, guard: &Option<Guard>) -> Option<()> {
        if let Some(guard) = guard {
            self.selector_work.charge_guard(guard)?;
        }
        Some(())
    }

    fn map_future_writes(
        &self,
        map: &MapNode,
        scope: (&Context, Traversal<'_>),
        prior: &FutureWrites,
    ) -> FutureWrites {
        let (context, traversal) = scope;
        if map.promoted_state_paths.is_empty() {
            return FutureWrites::default();
        }
        let cardinality = self.prospective_map_cardinality(map, (context, traversal), prior);
        let mut writes = FutureWrites::default();
        for (index, path) in map.promoted_state_paths.iter().enumerate() {
            if let Some(guard) =
                openengine_cluster_server::graph_verifier::map_write_guard(map, index)
            {
                writes.add_write(
                    path.clone(),
                    CollectionWrite {
                        condition: Some(guard.clone()),
                        lengths: cardinality.length.map(|length| BTreeSet::from([length])),
                    },
                );
                for source in &cardinality.writes {
                    writes.add_write(
                        path.clone(),
                        CollectionWrite {
                            condition: together(Some(guard.clone()), source.condition.clone()),
                            lengths: source.lengths.clone(),
                        },
                    );
                }
            }
        }
        writes
    }

    fn prospective_map_cardinality(
        &self,
        map: &MapNode,
        scope: (&Context, Traversal<'_>),
        prior: &FutureWrites,
    ) -> MapCardinality {
        let (context, traversal) = scope;
        if !self.provisional(context, &map.name, traversal.map_indices) {
            if let Some(cardinality) = context
                .map_sizes
                .get(&(map.name.clone(), traversal.map_indices.to_vec()))
            {
                return cardinality.clone();
            }
        }
        let length = collection_length(&map.over, &context.state, traversal.item);
        prior.cardinality(&map.over, length)
    }

    fn mark_future_map(
        &self,
        selected: (&MapNode, Option<Guard>),
        context: &mut Context,
        projection: (Traversal<'_>, &mut FutureWrites),
    ) {
        let (map, condition) = selected;
        let (traversal, writes) = projection;
        let length = collection_length(&map.over, &context.state, traversal.item);
        let names = descendant_names(&map.body);
        // A queued-item template is not a replacement for already selected item occurrences.
        if self.map_depths[&map.name] == traversal.map_indices.len() {
            context.map_sizes.retain(|(name, scope), _| {
                !names.contains(name) || !scope.starts_with(traversal.map_indices)
            });
        }
        context.map_sizes.insert(
            (map.name.clone(), traversal.map_indices.to_vec()),
            writes.cardinality(&map.over, length),
        );
        let mut local = writes.clone();
        self.mark_future_flow(
            GroupMutation {
                group: &map.body,
                context,
                traversal: Traversal {
                    item: None,
                    ..traversal
                },
            },
            &mut local,
            condition.clone(),
        );
        let stable = writes.cardinality(&map.over, length);
        if stable.length.is_some() && stable.writes.is_empty() {
            let projected = select_data(&map.over, &context.state, traversal.item)
                .ok()
                .and_then(Value::as_array)
                .map_or_else(BTreeMap::new, |items| {
                    self.project_queued_item_maps(&map.body, items, (traversal.map_indices, 0))
                });
            context.map_sizes.extend(projected);
        }
        writes.extend(
            self.map_future_writes(map, (context, traversal), writes)
                .under(condition),
        );
    }

    pub(super) fn future_writes(
        &self,
        node: &GraphNode,
        context: &Context,
        traversal: Traversal<'_>,
    ) -> FutureWrites {
        self.future_writes_after(node, (context, traversal), &FutureWrites::default())
    }

    fn future_writes_after(
        &self,
        node: &GraphNode,
        scope: (&Context, Traversal<'_>),
        prior: &FutureWrites,
    ) -> FutureWrites {
        let (context, traversal) = scope;
        if !self.provisional(context, node.name(), traversal.map_indices) {
            if let Some(writes) = context
                .pending_writes
                .get(&(node.name().clone(), traversal.map_indices.to_vec()))
            {
                return writes.clone();
            }
        }
        if self.candidate_node_fixed(node, context, traversal) {
            return FutureWrites::default();
        }
        if let GraphNode::Map(map) = node {
            return self.map_future_writes(map, (context, traversal), prior);
        }
        let mut writes = leaf_write_paths(node);
        for (child, condition) in self.future_children(node, context, traversal) {
            let mut prefix = prior.clone();
            if matches!(node, GraphNode::Seq(_)) {
                prefix.extend(writes.clone());
            }
            writes.extend(
                self.future_writes_after(child, scope, &prefix)
                    .under(condition),
            );
        }
        promoted_paths(node).map_or_else(|| writes.clone(), |paths| writes.promoted(paths))
    }

    fn candidate_node_fixed(
        &self,
        node: &GraphNode,
        context: &Context,
        traversal: Traversal<'_>,
    ) -> bool {
        !self.provisional(context, node.name(), traversal.map_indices)
            && context
                .completed
                .contains(&(node.name().clone(), traversal.map_indices.to_vec()))
    }

    fn future_children<'a>(
        &self,
        node: &'a GraphNode,
        context: &Context,
        traversal: Traversal<'_>,
    ) -> Vec<(&'a GraphNode, Option<Guard>)> {
        if let GraphNode::Par(group) = node {
            return group
                .branches
                .as_slice()
                .iter()
                .enumerate()
                .map(|(index, branch)| {
                    (
                        branch,
                        openengine_cluster_server::graph_verifier::retained_branch_guard(
                            group, index,
                        ),
                    )
                })
                .collect();
        }
        let GraphNode::Choice(group) = node else {
            return openengine_cluster_server::graph_verifier::graph_node_children(node)
                .into_iter()
                .map(|node| (node, None))
                .collect();
        };
        let mut children = Vec::new();
        let mut residual = None;
        for branch in group.branches.as_slice() {
            let route = together(residual.clone(), Some(branch.when.clone()));
            match self.candidate_guard(&branch.when, context, traversal.map_indices) {
                openengine_cluster_protocol::GuardTruth::False => {}
                openengine_cluster_protocol::GuardTruth::True => {
                    children.push((&branch.node, route));
                    return children;
                }
                openengine_cluster_protocol::GuardTruth::Unknown => {
                    children.push((&branch.node, route))
                }
            }
            residual = together(
                residual,
                Some(Guard::Not {
                    guard: Box::new(branch.when.clone()),
                }),
            );
        }
        if let Some(otherwise) = &group.otherwise {
            children.push((otherwise, residual));
        }
        children
    }
}

fn together(left: Option<Guard>, right: Option<Guard>) -> Option<Guard> {
    match (left, right) {
        (Some(a), Some(b)) if a != b => openengine_cluster_protocol::NonEmptyVec::new(vec![a, b])
            .ok()
            .map(|guards| Guard::All { guards }),
        (Some(a), _) | (_, Some(a)) => Some(a),
        _ => None,
    }
}

fn promoted_paths(node: &GraphNode) -> Option<&[FieldPath]> {
    Some(match node {
        GraphNode::Seq(n) => &n.promoted_state_paths,
        GraphNode::Choice(n) => &n.promoted_state_paths,
        GraphNode::Par(n) => &n.promoted_state_paths,
        GraphNode::Loop(n) => &n.promoted_state_paths,
        GraphNode::Map(n) => &n.promoted_state_paths,
        _ => return None,
    })
}
fn leaf_write_paths(node: &GraphNode) -> FutureWrites {
    let bindings = match node {
        GraphNode::Step(n) => &n.write_bindings,
        GraphNode::Verifier(n) => &n.write_bindings,
        _ => return FutureWrites::default(),
    };
    FutureWrites {
        changes: bindings
            .iter()
            .map(|binding| {
                (
                    binding.target.clone(),
                    CollectionWrite {
                        condition: None,
                        lengths: None,
                    },
                )
            })
            .collect(),
        known: BTreeMap::new(),
    }
}
fn paths_overlap(left: &FieldPath, right: &FieldPath) -> bool {
    left.segments().starts_with(right.segments()) || right.segments().starts_with(left.segments())
}

fn collection_length(selector: &DataSelector, state: &Value, item: Option<&Value>) -> Option<u64> {
    select_data(selector, state, item)
        .ok()?
        .as_array()
        .map(|items| items.len() as u64)
}
