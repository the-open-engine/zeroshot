//! Residual viability conditions the same correlated assignments used at admission.
use super::*;

/// Read-only runtime facts; the reducer retains ownership of occurrences and replay state.
pub trait ControlSnapshot {
    /// One runtime projection may check several coherent exits under a single proof-work owner.
    fn proof_work(&self) -> ProofWork {
        ProofWork::default()
    }
    fn is_fixed(&self, node: &NodeName, scope: &[u64]) -> bool;
    fn fixed_count(
        &self,
        query: (&ControlSelector, &[EnumLabel], bool),
        scope: &[u64],
    ) -> Option<u64>;
    fn retained(&self, node: &NodeName, scope: &[u64], index: usize) -> Option<bool>;
    fn label(&self, selector: &ControlSelector, scope: &[u64]) -> Option<EnumLabel>;
    fn map_progress(&self, map: &MapNode, scope: &[u64]) -> MapProgress;
    fn selection_guards(&self) -> Box<dyn Iterator<Item = &Guard> + '_>;
    /// Required predicates for one occurrence, checked before its outcomes enter a parent map.
    fn scoped_guards<'a>(&'a self, _scope: &'a [u64]) -> Box<dyn Iterator<Item = &'a Guard> + 'a> {
        Box::new(std::iter::empty())
    }
}

/// Compact facts for one map occurrence; unknown future cardinality stays explicit.
pub struct MapProgress {
    /// Exact current/final length when remaining visible writes cannot change the collection.
    pub cardinality: MapCardinality,
    /// Item indices with runtime facts; all other items share the unstarted outcome domain.
    pub known_items: Vec<u64>,
}

/// Compact selector facts, with changes restricted to the authored continuations that write it.
#[derive(Clone, Default, PartialEq)]
pub struct MapCardinality {
    pub length: Option<u64>,
    pub writes: Vec<CollectionWrite>,
}

/// A prospective collection replacement, conditioned on the same joint control assignment.
#[derive(Clone, PartialEq)]
pub struct CollectionWrite {
    /// No guard means this replacement is unconditional in the selected continuation.
    pub condition: Option<Guard>,
    /// Exact possible lengths; None is reserved for genuinely unknown output cardinality.
    pub lengths: Option<BTreeSet<u64>>,
}

impl CollectionWrite {
    /// Conservatively collect sizes without binding the controls of a different loop round.
    pub fn possible_lengths(baseline: Option<u64>, writes: &[Self]) -> Option<BTreeSet<u64>> {
        let mut lengths = baseline.map(|length| BTreeSet::from([length]));
        for write in writes {
            if write.condition.is_none() {
                lengths = write.lengths.clone();
            } else {
                lengths.as_mut()?.extend(write.lengths.as_ref()?);
            }
        }
        lengths
    }
}

impl MapCardinality {
    pub fn fixed(length: u64) -> Self {
        Self {
            length: Some(length),
            writes: vec![],
        }
    }
}

#[derive(Clone, Copy)]
pub(in crate::graph_verifier) struct RuntimeScope<'a> {
    pub snapshot: &'a dyn ControlSnapshot,
    pub indices: &'a [u64],
}

/// Fixed facts stay fixed. Missing foreign facts remain unknown. Bounded analysis conservatively
/// retains a candidate if the shared proof-work limit is reached.
pub fn guard_has_possible_future(
    branch: &GraphNode,
    guard: &Guard,
    snapshot: &dyn ControlSnapshot,
    indices: &[u64],
) -> bool {
    possible_future(
        branch,
        Some(guard),
        RuntimeScope { snapshot, indices },
        FutureRequirement::Candidate,
    )
}

/// Whether one normally completing round can both select a carried write and leave the exit
/// guard unproved. A missing exit guard always permits reentry; a missing write is unconditional.
/// Unavailable foreign controls and bounded-analysis refusal remain conservative possibilities.
pub fn loop_reentry_possible(
    body: &GraphNode,
    conditions: (Option<&Guard>, Option<&Guard>),
    snapshot: &dyn ControlSnapshot,
    indices: &[u64],
) -> bool {
    possible_future(
        body,
        conditions.0,
        RuntimeScope { snapshot, indices },
        FutureRequirement::NonConvergence(conditions.1),
    )
}

#[derive(Clone, Copy)]
enum FutureRequirement<'a> {
    Candidate,
    NonConvergence(Option<&'a Guard>),
}

fn possible_future(
    branch: &GraphNode,
    guard: Option<&Guard>,
    runtime: RuntimeScope<'_>,
    requirement: FutureRequirement<'_>,
) -> bool {
    let RuntimeScope { snapshot, indices } = runtime;
    let mut analyzer = Analyzer::from_view(GraphView {
        profile: GraphProfile::Full,
        initial_input: branch.state().unwrap_or(&PayloadType::Null),
        root: branch,
    });
    analyzer.proof_work = snapshot.proof_work();
    analyzer.index_node(branch, vec![node_segment(branch.name())], 1);
    let completion = analyzer.collect_control_flow(branch);
    let mut guards = guard.into_iter().collect::<Vec<_>>();
    if let FutureRequirement::NonConvergence(Some(write)) = requirement {
        guards.push(write);
    }
    let available = future_availability(&analyzer, &guards, runtime);
    completion.collect_guards(&mut guards);
    for presence in available.values() {
        presence.collect_guards(&mut guards);
    }
    let Some(selection_guards) = snapshot
        .selection_guards()
        .map(|guard| {
            analyzer
                .proof_work
                .charge(1 + guard_node_count(guard) as usize)?;
            Some(guard)
        })
        .collect::<Option<Vec<_>>>()
    else {
        return true;
    };
    guards.extend(selection_guards);
    analyzer
        .assignments_with_runtime(&guards, &[], Some(RuntimeScope { snapshot, indices }))
        .is_none_or(|assignments| {
            assignments.iter().any(|assignment| {
                let evaluate = |guard: &Guard| {
                    openengine_cluster_protocol::evaluate_partial_guard(
                        guard,
                        &|selector, labels, aggregate| {
                            snapshot
                                .fixed_count((selector, labels, aggregate), indices)
                                .or_else(|| {
                                    let key = SelectorKey::from(selector);
                                    let present = available
                                        .get(&key)
                                        .is_some_and(|presence| presence.evaluate(assignment));
                                    if !present
                                        && matches!(
                                            requirement,
                                            FutureRequirement::NonConvergence(_)
                                        )
                                        && analyzer.nodes.contains_key(&selector.name)
                                    {
                                        return Some(0);
                                    }
                                    present
                                        .then(|| assignment.get(&key))
                                        .flatten()
                                        .map(|control| {
                                            control
                                                .counts
                                                .iter()
                                                .filter(|(label, _)| labels.contains(label))
                                                .map(|(_, count)| *count)
                                                .sum()
                                        })
                                })
                        },
                    )
                };
                completion.evaluate(assignment)
                    && requirement.accepts(
                        guard
                            .map(evaluate)
                            .unwrap_or(openengine_cluster_protocol::GuardTruth::False),
                        &evaluate,
                    )
            })
        })
}

fn future_availability(
    analyzer: &Analyzer<'_>,
    guards: &[&Guard],
    runtime: RuntimeScope<'_>,
) -> BTreeMap<SelectorKey, CompletionPredicate> {
    let incoming = guards
        .iter()
        .flat_map(|guard| guard_selectors(guard))
        .filter(|selector| runtime.snapshot.is_fixed(&selector.name, runtime.indices))
        .map(|selector| selector.name.clone())
        .collect();
    let mut available = BTreeMap::new();
    for guard in guards {
        available.extend(analyzer.first_guard_available_on_branch(
            guard,
            analyzer.graph.root,
            &incoming,
        ));
        for (selector, aggregate) in guard_selector_uses(guard) {
            if runtime
                .snapshot
                .fixed_count((selector, &[], aggregate), runtime.indices)
                .is_some()
            {
                available.insert(SelectorKey::from(selector), CompletionPredicate::Always);
            }
        }
    }
    available
}

impl FutureRequirement<'_> {
    fn accepts(
        self,
        truth: openengine_cluster_protocol::GuardTruth,
        evaluate: &impl Fn(&Guard) -> openengine_cluster_protocol::GuardTruth,
    ) -> bool {
        use openengine_cluster_protocol::GuardTruth::{True, False};
        match self {
            Self::Candidate => truth == True,
            Self::NonConvergence(write) => {
                truth != True && write.is_none_or(|guard| evaluate(guard) != False)
            }
        }
    }
}

pub(super) fn fixed_outcome(
    name: &NodeName,
    keys: &[SelectorKey],
    runtime: RuntimeScope<'_>,
) -> Option<Assignment> {
    if !runtime.snapshot.is_fixed(name, runtime.indices) {
        return None;
    }
    keys.iter()
        .map(|key| {
            let label = if let Some(index) = (key.source == ControlSourceKey::Group)
                .then(|| retention_index(key.field.as_ref()))
                .flatten()
            {
                let retained = runtime.snapshot.retained(name, runtime.indices, index)?;
                enum_label(if retained { "retained" } else { "omitted" })
            } else {
                match key.source {
                    ControlSourceKey::Execution => enum_label("executed"),
                    source => runtime.snapshot.label(
                        &ControlSelector {
                            name: key.name.clone(),
                            source: match source {
                                ControlSourceKey::Signal => ControlSource::Signal,
                                ControlSourceKey::Error => ControlSource::Error,
                                _ => ControlSource::Group,
                            },
                            field: key.field.clone(),
                        },
                        runtime.indices,
                    ),
                }
            };
            Some((key.clone(), assigned_control(label, 1)))
        })
        .collect()
}

mod writes;

impl Analyzer<'_> {
    // Populate only structural correlations, without repeating admission or binding verification.
    fn collect_control_flow(&mut self, node: &GraphNode) -> CompletionPredicate {
        let children = child_nodes(node);
        let completions = children
            .iter()
            .map(|child| self.collect_control_flow(child))
            .collect::<Vec<_>>();
        let completion = match node {
            GraphNode::Succeed(_) | GraphNode::Fail(_) => CompletionPredicate::Never,
            GraphNode::Seq(_) => CompletionPredicate::all(completions),
            GraphNode::Choice(group) => {
                let routes = choice_branch_completion_predicates(group)
                    .into_iter()
                    .chain(std::iter::once(choice_otherwise_completion_predicate(
                        group,
                    )));
                CompletionPredicate::any(
                    routes
                        .zip(completions)
                        .map(|(route, completion)| CompletionPredicate::all([route, completion])),
                )
            }
            GraphNode::Loop(_) => completions.into_iter().next().unwrap_or_default(),
            GraphNode::Map(group) => {
                self.collect_map_execution_correlations(group);
                map_completion_predicate(group).unwrap_or(CompletionPredicate::Always)
            }
            GraphNode::Par(group) => {
                let owned = collect_descendant_names(&group.branches);
                let incoming = match &group.join {
                    Join::First { when } => guard_selectors(when)
                        .into_iter()
                        .filter(|selector| !owned.contains(&selector.name))
                        .map(|selector| selector.name.clone())
                        .collect(),
                    _ => BTreeSet::new(),
                };
                let correlation = self.parallel_correlation(group, &completions, &incoming);
                self.parallel_join_correlations
                    .insert(group.name.clone(), correlation);
                CompletionPredicate::Always
            }
            _ => CompletionPredicate::Always,
        };
        self.node_fallthrough.insert(
            node_identity(node),
            !matches!(completion, CompletionPredicate::Never),
        );
        self.node_completion
            .insert(node.name().clone(), completion.clone());
        completion
    }
}

#[cfg(test)]
mod tests;
