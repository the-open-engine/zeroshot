//! Reduced ordered decision diagrams over finite control variables.
//!
//! A diagram is one condition over the controls it reads. Equal sub-conditions are stored
//! once, so conditions over unrelated controls add their sizes instead of multiplying them.
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap};

use super::FULL_V1_MAX_GUARD_SEARCH_STEPS;

pub(in crate::graph_verifier) type Ref = u32;
pub(in crate::graph_verifier) const NEVER: Ref = 0;
pub(in crate::graph_verifier) const ALWAYS: Ref = 1;
const TERMINAL: u32 = u32::MAX;
const PENDING: Ref = u32::MAX;

struct Node {
    variable: u32,
    branches: Box<[Ref]>,
}

/// The conditions of one check. A node is always created after its branches.
pub(in crate::graph_verifier) struct Diagram {
    domains: Vec<u32>,
    nodes: Vec<Node>,
    unique: HashMap<(u32, Box<[Ref]>), Ref>,
    conjunctions: HashMap<(Ref, Ref), Ref>,
    negations: HashMap<Ref, Ref>,
    steps: u64,
}

impl Default for Diagram {
    fn default() -> Self {
        let terminal = || Node {
            variable: TERMINAL,
            branches: Box::default(),
        };
        Self {
            domains: Vec::new(),
            nodes: vec![terminal(), terminal()],
            unique: HashMap::new(),
            conjunctions: HashMap::new(),
            negations: HashMap::new(),
            steps: 0,
        }
    }
}

pub(in crate::graph_verifier) const fn constant(value: bool) -> Ref {
    if value { ALWAYS } else { NEVER }
}

impl Diagram {
    /// Counts search work. `None` means this check passed its backstop; the caller reports
    /// that at the condition it was deciding.
    pub(in crate::graph_verifier) fn charge(&mut self, steps: usize) -> Option<()> {
        self.steps = self.steps.saturating_add(steps as u64);
        (self.steps <= FULL_V1_MAX_GUARD_SEARCH_STEPS).then_some(())
    }

    pub(in crate::graph_verifier) fn steps(&self) -> u64 {
        self.steps
    }

    /// A new variable with `domain` values. Later variables sit below earlier ones.
    pub(in crate::graph_verifier) fn variable(&mut self, domain: usize) -> Option<u32> {
        let variable = u32::try_from(self.domains.len()).ok()?;
        self.domains.push(u32::try_from(domain).ok()?);
        Some(variable)
    }

    pub(in crate::graph_verifier) fn variables(&self) -> u32 {
        self.domains.len() as u32
    }

    pub(in crate::graph_verifier) fn domain(&self, variable: u32) -> usize {
        self.domains[variable as usize] as usize
    }

    pub(in crate::graph_verifier) fn variable_of(&self, node: Ref) -> u32 {
        self.nodes[node as usize].variable
    }

    pub(in crate::graph_verifier) fn branches(&self, node: Ref) -> &[Ref] {
        &self.nodes[node as usize].branches
    }

    /// The condition that holds when `variable` takes a value whose branch holds. Every
    /// branch must read later variables only.
    pub(in crate::graph_verifier) fn test(
        &mut self,
        variable: u32,
        branches: impl IntoIterator<Item = Ref>,
    ) -> Option<Ref> {
        let branches = branches.into_iter().collect::<Box<[Ref]>>();
        let first = *branches.first()?;
        if branches.iter().all(|branch| *branch == first) {
            return Some(first);
        }
        let node = Ref::try_from(self.nodes.len()).ok()?;
        match self.unique.entry((variable, branches)) {
            Entry::Occupied(known) => Some(*known.get()),
            Entry::Vacant(unknown) => {
                self.steps += 1;
                if self.steps > FULL_V1_MAX_GUARD_SEARCH_STEPS {
                    return None;
                }
                let branches = unknown.key().1.clone();
                self.nodes.push(Node { variable, branches });
                unknown.insert(node);
                Some(node)
            }
        }
    }

    /// The condition left of `node` once `variable` is known to take value `index`.
    pub(in crate::graph_verifier) fn follow(&self, node: Ref, variable: u32, index: usize) -> Ref {
        let source = &self.nodes[node as usize];
        if source.variable == variable {
            source.branches[index]
        } else {
            node
        }
    }

    /// Nodes reachable from `root` through nodes `open` accepts, branches before the nodes
    /// that test them.
    fn reachable(&mut self, root: Ref, open: impl Fn(&Self, Ref) -> bool) -> Option<Vec<Ref>> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if !open(self, node) || !seen.insert(node) {
                continue;
            }
            self.charge(1)?;
            stack.extend(self.branches(node).iter().copied());
        }
        Some(seen.into_iter().collect())
    }

    /// Nodes reachable from `root` that test a variable before `floor`, branches first.
    pub(in crate::graph_verifier) fn above(&mut self, root: Ref, floor: u32) -> Option<Vec<Ref>> {
        self.reachable(root, |diagram, node| diagram.variable_of(node) < floor)
    }

    pub(in crate::graph_verifier) fn not(&mut self, node: Ref) -> Option<Ref> {
        let unknown = |diagram: &Self, node: Ref| {
            diagram.variable_of(node) != TERMINAL && !diagram.negations.contains_key(&node)
        };
        for inner in self.reachable(node, unknown)? {
            let variable = self.variable_of(inner);
            let branches = self
                .branches(inner)
                .iter()
                .map(|branch| self.negated(*branch))
                .collect::<Vec<_>>();
            let negation = self.test(variable, branches)?;
            self.negations.insert(inner, negation);
            self.negations.insert(negation, inner);
        }
        Some(self.negated(node))
    }

    fn negated(&self, node: Ref) -> Ref {
        match node {
            NEVER => ALWAYS,
            ALWAYS => NEVER,
            node => self.negations[&node],
        }
    }

    fn conjoined(&self, left: Ref, right: Ref) -> Option<Ref> {
        if left == NEVER || right == NEVER {
            Some(NEVER)
        } else if left == ALWAYS || left == right {
            Some(right)
        } else if right == ALWAYS {
            Some(left)
        } else {
            self.conjunctions
                .get(&(left.min(right), left.max(right)))
                .copied()
        }
    }

    pub(in crate::graph_verifier) fn and(&mut self, left: Ref, right: Ref) -> Option<Ref> {
        let mut open = Vec::new();
        let mut stack = vec![(left.min(right), left.max(right))];
        while let Some(pair) = stack.pop() {
            if self.conjoined(pair.0, pair.1).is_some() {
                continue;
            }
            self.charge(1)?;
            self.conjunctions.insert(pair, PENDING);
            open.push(pair);
            let variable = self.variable_of(pair.0).min(self.variable_of(pair.1));
            for index in 0..self.domain(variable) {
                let low = self.follow(pair.0, variable, index);
                let high = self.follow(pair.1, variable, index);
                stack.push((low.min(high), low.max(high)));
            }
        }
        // Both members of a branch pair are no newer than their parent's and one is older.
        open.sort_by_key(|pair| u64::from(pair.0) + u64::from(pair.1));
        for pair in open {
            let variable = self.variable_of(pair.0).min(self.variable_of(pair.1));
            let branches = (0..self.domain(variable))
                .map(|index| {
                    self.conjoined(
                        self.follow(pair.0, variable, index),
                        self.follow(pair.1, variable, index),
                    )
                })
                .collect::<Option<Vec<_>>>()?;
            let node = self.test(variable, branches)?;
            self.conjunctions.insert(pair, node);
        }
        self.conjoined(left, right)
    }

    pub(in crate::graph_verifier) fn or(&mut self, left: Ref, right: Ref) -> Option<Ref> {
        let left = self.not(left)?;
        let right = self.not(right)?;
        let neither = self.and(left, right)?;
        self.not(neither)
    }

    /// Conjoins the deepest conditions first: joining a condition above its partner costs
    /// only that condition's size, so unrelated conditions add up linearly.
    pub(in crate::graph_verifier) fn all(
        &mut self,
        conditions: impl IntoIterator<Item = Ref>,
    ) -> Option<Ref> {
        let mut conditions = conditions.into_iter().collect::<Vec<_>>();
        conditions.sort_by_key(|condition| std::cmp::Reverse(self.variable_of(*condition)));
        conditions
            .into_iter()
            .try_fold(ALWAYS, |all, condition| self.and(condition, all))
    }

    pub(in crate::graph_verifier) fn any(
        &mut self,
        conditions: impl IntoIterator<Item = Ref>,
    ) -> Option<Ref> {
        let excluded = conditions
            .into_iter()
            .map(|condition| self.not(condition))
            .collect::<Option<Vec<_>>>()?;
        let none = self.all(excluded)?;
        self.not(none)
    }

    /// Holds where `condition` holds with `then`, or fails with `otherwise`.
    pub(in crate::graph_verifier) fn choose(
        &mut self,
        condition: Ref,
        then: Ref,
        otherwise: Ref,
    ) -> Option<Ref> {
        let taken = self.and(condition, then)?;
        let skipped = self.not(condition)?;
        let skipped = self.and(skipped, otherwise)?;
        self.or(taken, skipped)
    }

    /// Holds when at least `count` of `conditions` hold.
    pub(in crate::graph_verifier) fn at_least(
        &mut self,
        count: u64,
        conditions: &[Ref],
    ) -> Option<Ref> {
        let total = conditions.len();
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        if count > total {
            return Some(NEVER);
        }
        // held[needed]: at least `needed` of the conditions folded so far hold. Folding starts
        // from the last condition for the same reason `all` starts from the deepest.
        let mut held = vec![NEVER; count + 1];
        held[0] = ALWAYS;
        for (folded, condition) in conditions.iter().rev().enumerate() {
            // Thresholds the remaining conditions cannot lift to `count` are never read.
            let lowest = count.saturating_sub(total - folded - 1).max(1);
            for needed in (lowest..=count.min(folded + 1)).rev() {
                held[needed] = self.choose(*condition, held[needed - 1], held[needed])?;
            }
        }
        Some(held[count])
    }
}
