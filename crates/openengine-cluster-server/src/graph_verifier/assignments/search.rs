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
    /// The check's own scope, as opposed to one item of an enclosing map.
    root: bool,
}

/// One item condition walked through the item's variables. `narrowed[i]` is the single
/// condition every legal outcome is left with before variable `i`, where there is one, and
/// `forced[i]` the single value legal outcomes give variable `i`, where there is one.
struct Passage {
    narrowed: Vec<Option<Ref>>,
    forced: Vec<Option<usize>>,
    /// One value per variable created since the item began. Forced variables hold their
    /// value; the others are scratch for the part being listed.
    values: Vec<usize>,
}

/// A run of an item's variables that no rule ties to the rest, with the selectors read
/// through it and their count states. Its variable exists once the states are known.
struct Part {
    range: std::ops::Range<usize>,
    counts: CountStates,
    variable: u32,
}

/// One map's items folded: the nodes of the item condition that read enclosing controls, the
/// item conditions those leave, the parts, and per condition the states the parts reach by
/// number of items. `count` picks that number where it matters.
struct Folded {
    enclosing: Vec<Ref>,
    conditions: Vec<Ref>,
    parts: Vec<Part>,
    layers: Vec<CountLayers>,
    width: usize,
    count: Option<u32>,
}

impl Part {
    fn new(range: std::ops::Range<usize>, keys: Vec<SelectorKey>) -> Self {
        Self {
            range,
            counts: CountStates::new(keys),
            variable: 0,
        }
    }
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

/// Splits an item's variables where every item condition has narrowed to one condition, and
/// keeps together what one selector is read through. A variable that each condition forces
/// to one value ties nothing together: whoever reads it finds that value filled in.
fn parts(body: &ScopePlan<'_>, item: &Scope, passages: &[(Ref, Passage)]) -> Vec<Part> {
    let positions = item
        .variables
        .iter()
        .enumerate()
        .map(|(position, (variable, _))| (*variable, position))
        .collect::<BTreeMap<_, _>>();
    let varies = |position: &usize| {
        passages
            .iter()
            .any(|(_, passage)| passage.forced[*position].is_none())
    };
    // reach[p]: the last position that some selector read at `p` is also read through.
    let mut reach = (0..item.variables.len()).collect::<Vec<_>>();
    // The first varying position each selector is read at, or its own when none varies.
    let mut homes = BTreeMap::<usize, Vec<SelectorKey>>::new();
    for key in &body.keys {
        let reader = &item.controls[key];
        let gate = reader.gate.as_ref().map(|(gate, _)| *gate);
        let read = [
            Some(reader.variable),
            gate,
            item.masks.get(&key.name).copied(),
        ]
        .into_iter()
        .flatten()
        .map(|variable| positions[&variable])
        .filter(varies);
        let (low, high) = read.fold((None, None), |(low, high), position| {
            (
                Some(low.map_or(position, |low: usize| low.min(position))),
                Some(high.map_or(position, |high: usize| high.max(position))),
            )
        });
        if let (Some(low), Some(high)) = (low, high) {
            reach[low] = reach[low].max(high);
        }
        let home = low.unwrap_or(positions[&reader.variable]);
        homes.entry(home).or_default().push(key.clone());
    }
    let mut parts = Vec::new();
    let mut start = 0;
    let mut read_through = 0;
    for (position, reach) in reach.into_iter().enumerate() {
        read_through = read_through.max(reach);
        let end = position + 1;
        let narrowed = passages
            .iter()
            .all(|(_, passage)| passage.narrowed[end].is_some());
        if end < item.variables.len() && (read_through >= end || !narrowed) {
            continue;
        }
        let keys = homes
            .range(start..end)
            .flat_map(|(_, keys)| keys.iter().cloned())
            .collect::<Vec<_>>();
        if !keys.is_empty() {
            parts.push(Part::new(start..end, keys));
        }
        start = end;
    }
    parts
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
            root: plan.owner.is_none(),
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

    /// A map contributes the saturated counts its items can add up to. The counts depend on
    /// enclosing controls only through the distinct conditions those controls leave for one
    /// item, so each such condition is folded once. Parts of an item that no rule ties
    /// together are folded on their own and meet only in the number of items.
    fn map(&mut self, map: &MapNode, body: &ScopePlan<'_>, scope: &mut Scope) -> Option<()> {
        let item = self.scope(body, &scope.controls)?;
        let folded = self.folded(body, &item, scope.root)?;
        let mut relation = self.relation(&folded, item.legal)?;
        if let Some(correlation) = self.inputs.execution_correlations.get(&map.name) {
            let present = self
                .diagram
                .predicate(&scope.controls, &correlation.presence)?;
            let mut absent = vec![self.members(&folded.parts, |_, state| state == 0)?];
            absent.resize(folded.width, NEVER);
            let absent = self.counted(folded.count, absent)?;
            relation = self.diagram.choose(present, relation, absent)?;
        }
        scope.rules.push(relation);
        scope
            .variables
            .extend(folded.count.map(|count| (count, None)));
        for part in folded.parts {
            scope.variables.push((part.variable, None));
            for key in &part.counts.keys {
                let controls = part.counts.states.iter().map(|state| state[key].clone());
                let reader = Reader {
                    gate: None,
                    variable: part.variable,
                    controls: controls.collect(),
                };
                scope.controls.insert(key.clone(), reader);
            }
        }
        Some(())
    }

    /// Folds one item under every condition its enclosing controls leave for it, and gives
    /// the count states their variables.
    fn folded(&mut self, body: &ScopePlan<'_>, item: &Scope, split: bool) -> Option<Folded> {
        let enclosing = self.diagram.above(item.legal, item.first)?;
        let mut left = BTreeSet::from([item.legal]);
        for node in &enclosing {
            left.extend(self.diagram.branches(*node).iter().copied());
        }
        left.retain(|node| self.diagram.variable_of(*node) >= item.first);
        // Parts are looked for in the check's own scope only. One level up, an inner map's
        // count variable and parts would be one part of the outer item again.
        let mut passages = Vec::new();
        for node in left {
            passages.push((node, self.passage(node, item, split)?));
        }
        let whole = || {
            let keys = body.keys.iter().cloned().collect();
            vec![Part::new(0..item.variables.len(), keys)]
        };
        let mut parts = match split {
            true => parts(body, item, &passages),
            false => whole(),
        };
        let layers = match self.fold(item, &mut passages, &mut parts)? {
            Some(layers) => layers,
            // A part depends on the exact number of items for too long: fold the item whole.
            // One part alone never depends on that number, so the second attempt settles.
            None => {
                parts = whole();
                self.fold(item, &mut passages, &mut parts)??
            }
        };
        // Item conditions whose parts depend on the number of items share a count variable.
        let width = layers.iter().map(Vec::len).max()?;
        let count = match width {
            1 => None,
            _ => Some(self.diagram.variable(width)?),
        };
        for part in &mut parts {
            part.variable = self.diagram.variable(part.counts.states.len())?;
        }
        Some(Folded {
            enclosing,
            conditions: passages.into_iter().map(|(node, _)| node).collect(),
            parts,
            layers,
            width,
            count,
        })
    }

    /// Holds where the enclosing controls leave an item condition and the parts are in
    /// states that condition reaches with some number of items.
    fn relation(&mut self, folded: &Folded, legal: Ref) -> Option<Ref> {
        let mut related = BTreeMap::new();
        for (node, entries) in folded.conditions.iter().zip(&folded.layers) {
            // The count variable only picks an entry of the condition that applies, so
            // values past that condition's last entry are never taken.
            let mut reached = Vec::with_capacity(folded.width);
            for entry in entries {
                let members = |part: usize, state: usize| entry[part].contains(&state);
                reached.push(self.members(&folded.parts, members)?);
            }
            reached.resize(folded.width, NEVER);
            related.insert(*node, self.counted(folded.count, reached)?);
        }
        for node in &folded.enclosing {
            let branches = self
                .diagram
                .branches(*node)
                .iter()
                .map(|branch| related[branch])
                .collect::<Vec<_>>();
            let tested = self.diagram.variable_of(*node);
            related.insert(*node, self.diagram.test(tested, branches)?);
        }
        Some(related[&legal])
    }

    /// Holds where every part is in a state `contains` accepts.
    fn members(&mut self, parts: &[Part], contains: impl Fn(usize, usize) -> bool) -> Option<Ref> {
        let mut members = Vec::with_capacity(parts.len());
        for (index, part) in parts.iter().enumerate() {
            let states = part.counts.states.len();
            self.diagram.charge(1 + states / 16)?;
            let member = (0..states).map(|state| constant(contains(index, state)));
            members.push(self.diagram.test(part.variable, member)?);
        }
        self.diagram.all(members)
    }

    /// Holds where the number of items, as far as it matters, selects a condition that holds.
    fn counted(&mut self, count: Option<u32>, reached: Vec<Ref>) -> Option<Ref> {
        match count {
            Some(count) => self.diagram.test(count, reached),
            None => reached.first().copied(),
        }
    }

    /// Walks one item condition through the item's variables, or only sets up the values
    /// for listing the item whole.
    fn passage(&mut self, condition: Ref, item: &Scope, walk: bool) -> Option<Passage> {
        let slots = item.slot(self.diagram.variables());
        self.diagram.charge(1 + slots / 16)?;
        let mut passage = Passage {
            narrowed: vec![Some(condition)],
            forced: Vec::new(),
            values: vec![0; slots],
        };
        if !walk {
            return Some(passage);
        }
        let mut open = BTreeSet::from([condition]);
        open.remove(&NEVER);
        for (variable, _) in &item.variables {
            let mut below = BTreeSet::new();
            let mut live = BTreeSet::new();
            for node in &open {
                self.diagram
                    .charge(1 + self.diagram.domain(*variable) / 16)?;
                for index in 0..self.diagram.domain(*variable) {
                    let next = self.diagram.follow(*node, *variable, index);
                    if next != NEVER {
                        below.insert(next);
                        live.insert(index);
                    }
                }
            }
            let forced = live.first().copied().filter(|_| live.len() == 1);
            passage.values[item.slot(*variable)] = forced.unwrap_or(0);
            passage.forced.push(forced);
            // No legal outcome at all narrows to the condition that never holds.
            passage.narrowed.push(match below.len() {
                0 => Some(NEVER),
                1 => below.first().copied(),
                _ => None,
            });
            open = below;
        }
        Some(passage)
    }

    /// The count states each item condition lets the parts reach, by number of items. The
    /// inner `None` says the parts are not worth folding apart.
    fn fold(
        &mut self,
        item: &Scope,
        passages: &mut [(Ref, Passage)],
        parts: &mut [Part],
    ) -> Option<Option<Vec<CountLayers>>> {
        let mut folded = Vec::with_capacity(passages.len());
        for (_, passage) in passages {
            let mut outcomes = Vec::with_capacity(parts.len());
            for part in parts.iter() {
                outcomes.push(self.rows(item, part, passage)?);
            }
            let mut read = parts
                .iter_mut()
                .zip(outcomes)
                .map(|(part, outcomes)| PartOutcomes::new(&mut part.counts, outcomes))
                .collect::<Vec<_>>();
            let layers = count_layers(&mut read, self.inputs.count_limits, &mut self.diagram)?;
            let Some(layers) = layers else {
                return Some(None);
            };
            folded.push(layers);
        }
        Some(Some(folded))
    }

    /// The distinct outcomes one item shows its map through one part: every legal joint
    /// value of the part's variables under the item condition, read through its selectors.
    fn rows(
        &mut self,
        item: &Scope,
        part: &Part,
        passage: &mut Passage,
    ) -> Option<Vec<Assignment>> {
        let keys = &part.counts.keys;
        let mut rows = BTreeSet::new();
        // Depth-first over the part's variables: the condition left and the next value to try.
        let mut stack = vec![(passage.narrowed[part.range.start]?, 0)];
        while let Some((node, next)) = stack.last().copied() {
            if node == NEVER {
                stack.pop();
                continue;
            }
            let position = part.range.start + stack.len() - 1;
            let Some((variable, gate)) = item.variables[..part.range.end].get(position) else {
                let mut row = item.outcome(keys, &passage.values);
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
            let silent = item.silent(gate.as_ref(), &passage.values);
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
            passage.values[item.slot(*variable)] = next;
            let below = if silent {
                node
            } else {
                self.diagram.follow(node, *variable, next)
            };
            stack.push((below, 0));
        }
        // Other parts read this part's forced variables: leave those at their value.
        for position in part.range.clone() {
            let forced = passage.forced.get(position).copied().flatten();
            passage.values[item.slot(item.variables[position].0)] = forced.unwrap_or(0);
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
