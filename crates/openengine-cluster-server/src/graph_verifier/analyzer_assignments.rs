use super::*;

struct AssignmentInputs {
    selectors: Vec<ControlSelector>,
    map_aggregates: BTreeMap<SelectorKey, MapAggregate>,
    join_correlations: BTreeSet<NodeName>,
    saturation: u64,
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
            saturation,
            label_classes,
        }) = self.assignment_inputs(guards)
        else {
            self.assignment_ceiling(path);
            return None;
        };
        if self.proof_work.charge(1 + selectors.len()).is_none() {
            self.assignment_ceiling(path);
            return None;
        }
        let Some(dimensions) = build_dimensions(
            &selectors,
            DimensionInputs {
                saturation,
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
        let Some(mut assignments) = enumerate_assignments(dimensions, &self.proof_work) else {
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

    fn assignment_inputs(&self, guards: &[&Guard]) -> Option<AssignmentInputs> {
        let mut expanded = guards.to_vec();
        let mut aggregates = self.explicit_map_aggregates(&expanded);
        let mut join_correlations = BTreeSet::new();
        let mut execution_correlations = BTreeSet::new();
        loop {
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
                    for dependency in correlation_guards
                        .iter()
                        .flat_map(|guard| guard_selectors(guard))
                    {
                        if self
                            .map_control_scope(dependency)
                            .is_some_and(|scope| scope.owner == aggregate.owner)
                        {
                            aggregates.insert(SelectorKey::from(dependency), aggregate.clone());
                        }
                    }
                }
                expanded.extend(correlation_guards);
                aggregates.extend(self.explicit_map_aggregates(&expanded));
                discovered = true;
            }
            let aggregate_controls = aggregates.clone();
            for (key, aggregate) in aggregate_controls {
                let Some(correlation) = self.map_execution_correlations.get(&key.name) else {
                    continue;
                };
                if correlation.owner != aggregate.owner
                    || !execution_correlations.insert(key.name.clone())
                {
                    continue;
                }
                let mut correlation_guards = Vec::new();
                correlation.presence.collect_guards(&mut correlation_guards);
                for dependency in correlation_guards
                    .iter()
                    .flat_map(|guard| guard_selectors(guard))
                {
                    let dependency_key = SelectorKey::from(dependency);
                    if self
                        .map_control_scope(dependency)
                        .is_some_and(|scope| scope.owner == aggregate.owner)
                    {
                        aggregates.insert(dependency_key, aggregate.clone());
                    }
                }
                expanded.extend(correlation_guards);
                discovered = true;
            }
            if !discovered {
                break;
            }
        }
        let selectors = expanded
            .iter()
            .flat_map(|guard| guard_selectors(guard))
            .cloned()
            .collect::<Vec<_>>();

        let saturation = expanded
            .iter()
            .map(|guard| aggregate_threshold(guard))
            .max()
            .unwrap_or(1);
        let label_classes = self.aggregate_label_classes(&selectors, &aggregates, &expanded)?;
        Some(AssignmentInputs {
            selectors,
            map_aggregates: aggregates,
            join_correlations,
            saturation,
            label_classes,
        })
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

fn aggregate_threshold(guard: &Guard) -> u64 {
    match guard {
        Guard::KOfMap { count, .. } => count.get(),
        Guard::All { guards } | Guard::Any { guards } => guards
            .as_slice()
            .iter()
            .map(aggregate_threshold)
            .max()
            .unwrap_or(1),
        Guard::Not { guard } => aggregate_threshold(guard),
        Guard::In { .. } | Guard::KOfN { .. } => 1,
    }
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
