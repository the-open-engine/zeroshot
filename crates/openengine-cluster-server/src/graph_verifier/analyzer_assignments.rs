use super::*;

struct AssignmentInputs {
    selectors: Vec<ControlSelector>,
    map_aggregates: BTreeMap<SelectorKey, MapAggregate>,
    join_correlations: BTreeSet<NodeName>,
    count_limits: CountLimits,
    label_classes: BTreeMap<SelectorKey, BTreeMap<EnumLabel, EnumLabel>>,
}

impl<'a> Analyzer<'a> {
    pub(super) fn guard_has_satisfying_assignment(
        &mut self,
        guard: &Guard,
        path: &[DiagnosticPathSegment],
    ) -> bool {
        self.assignments_for_guards(&[guard], path)
            .is_some_and(|assignments| {
                assignments
                    .iter()
                    .any(|assignment| evaluate_guard(guard, assignment))
            })
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
        self.assignments_for_completion_predicates(&[predicate], path)
            .map(|assignments| {
                assignments
                    .iter()
                    .any(|assignment| predicate.evaluate(assignment))
            })
    }

    pub(super) fn assignments_for_completion_predicates(
        &mut self,
        predicates: &[&CompletionPredicate],
        path: &[DiagnosticPathSegment],
    ) -> Option<Vec<Assignment>> {
        let mut guards = Vec::new();
        for predicate in predicates {
            predicate.collect_guards(&mut guards);
        }
        self.assignments_for_guards(&guards, path)
    }

    pub(super) fn assignments_for_guards(
        &mut self,
        guards: &[&Guard],
        path: &[DiagnosticPathSegment],
    ) -> Option<Vec<Assignment>> {
        let Some(AssignmentInputs {
            selectors,
            map_aggregates,
            join_correlations,
            count_limits,
            label_classes,
        }) = self.assignment_inputs(guards, path)
        else {
            self.assignment_ceiling(path);
            return None;
        };
        if self.proof_work.charge(1 + selectors.len()).is_none() {
            self.assignment_ceiling(path);
            return None;
        }
        let Some(mut assignments) = build_assignments(
            &selectors,
            DimensionInputs {
                count_limits: &count_limits,
                budget: &self.proof_work,
                label_classes: &label_classes,
                nodes: &self.nodes,
                map_aggregates: &map_aggregates,
                join_correlations: &self.parallel_join_correlations,
                execution_correlations: &self.map_execution_correlations,
            },
        ) else {
            self.assignment_ceiling(path);
            return None;
        };
        assignments.retain(|assignment| {
            join_correlations
                .iter()
                .filter(|name| {
                    !map_aggregates
                        .keys()
                        .any(|key| &key.name == *name && key.source == ControlSourceKey::Group)
                })
                .all(|name| self.assignment_matches_parallel_join_correlation(assignment, name))
        });
        Some(assignments)
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
            self.proof_work.charge(
                expanded
                    .iter()
                    .map(|guard| guard_node_count(guard) as usize)
                    .sum::<usize>()
                    .saturating_add(aggregates.len()),
            )?;
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
                if let Some(aggregate) = aggregates.get(&SelectorKey::from(selector)).cloned() {
                    self.extend_aggregate_dependencies(
                        &correlation_guards,
                        &aggregate,
                        &mut aggregates,
                    );
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
                self.extend_aggregate_dependencies(
                    &correlation_guards,
                    &aggregate,
                    &mut aggregates,
                );
                expanded.extend(correlation_guards);
                aggregates.extend(self.explicit_map_aggregates(&expanded));
                discovered = true;
            }
            if !discovered {
                break;
            }
        }
        aggregates.retain(|_, aggregate| !local_maps.contains(&aggregate.owner));
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
            if let Some(aggregate) = aggregates.get(&SelectorKey::from(selector)).cloned() {
                self.extend_aggregate_dependencies(&[until], &aggregate, aggregates);
            }
            aggregates.extend(self.explicit_map_aggregates(&[until]));
            expanded.push(until);
        }
        true
    }

    fn extend_aggregate_dependencies(
        &self,
        guards: &[&Guard],
        aggregate: &MapAggregate,
        aggregates: &mut BTreeMap<SelectorKey, MapAggregate>,
    ) {
        for dependency in guards.iter().flat_map(|guard| guard_selectors(guard)) {
            if self
                .map_control_scope(dependency)
                .is_some_and(|scope| scope.owner == aggregate.owner)
            {
                aggregates.insert(SelectorKey::from(dependency), aggregate.clone());
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
            for dependency in guards.iter().flat_map(|guard| guard_selectors(guard)) {
                if let Some(owner) = self.map_control_scope(dependency) {
                    aggregates.insert(SelectorKey::from(dependency), owner);
                }
            }
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
        let mut queries = BTreeMap::<SelectorKey, Vec<Vec<EnumLabel>>>::new();
        for guard in expanded {
            collect_label_queries(guard, &mut queries, &self.proof_work)?;
        }
        let mut label_classes = BTreeMap::new();
        for selector in selectors {
            let key = SelectorKey::from(selector);
            if !aggregates.contains_key(&key) || label_classes.contains_key(&key) {
                continue;
            }
            let domain = self.proof_selector_domain(selector)?;
            let tests = queries.get(&key)?;
            self.proof_work.charge(
                domain
                    .len()
                    .checked_mul(1 + tests.iter().map(Vec::len).sum::<usize>())?,
            )?;
            let mut representatives = BTreeMap::<Vec<bool>, EnumLabel>::new();
            let classes = domain
                .into_iter()
                .map(|label| {
                    let signature = tests.iter().map(|labels| labels.contains(&label)).collect();
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

    fn assignment_matches_parallel_join_correlation(
        &self,
        assignment: &Assignment,
        name: &NodeName,
    ) -> bool {
        self.parallel_join_correlations
            .get(name)
            .is_none_or(|correlation| {
                assignment_matches_parallel_join_correlation(assignment, name, correlation)
            })
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

fn collect_label_queries(
    guard: &Guard,
    queries: &mut BTreeMap<SelectorKey, Vec<Vec<EnumLabel>>>,
    budget: &ProofWork,
) -> Option<()> {
    match guard {
        Guard::In { value, labels } | Guard::KOfMap { value, labels, .. } => add_label_queries(
            std::slice::from_ref(value),
            labels.values(),
            queries,
            budget,
        )?,
        Guard::KOfN { values, labels, .. } => {
            add_label_queries(values.as_slice(), labels.values(), queries, budget)?
        }
        Guard::All { guards } | Guard::Any { guards } => {
            for guard in guards.as_slice() {
                collect_label_queries(guard, queries, budget)?;
            }
        }
        Guard::Not { guard } => collect_label_queries(guard, queries, budget)?,
    }
    Some(())
}

fn add_label_queries(
    values: &[ControlSelector],
    labels: &[EnumLabel],
    queries: &mut BTreeMap<SelectorKey, Vec<Vec<EnumLabel>>>,
    budget: &ProofWork,
) -> Option<()> {
    budget.charge(values.len().checked_mul(1 + labels.len())?)?;
    for value in values {
        queries
            .entry(SelectorKey::from(value))
            .or_default()
            .push(labels.to_vec());
    }
    Some(())
}
