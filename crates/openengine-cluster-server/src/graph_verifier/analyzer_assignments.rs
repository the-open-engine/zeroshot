use super::*;

struct AssignmentInputs {
    selectors: Vec<ControlSelector>,
    map_aggregates: BTreeMap<SelectorKey, MapAggregate>,
    join_correlations: BTreeSet<NodeName>,
    count_limits: CountLimits,
    label_classes: BTreeMap<SelectorKey, BTreeMap<EnumLabel, EnumLabel>>,
}

/// The legal joint outcomes of every control one check reads, as a condition to ask about.
pub(super) struct ControlProof {
    pub(super) diagram: Diagram,
    pub(super) controls: Controls,
    /// Holds exactly for the legal outcomes.
    pub(super) legal: Ref,
    /// Nodes whose success or failure the check can observe.
    outcomes: BTreeSet<NodeName>,
}

impl ControlProof {
    pub(super) fn guard(&mut self, guard: &Guard) -> Option<Ref> {
        self.diagram.guard(&self.controls, guard)
    }

    pub(super) fn predicate(&mut self, predicate: &CompletionPredicate) -> Option<Ref> {
        self.diagram.predicate(&self.controls, predicate)
    }

    /// Whether some legal outcome satisfies `condition`.
    pub(super) fn possible(&mut self, condition: Ref) -> Option<bool> {
        Some(self.diagram.and(self.legal, condition)? != NEVER)
    }

    /// Routes legal outcomes through a choice: each branch takes what its guard accepts of
    /// the outcomes no earlier branch took, and the last entry is what `otherwise` is left
    /// with. A route no legal outcome takes is `None`.
    pub(super) fn routes(
        &mut self,
        choice: &ChoiceNode,
        conditions: &[(NodeName, CompletionPredicate)],
    ) -> Option<Vec<Option<OutcomeRefinement>>> {
        let writes = conditions
            .iter()
            .map(|(name, condition)| Some((name, self.predicate(condition)?)))
            .collect::<Option<Vec<_>>>()?;
        let mut uncovered = self.legal;
        let mut routes = Vec::with_capacity(choice.branches.as_slice().len() + 1);
        for branch in choice.branches.as_slice() {
            let when = self.guard(&branch.when)?;
            let skipped = self.diagram.not(when)?;
            routes.push(self.diagram.and(uncovered, when)?);
            uncovered = self.diagram.and(uncovered, skipped)?;
        }
        routes.push(uncovered);
        routes
            .into_iter()
            .map(|residual| self.refinement(residual, &writes))
            .collect()
    }

    /// What every outcome in `residual` implies about observed nodes: a node counts as failed
    /// when any such outcome fails it, and a conditional write as made when all of them make
    /// it. `None` inside means no legal outcome is in `residual`.
    fn refinement(
        &mut self,
        residual: Ref,
        writes: &[(&NodeName, Ref)],
    ) -> Option<Option<OutcomeRefinement>> {
        if residual == NEVER {
            return Some(None);
        }
        let mut refinement = OutcomeRefinement::default();
        for name in &self.outcomes {
            let failure = self.diagram.failure(&self.controls, name)?;
            if self.diagram.and(residual, failure)? == NEVER {
                refinement.success.insert(name.clone());
            } else {
                refinement.failed.insert(name.clone());
            }
        }
        for (name, written) in writes {
            let unwritten = self.diagram.not(*written)?;
            if self.diagram.and(residual, unwritten)? == NEVER {
                refinement.failed.remove(*name);
                refinement.success.insert((*name).clone());
            }
        }
        Some(Some(refinement))
    }
}

impl<'a> Analyzer<'a> {
    pub(super) fn guard_has_satisfying_assignment(
        &mut self,
        guard: &Guard,
        path: &[DiagnosticPathSegment],
    ) -> bool {
        self.decide(&[guard], path, |proof| {
            let guard = proof.guard(guard)?;
            proof.possible(guard)
        })
        .unwrap_or(false)
    }

    pub(super) fn restrict_completion(
        &mut self,
        effects: &mut Effects,
        condition: CompletionPredicate,
        path: &[DiagnosticPathSegment],
    ) {
        effects.completion = CompletionPredicate::all([condition, effects.completion.clone()]);
        effects.falls_through = effects.falls_through
            && self
                .completion_is_satisfiable(&effects.completion, path)
                .unwrap_or(true);
    }

    pub(super) fn completion_is_satisfiable(
        &mut self,
        predicate: &CompletionPredicate,
        path: &[DiagnosticPathSegment],
    ) -> Option<bool> {
        let mut guards = Vec::new();
        predicate.collect_guards(&mut guards);
        self.decide(&guards, path, |proof| {
            let predicate = proof.predicate(predicate)?;
            proof.possible(predicate)
        })
    }

    /// Answers one question about the controls `guards` read.
    pub(super) fn decide<T>(
        &mut self,
        guards: &[&Guard],
        path: &[DiagnosticPathSegment],
        question: impl FnOnce(&mut ControlProof) -> Option<T>,
    ) -> Option<T> {
        let mut proof = self.control_proof(guards, path)?;
        let answer = question(&mut proof);
        self.search_steps += proof.diagram.steps();
        if answer.is_none() {
            self.assignment_ceiling(path);
        }
        answer
    }

    /// The legal outcomes of the controls `guards` read. A search that cannot finish within
    /// its backstop is a ceiling diagnostic at `path`, the condition being decided.
    pub(super) fn control_proof(
        &mut self,
        guards: &[&Guard],
        path: &[DiagnosticPathSegment],
    ) -> Option<ControlProof> {
        let proof = self.search_controls(guards, path);
        if proof.is_none() {
            self.assignment_ceiling(path);
        }
        proof
    }

    fn search_controls(
        &self,
        guards: &[&Guard],
        path: &[DiagnosticPathSegment],
    ) -> Option<ControlProof> {
        let AssignmentInputs {
            selectors,
            map_aggregates,
            join_correlations,
            count_limits,
            label_classes,
        } = self.assignment_inputs(guards, path)?;
        let inputs = DimensionInputs {
            count_limits: &count_limits,
            label_classes: &label_classes,
            nodes: &self.nodes,
            map_aggregates: &map_aggregates,
            join_correlations: &self.parallel_join_correlations,
            execution_correlations: &self.map_execution_correlations,
        };
        let plan = scope_plan(&selectors, inputs);
        let mut search = Search {
            inputs,
            diagram: Diagram::default(),
        };
        let mut scope = search.scope(&plan, &Controls::new())?;
        for name in &join_correlations {
            let aggregated = map_aggregates
                .keys()
                .any(|key| key.name == *name && key.source == ControlSourceKey::Group);
            if let Some(correlation) = self
                .parallel_join_correlations
                .get(name)
                .filter(|_| !aggregated)
            {
                let joined = search.join(name, correlation, &scope.controls)?;
                scope.legal = search.diagram.and(scope.legal, joined)?;
            }
        }
        Some(ControlProof {
            outcomes: plan
                .keys
                .iter()
                .filter(|key| observes_outcome(key))
                .map(|key| key.name.clone())
                .collect(),
            diagram: search.diagram,
            controls: scope.controls,
            legal: scope.legal,
        })
    }

    fn assignment_inputs(
        &self,
        guards: &[&Guard],
        path: &[DiagnosticPathSegment],
    ) -> Option<AssignmentInputs> {
        let local_maps = path
            .iter()
            .filter_map(|part| match part {
                DiagnosticPathSegment::Node { name }
                    if matches!(self.nodes.get(name)?.node, GraphNode::Map(_)) =>
                {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let mut expanded = guards.to_vec();
        let mut aggregates = self.explicit_map_aggregates(&expanded);
        let mut join_correlations = BTreeSet::new();
        let mut execution_correlations = BTreeSet::new();
        let mut loop_correlations = BTreeSet::new();
        loop {
            aggregates.retain(|_, aggregate| !local_maps.contains(&aggregate.owner));
            let mut discovered = false;
            let selectors = expanded
                .iter()
                .flat_map(|guard| guard_selectors(guard))
                .collect::<Vec<_>>();
            for selector in selectors {
                discovered |= self.expand_loop_dependencies(
                    selector,
                    &mut aggregates,
                    (&mut loop_correlations, &mut expanded),
                );
                let Some(correlation) = self.parallel_join_correlations.get(&selector.name) else {
                    continue;
                };
                if !is_parallel_join_selector(selector, correlation.field())
                    || !join_correlations.insert(selector.name.clone())
                {
                    continue;
                }
                let mut correlation_guards = Vec::new();
                correlation.collect_guards(&mut correlation_guards);
                if aggregates.contains_key(&SelectorKey::from(selector)) {
                    self.extend_aggregate_dependencies(&correlation_guards, &mut aggregates);
                }
                expanded.extend(correlation_guards);
                aggregates.extend(self.explicit_map_aggregates(&expanded));
                discovered = true;
            }
            let aggregate_controls = aggregates.clone();
            for (key, aggregate) in aggregate_controls {
                discovered |= self.expand_ancestor_map_dependencies(
                    &key,
                    &mut aggregates,
                    (&mut execution_correlations, &mut expanded),
                );
                let Some(correlation) = self.map_execution_correlations.get(&key.name) else {
                    continue;
                };
                if correlation.owner != aggregate.owner
                    || !execution_correlations.insert(key.name.clone())
                {
                    continue;
                }
                let mut correlation_guards = Vec::new();
                correlation.collect_guards(&mut correlation_guards);
                self.extend_aggregate_dependencies(&correlation_guards, &mut aggregates);
                expanded.extend(correlation_guards);
                aggregates.extend(self.explicit_map_aggregates(&expanded));
                discovered = true;
            }
            if !discovered {
                break;
            }
        }
        aggregates.retain(|_, aggregate| !local_maps.contains(&aggregate.owner));
        // Shared route predicates contribute the same guard once per reader.
        let mut seen = BTreeSet::new();
        expanded.retain(|guard| seen.insert(std::ptr::from_ref(*guard)));
        self.assignment_controls(&expanded, aggregates, join_correlations)
    }

    fn assignment_controls(
        &self,
        guards: &[&Guard],
        aggregates: BTreeMap<SelectorKey, MapAggregate>,
        join_correlations: BTreeSet<NodeName>,
    ) -> Option<AssignmentInputs> {
        let selectors = guards
            .iter()
            .flat_map(|guard| guard_selectors(guard))
            .cloned()
            .collect::<Vec<_>>();

        let label_classes = self.aggregate_label_classes(&selectors, &aggregates, guards)?;
        let count_limits = aggregate_count_limits(guards, &label_classes);
        Some(AssignmentInputs {
            selectors,
            map_aggregates: aggregates,
            join_correlations,
            count_limits,
            label_classes,
        })
    }

    fn expand_loop_dependencies<'g>(
        &'g self,
        selector: &ControlSelector,
        aggregates: &mut BTreeMap<SelectorKey, MapAggregate>,
        (registered, expanded): (&mut BTreeSet<NodeName>, &mut Vec<&'g Guard>),
    ) -> bool {
        let Some(GraphNode::Loop(group)) = self.nodes.get(&selector.name).map(|info| info.node)
        else {
            return false;
        };
        if selector.source != ControlSource::Group
            || selector
                .field
                .as_ref()
                .is_none_or(|field| field.as_str() != "terminated")
            || !registered.insert(selector.name.clone())
        {
            return false;
        }
        if let Some(until) = &group.until {
            if aggregates.contains_key(&SelectorKey::from(selector)) {
                self.extend_aggregate_dependencies(&[until], aggregates);
            }
            aggregates.extend(self.explicit_map_aggregates(&[until]));
            expanded.push(until);
        }
        true
    }

    fn extend_aggregate_dependencies(
        &self,
        guards: &[&Guard],
        aggregates: &mut BTreeMap<SelectorKey, MapAggregate>,
    ) {
        for dependency in guards.iter().flat_map(|guard| guard_selectors(guard)) {
            if let Some(scope) = self.map_control_scope(dependency) {
                aggregates.insert(SelectorKey::from(dependency), scope);
            }
        }
    }

    fn expand_ancestor_map_dependencies<'g>(
        &'g self,
        key: &SelectorKey,
        aggregates: &mut BTreeMap<SelectorKey, MapAggregate>,
        expansion: (&mut BTreeSet<NodeName>, &mut Vec<&'g Guard>),
    ) -> bool {
        let (registered, expanded) = expansion;
        let Some(info) = self.nodes.get(&key.name) else {
            return false;
        };
        let mut discovered = false;
        for ancestor in &info.path {
            let DiagnosticPathSegment::Node { name } = ancestor else {
                continue;
            };
            if name == &key.name
                || !matches!(
                    self.nodes.get(name).map(|info| info.node),
                    Some(GraphNode::Map(_))
                )
            {
                continue;
            }
            let Some(correlation) = self
                .map_execution_correlations
                .get(name)
                .filter(|_| registered.insert(name.clone()))
            else {
                continue;
            };
            let mut guards = Vec::new();
            correlation.collect_guards(&mut guards);
            self.extend_aggregate_dependencies(&guards, aggregates);
            expanded.extend(guards);
            discovered = true;
        }
        discovered
    }

    fn aggregate_label_classes(
        &self,
        selectors: &[ControlSelector],
        aggregates: &BTreeMap<SelectorKey, MapAggregate>,
        expanded: &[&Guard],
    ) -> Option<BTreeMap<SelectorKey, BTreeMap<EnumLabel, EnumLabel>>> {
        let mut queries = BTreeMap::new();
        for guard in expanded {
            collect_label_queries(guard, &mut queries);
        }
        let mut label_classes = BTreeMap::new();
        for selector in selectors {
            let key = SelectorKey::from(selector);
            if label_classes.contains_key(&key) {
                continue;
            }
            let (Some(domain), Some(tests)) =
                (self.proof_selector_domain(selector), queries.get(&key))
            else {
                if aggregates.contains_key(&key) {
                    return None;
                }
                continue;
            };
            let mut representatives = BTreeMap::<Vec<bool>, EnumLabel>::new();
            let classes = domain
                .into_iter()
                .map(|label| {
                    let mut signature = tests
                        .iter()
                        .map(|labels| labels.contains(&&label))
                        .collect::<Vec<_>>();
                    // Refinement and joined/raced correlations also observe failure.
                    signature.push(control_label_is_failure(&key, &label));
                    let representative = representatives
                        .entry(signature)
                        .or_insert_with(|| label.clone())
                        .clone();
                    (label, representative)
                })
                .collect();
            label_classes.insert(key, classes);
        }
        Some(label_classes)
    }

    fn proof_selector_domain(&self, selector: &ControlSelector) -> Option<Vec<EnumLabel>> {
        if selector.source == ControlSource::Group {
            proof_group_domain(
                self.nodes.get(&selector.name)?.node,
                selector.field.as_ref(),
            )
        } else {
            self.selector_domain(selector)
        }
    }

    fn explicit_map_aggregates(&self, guards: &[&Guard]) -> BTreeMap<SelectorKey, MapAggregate> {
        guards
            .iter()
            .copied()
            .flat_map(|guard| guard_selector_uses(guard))
            .filter_map(|(selector, aggregate)| {
                aggregate
                    .then(|| self.map_control_scope(selector))
                    .flatten()
                    .map(|aggregate| (SelectorKey::from(selector), aggregate))
            })
            .collect()
    }
}

fn aggregate_count_limits(
    guards: &[&Guard],
    classes: &BTreeMap<SelectorKey, BTreeMap<EnumLabel, EnumLabel>>,
) -> CountLimits {
    fn collect(
        guard: &Guard,
        classes: &BTreeMap<SelectorKey, BTreeMap<EnumLabel, EnumLabel>>,
        limits: &mut CountLimits,
    ) {
        let mut add = |selector: &ControlSelector, labels: &[EnumLabel], count| {
            let key = SelectorKey::from(selector);
            if let Some(mapping) = classes.get(&key) {
                for label in labels {
                    if let Some(class) = mapping.get(label) {
                        let limit = limits
                            .entry(key.clone())
                            .or_default()
                            .entry(class.clone())
                            .or_default();
                        *limit = (*limit).max(count);
                    }
                }
            }
        };
        match guard {
            Guard::In { value, labels } => add(value, labels.values(), 1),
            Guard::KOfMap {
                value,
                labels,
                count,
            } => add(value, labels.values(), count.get()),
            Guard::KOfN { values, labels, .. } => {
                for value in values.as_slice() {
                    add(value, labels.values(), 1);
                }
            }
            Guard::All { guards } | Guard::Any { guards } => {
                for guard in guards.as_slice() {
                    collect(guard, classes, limits);
                }
            }
            Guard::Not { guard } => collect(guard, classes, limits),
        }
    }
    let mut limits = CountLimits::new();
    for guard in guards {
        collect(guard, classes, &mut limits);
    }
    limits
}

fn is_parallel_join_selector(selector: &ControlSelector, field: &str) -> bool {
    selector.source == ControlSource::Group
        && selector.field.as_ref().is_some_and(|selector_field| {
            selector_field.as_str() == field || retention_index(Some(selector_field)).is_some()
        })
}

/// The label sets each selector is tested against; labels no test separates are one class.
fn collect_label_queries<'g>(
    guard: &'g Guard,
    queries: &mut BTreeMap<SelectorKey, Vec<BTreeSet<&'g EnumLabel>>>,
) {
    let mut add = |values: &'g [ControlSelector], labels: &'g [EnumLabel]| {
        for value in values {
            queries
                .entry(SelectorKey::from(value))
                .or_default()
                .push(labels.iter().collect());
        }
    };
    match guard {
        Guard::In { value, labels } | Guard::KOfMap { value, labels, .. } => {
            add(std::slice::from_ref(value), labels.values());
        }
        Guard::KOfN { values, labels, .. } => add(values.as_slice(), labels.values()),
        Guard::All { guards } | Guard::Any { guards } => {
            for guard in guards.as_slice() {
                collect_label_queries(guard, queries);
            }
        }
        Guard::Not { guard } => collect_label_queries(guard, queries),
    }
}
