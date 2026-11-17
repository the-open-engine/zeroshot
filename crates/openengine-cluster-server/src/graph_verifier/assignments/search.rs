//! Legal control outcomes as one condition per scope.
//!
//! Every selector a check reads becomes a small variable. Join, loop and execution rules are
//! conditions over those variables, so controls no rule connects never multiply. A map body is
//! solved once for each distinct condition its enclosing controls leave, and its items fold
//! into saturated counts.
use std::rc::Rc;

use super::*;

pub(in crate::graph_verifier) struct Search<'a, 'graph> {
    pub(in crate::graph_verifier) inputs: DimensionInputs<'a, 'graph>,
    pub(in crate::graph_verifier) diagram: Diagram,
}

pub(in crate::graph_verifier) struct Scope {
    /// Holds exactly for the legal joint outcomes of this scope's controls.
    pub(in crate::graph_verifier) legal: Ref,
    rules: Vec<Ref>,
    pub(in crate::graph_verifier) controls: Controls,
    first: u32,
    variables: Vec<(u32, Option<Gate>)>,
    masks: BTreeMap<NodeName, u32>,
}

impl Scope {
    fn slot(&self, variable: u32) -> usize {
        (variable - self.first) as usize
    }

    fn silent(&self, gate: Option<&Gate>, values: &[usize]) -> bool {
        gate.is_some_and(|(gate, emits)| !emits[values[self.slot(*gate)]])
    }

    /// What the item shows its map for one value of each of its variables. Controls of a
    /// node that did not emit, or that its join did not retain, are empty.
    fn outcome(&self, keys: &[SelectorKey], values: &[usize]) -> Assignment {
        keys.iter()
            .map(|key| {
                let reader = &self.controls[key];
                let dropped = self
                    .masks
                    .get(&key.name)
                    .is_some_and(|mask| values[self.slot(*mask)] == 0);
                let control = if dropped || self.silent(reader.gate.as_ref(), values) {
                    empty_control()
                } else {
                    reader.controls[values[self.slot(reader.variable)]].clone()
                };
                (key.clone(), control)
            })
            .collect()
    }
}

struct Status {
    emits: bool,
    executed: bool,
    errors: Vec<Option<EnumLabel>>,
}

fn labelled(label: Option<EnumLabel>) -> AssignedControl {
    let occurrences = u64::from(label.is_some());
    assigned_control(label, occurrences)
}

impl Search<'_, '_> {
    pub(in crate::graph_verifier) fn scope(
        &mut self,
        plan: &ScopePlan<'_>,
        inherited: &Controls,
    ) -> Option<Scope> {
        let mut scope = Scope {
            legal: ALWAYS,
            rules: Vec::new(),
            controls: inherited.clone(),
            first: self.diagram.variables(),
            variables: Vec::new(),
            masks: BTreeMap::new(),
        };
        let mut names = plan.local.iter().collect::<Vec<_>>();
        names.sort_by_key(|(name, _)| self.inputs.nodes.get(*name).map(|info| info.ordinal));
        for (name, keys) in names {
            self.node(name, keys, &mut scope)?;
        }
        for (map, body) in &plan.maps {
            self.map(map, body, &mut scope)?;
        }
        self.constrain(plan, inherited, &mut scope)?;
        self.mask(plan, &mut scope)?;
        scope.legal = self.diagram.all(std::mem::take(&mut scope.rules))?;
        Some(scope)
    }

    /// Labels the check cannot tell apart share one representative.
    fn class(&self, key: &SelectorKey, label: EnumLabel) -> EnumLabel {
        self.inputs
            .label_classes
            .get(key)
            .and_then(|classes| classes.get(&label))
            .cloned()
            .unwrap_or(label)
    }

    /// One executable ends in success, one of its error classes, or, inside a map item,
    /// not at all. Signals exist only on success and errors only on failure.
    fn node(&mut self, name: &NodeName, keys: &[SelectorKey], scope: &mut Scope) -> Option<()> {
        let node = self.inputs.nodes.get(name)?.node;
        let correlated = self.inputs.execution_correlations.contains_key(name);
        let source = |source| {
            keys.iter()
                .filter(move |key| key.source == source && !is_map_item_predicate(key))
        };
        let errors = source(ControlSourceKey::Error).collect::<Vec<_>>();
        let emitted = source(ControlSourceKey::Signal)
            .chain(source(ControlSourceKey::Group))
            .map(|key| {
                let mut labels = Vec::new();
                for label in selector_key_domain(key, node) {
                    let label = if key.source == ControlSourceKey::Signal {
                        self.class(key, label)
                    } else {
                        label
                    };
                    if !labels.contains(&label) {
                        labels.push(label);
                    }
                }
                (key, labels)
            })
            .collect::<Vec<_>>();
        let succeeds = emitted.iter().all(|(_, labels)| !labels.is_empty());
        let mut failures = Vec::<Vec<Option<EnumLabel>>>::new();
        if matches!(node, GraphNode::Step(_) | GraphNode::Verifier(_)) {
            for label in error_labels() {
                let classes = errors
                    .iter()
                    .map(|key| Some(self.class(key, label.clone())))
                    .collect();
                if !failures.contains(&classes) {
                    failures.push(classes);
                }
            }
        }
        let outcome = |emits, executed, errors| Status {
            emits,
            executed,
            errors,
        };
        let mut statuses = Vec::new();
        if succeeds {
            statuses.push(outcome(true, true, vec![None; errors.len()]));
        }
        statuses.extend(
            failures
                .into_iter()
                .map(|errors| outcome(false, true, errors)),
        );
        if correlated || statuses.is_empty() {
            if !correlated {
                scope.rules.push(NEVER);
            }
            statuses.push(outcome(false, false, vec![None; errors.len()]));
        }
        let status = self.diagram.variable(statuses.len())?;
        scope.variables.push((status, None));
        let by_status = |control: &dyn Fn(&Status) -> AssignedControl| Reader {
            gate: None,
            variable: status,
            controls: statuses.iter().map(control).collect(),
        };
        let emits = statuses
            .iter()
            .map(|status| status.emits)
            .collect::<Rc<_>>();
        for (key, labels) in emitted {
            let reader = if succeeds {
                let gate = Some((status, emits.clone()));
                let variable = self.diagram.variable(labels.len())?;
                scope.variables.push((variable, gate.clone()));
                Reader {
                    gate,
                    variable,
                    controls: labels
                        .into_iter()
                        .map(|label| labelled(Some(label)))
                        .collect(),
                }
            } else {
                by_status(&|_| empty_control())
            };
            scope.controls.insert(key.clone(), reader);
        }
        for (index, key) in errors.into_iter().enumerate() {
            let reader = by_status(&|status| labelled(status.errors[index].clone()));
            scope.controls.insert(key.clone(), reader);
        }
        if correlated {
            let reader =
                by_status(&|status| labelled(enum_label("executed").filter(|_| status.executed)));
            scope.controls.insert(execution_key(name), reader);
        }
        for key in keys.iter().filter(|key| is_map_item_predicate(key)) {
            let labels = proof_group_domain(node, key.field.as_ref())?;
            let variable = self.diagram.variable(labels.len())?;
            scope.variables.push((variable, None));
            let reader = Reader {
                gate: None,
                variable,
                controls: labels
                    .into_iter()
                    .map(|label| labelled(Some(label)))
                    .collect(),
            };
            scope.controls.insert(key.clone(), reader);
        }
        Some(())
    }

    /// A nested map contributes one variable: the saturated counts its items can add up to.
    /// The counts depend on enclosing controls only through the distinct conditions those
    /// controls leave for one item, so each such condition is folded once.
    fn map(&mut self, map: &MapNode, body: &ScopePlan<'_>, scope: &mut Scope) -> Option<()> {
        let keys = body.keys.iter().cloned().collect::<Vec<_>>();
        let item = self.scope(body, &scope.controls)?;
        let enclosing = self.diagram.above(item.legal, item.first)?;
        let mut left = BTreeSet::from([item.legal]);
        for node in &enclosing {
            left.extend(self.diagram.branches(*node).iter().copied());
        }
        left.retain(|node| self.diagram.variable_of(*node) >= item.first);
        // State zero is the map without a contributing item.
        let mut states = vec![empty_outcome(&keys)];
        let mut known = BTreeMap::from([(states[0].clone(), 0)]);
        let mut reachable = BTreeMap::new();
        for node in left {
            let outcomes = self.rows(node, &item, &keys)?;
            let counts = aggregate_assignments_from_outcomes(
                AggregateCounts {
                    keys: &keys,
                    outcomes: &outcomes,
                    count_limits: self.inputs.count_limits,
                },
                &mut self.diagram,
            )?;
            let mut members = BTreeSet::new();
            for state in counts {
                let next = states.len();
                members.insert(*known.entry(state.clone()).or_insert_with(|| {
                    states.push(state);
                    next
                }));
            }
            reachable.insert(node, members);
        }
        let variable = self.diagram.variable(states.len())?;
        let mut related = BTreeMap::new();
        for (node, members) in reachable {
            let member = (0..states.len()).map(|state| constant(members.contains(&state)));
            related.insert(node, self.diagram.test(variable, member)?);
        }
        for node in enclosing {
            let branches = self
                .diagram
                .branches(node)
                .iter()
                .map(|branch| related[branch])
                .collect::<Vec<_>>();
            let tested = self.diagram.variable_of(node);
            related.insert(node, self.diagram.test(tested, branches)?);
        }
        let mut relation = related[&item.legal];
        if let Some(correlation) = self.inputs.execution_correlations.get(&map.name) {
            let present = self
                .diagram
                .predicate(&scope.controls, &correlation.presence)?;
            let absent = (0..states.len()).map(|state| constant(state == 0));
            let absent = self.diagram.test(variable, absent)?;
            relation = self.diagram.choose(present, relation, absent)?;
        }
        scope.rules.push(relation);
        scope.variables.push((variable, None));
        for key in keys {
            let controls = states.iter().map(|state| state[&key].clone()).collect();
            let reader = Reader {
                gate: None,
                variable,
                controls,
            };
            scope.controls.insert(key, reader);
        }
        Some(())
    }

    /// The distinct outcomes one item shows its map under `condition`: every legal joint
    /// value of the item's own variables, read through `keys`.
    fn rows(
        &mut self,
        condition: Ref,
        item: &Scope,
        keys: &[SelectorKey],
    ) -> Option<Vec<Assignment>> {
        let mut values = vec![0; item.slot(self.diagram.variables())];
        let mut rows = BTreeSet::new();
        // Depth-first over the item's variables: the condition left and the next value to try.
        let mut stack = vec![(condition, 0)];
        while let Some((node, next)) = stack.last().copied() {
            if node == NEVER {
                stack.pop();
                continue;
            }
            let Some((variable, gate)) = item.variables.get(stack.len() - 1) else {
                let mut row = item.outcome(keys, &values);
                normalize_outcome_labels(
                    std::slice::from_mut(&mut row),
                    self.inputs.label_classes,
                )?;
                self.diagram.charge(1 + keys.len() / 4)?;
                rows.insert(row);
                if rows.len() as u64 > FULL_V1_MAX_GUARD_ASSIGNMENTS {
                    return None;
                }
                stack.pop();
                continue;
            };
            // A node that does not emit leaves its label variables without effect.
            let silent = item.silent(gate.as_ref(), &values);
            if next
                == if silent {
                    1
                } else {
                    self.diagram.domain(*variable)
                }
            {
                stack.pop();
                continue;
            }
            self.diagram.charge(1)?;
            if let Some(top) = stack.last_mut() {
                top.1 += 1;
            }
            values[item.slot(*variable)] = next;
            let below = if silent {
                node
            } else {
                self.diagram.follow(node, *variable, next)
            };
            stack.push((below, 0));
        }
        Some(rows.into_iter().collect())
    }

    fn constrain(
        &mut self,
        plan: &ScopePlan<'_>,
        inherited: &Controls,
        scope: &mut Scope,
    ) -> Option<()> {
        let local = plan.local.values().flatten().cloned().collect();
        let available = plan.keys.iter().chain(inherited.keys()).cloned().collect();
        for (name, correlation) in self.inputs.join_correlations {
            if correlation_is_available_to_scope(name, correlation, &local, &available) {
                let joined = self.join(name, correlation, &scope.controls)?;
                scope.rules.push(joined);
            }
        }
        for name in plan.local.keys() {
            if let Some(GraphNode::Loop(group)) = self.inputs.nodes.get(name).map(|info| info.node)
            {
                let terminated = self.terminated(group, &scope.controls)?;
                scope.rules.push(terminated);
            }
            if let Some(correlation) = self.inputs.execution_correlations.get(name) {
                let executed = self.executed(name, correlation, &scope.controls)?;
                scope.rules.push(executed);
            }
        }
        Some(())
    }

    /// A `joined`/`raced` control reports success exactly when enough branches qualify, and
    /// retained-branch witnesses name a set the join could have kept.
    pub(in crate::graph_verifier) fn join(
        &mut self,
        name: &NodeName,
        correlation: &ParallelJoinCorrelation,
        controls: &Controls,
    ) -> Option<Ref> {
        let retention = correlation.retention();
        let eligible = self.diagram.predicates(controls, &retention.branches)?;
        let witnesses = controls
            .keys()
            .filter(|key| key.name == *name && key.source == ControlSourceKey::Group)
            .filter_map(|key| Some((retention_index(key.field.as_ref())?, key.clone())))
            .collect::<BTreeMap<_, _>>();
        // A witness names its branch as kept or dropped. Branches without one fill the
        // winner slots the named ones leave open.
        let mut asked = Vec::new();
        let mut selected = Vec::new();
        let mut allowed = Vec::new();
        let mut winners = eligible.clone();
        for (index, key) in &witnesses {
            let named = self.diagram.control(controls, key, occurs)?;
            let kept = self
                .diagram
                .control(controls, key, |control| reports(control, "retained"))?;
            let branch = eligible.get(*index).copied().unwrap_or(NEVER);
            let dropped = self.diagram.not(kept)?;
            allowed.push(self.diagram.or(dropped, branch)?);
            if let Some(winner) = winners.get_mut(*index) {
                let unnamed = self.diagram.not(named)?;
                *winner = self.diagram.and(unnamed, branch)?;
            }
            asked.push(named);
            selected.push(kept);
        }
        let completed = self.diagram.at_least(retention.required, &eligible)?;
        let none = self.diagram.any(selected.iter().copied())?;
        let none = self.diagram.not(none)?;
        let excess = self
            .diagram
            .at_least(retention.required.saturating_add(1), &selected)?;
        let bounded = self.diagram.not(excess)?;
        winners.extend(selected);
        let filled = self.diagram.at_least(retention.required, &winners)?;
        allowed.extend([bounded, filled]);
        let kept = self.diagram.all(allowed)?;
        let kept = self.diagram.choose(completed, kept, none)?;
        let asked = self.diagram.any(asked)?;
        let unasked = self.diagram.not(asked)?;
        let retained = self.diagram.or(unasked, kept)?;

        let (decided, success, failure) = match correlation {
            ParallelJoinCorrelation::Joined { completion, .. } => {
                (completion, "reached", "quorum_unreachable")
            }
            ParallelJoinCorrelation::First { satisfaction, .. } => {
                (satisfaction, "satisfied", "no_satisfier")
            }
        };
        let key = SelectorKey {
            name: name.clone(),
            source: ControlSourceKey::Group,
            field: FieldName::new(correlation.field()).ok(),
        };
        if !controls.contains_key(&key) {
            return Some(retained);
        }
        let decided = self.diagram.predicate(controls, decided)?;
        let reported = self.reported(controls, &key, (decided, success, failure))?;
        let silent = self
            .diagram
            .control(controls, &key, |control| !occurs(control))?;
        let reported = self.diagram.or(silent, reported)?;
        self.diagram.and(retained, reported)
    }

    /// Holds where the control carries `success` with `decided` or `failure` without it.
    fn reported(
        &mut self,
        controls: &Controls,
        key: &SelectorKey,
        (decided, success, failure): (Ref, &str, &str),
    ) -> Option<Ref> {
        let succeeded = self
            .diagram
            .control(controls, key, |control| reports(control, success))?;
        let failed = self
            .diagram
            .control(controls, key, |control| reports(control, failure))?;
        let succeeded = self.diagram.and(succeeded, decided)?;
        let undecided = self.diagram.not(decided)?;
        let failed = self.diagram.and(failed, undecided)?;
        self.diagram.or(succeeded, failed)
    }

    /// A finished loop reports `converged` exactly when its exit guard holds on the final body.
    fn terminated(&mut self, group: &LoopNode, controls: &Controls) -> Option<Ref> {
        let key = SelectorKey {
            name: group.name.clone(),
            source: ControlSourceKey::Group,
            field: FieldName::new("terminated").ok(),
        };
        if !controls.contains_key(&key) {
            return Some(ALWAYS);
        }
        let converged = match &group.until {
            Some(until) => self.diagram.guard(controls, until)?,
            None => NEVER,
        };
        let legal = |control: &AssignedControl, expected: &str| {
            control
                .counts
                .iter()
                .all(|(label, count)| *count == 0 || label.as_str() == expected)
        };
        let when_converged = self
            .diagram
            .control(controls, &key, |control| legal(control, "converged"))?;
        let when_exhausted = self
            .diagram
            .control(controls, &key, |control| legal(control, "exhausted"))?;
        self.diagram
            .choose(converged, when_converged, when_exhausted)
    }

    /// A node inside a map item runs exactly when its route holds, and an item-completion
    /// marker reports `complete` exactly when the body completes.
    fn executed(
        &mut self,
        name: &NodeName,
        correlation: &MapExecutionCorrelation,
        controls: &Controls,
    ) -> Option<Ref> {
        let present = self.diagram.predicate(controls, &correlation.presence)?;
        let executed = self
            .diagram
            .control(controls, &execution_key(name), occurs)?;
        let idle = self.diagram.not(executed)?;
        let mut legal = self.diagram.choose(present, executed, idle)?;
        for (field, predicate) in &correlation.item_predicates {
            let key = SelectorKey {
                name: name.clone(),
                source: ControlSourceKey::Group,
                field: Some(field.clone()),
            };
            if !controls.contains_key(&key) {
                continue;
            }
            let complete = self.diagram.predicate(controls, predicate)?;
            let reported = self.reported(controls, &key, (complete, "complete", "blocked"))?;
            legal = self.diagram.and(legal, reported)?;
        }
        Some(legal)
    }

    /// Controls of a branch its join did not keep leave the item empty. The retaining rule
    /// belongs to the scope that owns the join, so nested facts stay visible until then.
    fn mask(&mut self, plan: &ScopePlan<'_>, scope: &mut Scope) -> Option<()> {
        let Some(owner) = &plan.owner else {
            return Some(());
        };
        let mut sources = BTreeMap::new();
        for key in &plan.keys {
            let mut source = &key.name;
            let mut correlation = self.inputs.execution_correlations.get(source);
            while let Some(found) = correlation.filter(|found| found.owner != *owner) {
                source = &found.owner;
                correlation = self.inputs.execution_correlations.get(source);
            }
            let Some(correlation) = correlation else {
                continue;
            };
            if matches!(correlation.retention, CompletionPredicate::Always) {
                continue;
            }
            let variable = match sources.get(source) {
                Some(variable) => *variable,
                None => {
                    let retained = self
                        .diagram
                        .predicate(&scope.controls, &correlation.retention)?;
                    let variable = self.diagram.variable(2)?;
                    let kept = self.diagram.test(variable, [NEVER, ALWAYS])?;
                    let dropped = self.diagram.not(kept)?;
                    let tied = self.diagram.choose(retained, kept, dropped)?;
                    scope.rules.push(tied);
                    scope.variables.push((variable, None));
                    sources.insert(source.clone(), variable);
                    variable
                }
            };
            scope.masks.insert(key.name.clone(), variable);
        }
        Some(())
    }
}
