use super::*;

pub(in crate::graph_verifier) struct AggregateCounts<'a> {
    pub(super) keys: &'a [SelectorKey],
    pub(super) outcomes: &'a [Assignment],
    pub(super) count_limits: &'a CountLimits,
}

/// Every saturated count state any number of items can add up to.
pub(in crate::graph_verifier) fn aggregate_assignments_from_outcomes(
    context: AggregateCounts<'_>,
    diagram: &mut Diagram,
) -> Option<Vec<Assignment>> {
    let empty = empty_outcome(context.keys);
    // Adding an item copies one count per selector; four counts weigh one search step.
    let weight = 1 + context.keys.len() / 4;
    // Close over finite input cardinalities using saturated correlated counts.
    let mut states = BTreeSet::from([empty.clone()]);
    let mut frontier = BTreeSet::from([empty]);
    loop {
        let mut next = BTreeSet::new();
        for state in &frontier {
            for outcome in context.outcomes {
                diagram.charge(weight)?;
                let candidate = add_aggregate_outcome(state, outcome, context.count_limits)?;
                if !states.contains(&candidate) {
                    next.insert(candidate);
                }
            }
        }
        if next.is_empty() {
            return Some(states.into_iter().collect());
        }
        if states.len().checked_add(next.len())? as u64 > FULL_V1_MAX_GUARD_ASSIGNMENTS {
            return None;
        }
        states.extend(next.iter().cloned());
        frontier = next;
    }
}

/// The saturated count states of the selectors one part of a map item is read through.
/// State zero is the map without a contributing item.
pub(in crate::graph_verifier) struct CountStates {
    pub(super) keys: Vec<SelectorKey>,
    pub(super) states: Vec<Assignment>,
    known: BTreeMap<Assignment, usize>,
}

impl CountStates {
    pub(super) fn new(keys: Vec<SelectorKey>) -> Self {
        let empty = empty_outcome(&keys);
        Self {
            known: BTreeMap::from([(empty.clone(), 0)]),
            states: vec![empty],
            keys,
        }
    }

    pub(super) fn index(&mut self, state: Assignment) -> usize {
        let next = self.states.len();
        *self.known.entry(state.clone()).or_insert_with(|| {
            self.states.push(state);
            next
        })
    }
}

/// One part of a map item: its count states and the outcomes one item shows through it.
pub(in crate::graph_verifier) struct PartOutcomes<'a> {
    counts: &'a mut CountStates,
    outcomes: Vec<Assignment>,
    /// The states one more item turns a state into, worked out once per state.
    added: BTreeMap<usize, Vec<usize>>,
}

impl<'a> PartOutcomes<'a> {
    pub(super) fn new(counts: &'a mut CountStates, outcomes: Vec<Assignment>) -> Self {
        Self {
            counts,
            outcomes,
            added: BTreeMap::new(),
        }
    }

    /// The states one more item makes of `states`.
    fn with_one_more(
        &mut self,
        states: &BTreeSet<usize>,
        count_limits: &CountLimits,
        diagram: &mut Diagram,
    ) -> Option<BTreeSet<usize>> {
        diagram.charge(1 + states.len() / 16)?;
        let mut reached = BTreeSet::new();
        for state in states {
            if !self.added.contains_key(state) {
                let weight = 1 + self.counts.keys.len() / 4;
                diagram.charge(self.outcomes.len() * weight)?;
                let mut sums = Vec::with_capacity(self.outcomes.len());
                for outcome in &self.outcomes {
                    let sum =
                        add_aggregate_outcome(&self.counts.states[*state], outcome, count_limits)?;
                    sums.push(self.counts.index(sum));
                }
                self.added.insert(*state, sums);
            }
            reached.extend(self.added[state].iter().copied());
        }
        (self.counts.states.len() as u64 <= FULL_V1_MAX_GUARD_ASSIGNMENTS).then_some(reached)
    }
}

/// For each number of items in turn, the states each part can then be in.
pub(in crate::graph_verifier) type CountLayers = Vec<Vec<BTreeSet<usize>>>;

/// How many items may matter one by one when parts are folded apart. A part whose states
/// still depend on the exact number of items beyond this is cheaper to fold with the rest.
const MAX_COUNTED_ITEMS: usize = 64;

/// Count states of the independent parts of one item, by number of items: entry `n` holds
/// for each part the states exactly `n` items add up to, and the last entry stands for every
/// larger number too. Parts share nothing but the number of items, so the joint states are
/// the products within one entry.
///
/// Some parts need no particular number. A part an item can leave untouched only gains
/// states with more items; a part the item condition never shows stays empty; and a part on
/// its own has nothing to agree with. The entries end once every other part has settled,
/// which happens because counts only grow and saturate. The inner `None` says that this
/// takes more than `MAX_COUNTED_ITEMS` items.
pub(in crate::graph_verifier) fn count_layers(
    parts: &mut [PartOutcomes<'_>],
    count_limits: &CountLimits,
    diagram: &mut Diagram,
) -> Option<Option<CountLayers>> {
    let alone = parts.len() == 1;
    let (free, counted): (Vec<_>, Vec<_>) = (0..parts.len()).partition(|part| {
        let part = &parts[*part];
        alone || part.outcomes.is_empty() || part.outcomes.contains(&part.counts.states[0])
    });
    let mut layers = Vec::new();
    let mut current = vec![BTreeSet::from([0]); parts.len()];
    loop {
        // Only the counted parts decide whether another entry is needed, so the free ones
        // are advanced after that test and their work is spent only on entries that stay.
        let mut next = current.clone();
        for part in &counted {
            next[*part] = parts[*part].with_one_more(&current[*part], count_limits, diagram)?;
        }
        if next == current {
            break;
        }
        if layers.len() >= MAX_COUNTED_ITEMS {
            return Some(None);
        }
        for part in &free {
            next[*part] = parts[*part].with_one_more(&current[*part], count_limits, diagram)?;
        }
        layers.push(std::mem::replace(&mut current, next));
    }
    // From here on the counted parts stay as they are and a free part can be in any state
    // it ever reaches.
    for part in free {
        let part_states = &mut parts[part];
        let states = aggregate_assignments_from_outcomes(
            AggregateCounts {
                keys: &part_states.counts.keys,
                outcomes: &part_states.outcomes,
                count_limits,
            },
            diagram,
        )?;
        current[part] = states
            .into_iter()
            .map(|state| part_states.counts.index(state))
            .collect();
    }
    layers.push(current);
    Some(Some(layers))
}

fn add_aggregate_outcome(
    state: &Assignment,
    outcome: &Assignment,
    count_limits: &CountLimits,
) -> Option<Assignment> {
    let mut next = state.clone();
    for (key, control) in outcome {
        let target = next.get_mut(key)?;
        for (label, count) in &control.counts {
            // Refinement and correlations observe presence even when no authored guard does.
            let limit = count_limits
                .get(key)
                .and_then(|limits| limits.get(label))
                .copied()
                .unwrap_or(1);
            let previous = target.counts.entry(label.clone()).or_default();
            *previous = previous.saturating_add(*count).min(limit);
        }
    }
    Some(next)
}

pub(in crate::graph_verifier) fn assigned_control(
    label: Option<EnumLabel>,
    occurrences: u64,
) -> AssignedControl {
    AssignedControl {
        counts: label
            .map(|label| BTreeMap::from([(label, occurrences)]))
            .unwrap_or_default(),
    }
}

pub(in crate::graph_verifier) fn selector_key_domain(
    key: &SelectorKey,
    node: &GraphNode,
) -> Vec<EnumLabel> {
    match key.source {
        ControlSourceKey::Signal => match node {
            GraphNode::Verifier(verifier) => key
                .field
                .as_ref()
                .and_then(|field| verifier.signals.get(field))
                .map(|labels| labels.values().to_vec())
                .unwrap_or_default(),
            _ => Vec::new(),
        },
        ControlSourceKey::Error => error_labels(),
        ControlSourceKey::Group => proof_group_domain(node, key.field.as_ref()).unwrap_or_default(),
        ControlSourceKey::Execution => enum_label("executed").into_iter().collect(),
    }
}

pub(in crate::graph_verifier) fn group_domain(
    node: &GraphNode,
    field: Option<&FieldName>,
) -> Option<Vec<EnumLabel>> {
    let expected = group_domain_labels(node, field?.as_str())?;
    Some(
        expected
            .iter()
            .filter_map(|label| enum_label(label))
            .collect(),
    )
}

fn group_domain_labels(node: &GraphNode, field: &str) -> Option<&'static [&'static str]> {
    match field {
        "terminated" => {
            matches!(node, GraphNode::Loop(_)).then_some(&["converged", "exhausted"][..])
        }

        "joined" => {
            parallel_join_domain(node, false).then_some(&["reached", "quorum_unreachable"][..])
        }
        "raced" => parallel_join_domain(node, true).then_some(&["satisfied", "no_satisfier"][..]),
        _ => None,
    }
}

fn parallel_join_domain(node: &GraphNode, first: bool) -> bool {
    let GraphNode::Par(parallel) = node else {
        return false;
    };
    matches!(parallel.join, Join::First { .. }) == first
}

pub(in crate::graph_verifier) fn error_labels() -> Vec<EnumLabel> {
    ["timeout", "crash", "malformed", "refusal"]
        .into_iter()
        .filter_map(enum_label)
        .collect()
}

pub(in crate::graph_verifier) fn signal_path_type(
    verifier: &VerifierNode,
    path: &FieldPath,
) -> Option<PayloadType> {
    let [field] = path.segments() else {
        return None;
    };
    verifier
        .signals
        .get(field)
        .cloned()
        .map(|values| PayloadType::Enum { values })
}
