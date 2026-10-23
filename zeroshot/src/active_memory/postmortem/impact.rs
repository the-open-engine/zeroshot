//! Conservative admission evidence, computed from main-run facts rather than model claims.

use std::collections::{BTreeMap, BTreeSet};

use openengine_cluster_protocol::{
    ControlSource, GraphNode, Guard, Join, LoopNode, WorkerErrorCode, WorkerOutcome,
};
use serde::Serialize;

use super::plan::Role;
use super::MainRunFacts;
use crate::active_memory::ToolObservation;
use crate::native_v2_contract::{ExecutionId, NodeRuntimeBinding};
use crate::v2_run_ledger::{NodeSnapshot, NodeState, cursor_sequence};

const SUBSTANTIAL_EXPLORATION_MS: u64 = 5 * 60 * 1000;
const MIN_TOOL_OBSERVATIONS: usize = 3;
const MIN_FAILURES: usize = 2;

#[derive(Default, Serialize)]
pub(super) struct ImpactEpisode {
    pub(super) failure_references: BTreeSet<String>,
    pub(super) tool_references: BTreeSet<String>,
    #[serde(skip)]
    tool_timestamps: BTreeMap<String, u64>,
    first_ms: Option<u64>,
    last_ms: Option<u64>,
}

impl ImpactEpisode {
    fn time(&mut self, timestamp: u64) {
        self.first_ms = Some(self.first_ms.map_or(timestamp, |old| old.min(timestamp)));
        self.last_ms = Some(self.last_ms.map_or(timestamp, |old| old.max(timestamp)));
    }

    fn exploration(&self, references: &[String]) -> bool {
        let times = references
            .iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|reference| self.tool_timestamps.get(reference))
            .copied()
            .collect::<Vec<_>>();
        times
            .iter()
            .max()
            .zip(times.iter().min())
            .is_some_and(|(last, first)| last.saturating_sub(*first) >= SUBSTANTIAL_EXPLORATION_MS)
            && times.len() >= MIN_TOOL_OBSERVATIONS
    }

    pub(super) fn allows(&self, role: Role, references: &[String]) -> bool {
        let repeated = self.failure_references.len() >= MIN_FAILURES
            && references
                .iter()
                .filter(|reference| self.failure_references.contains(*reference))
                .collect::<BTreeSet<_>>()
                .len()
                >= MIN_FAILURES;
        let exploration = role == Role::Repository
            && self.exploration(references)
            && references
                .iter()
                .filter(|reference| self.tool_references.contains(*reference))
                .collect::<BTreeSet<_>>()
                .len()
                >= MIN_TOOL_OBSERVATIONS;
        repeated || exploration
    }
}

#[derive(Default)]
pub(super) struct ImpactEvidence {
    pub(super) episodes: BTreeMap<String, ImpactEpisode>,
    pub(super) tools: Vec<ToolObservation>,
}

impl ImpactEvidence {
    pub(super) fn new<'a>(
        request: impl Into<MainRunFacts<'a>>,
        observations: Vec<ToolObservation>,
    ) -> Self {
        let request = request.into();
        let mut evidence = Self::default();
        for observation in observations.into_iter().take(512) {
            if !valid_observation(&request, &observation) {
                continue;
            }
            let episode = evidence
                .episodes
                .entry(observation.node_id.clone())
                .or_default();
            episode.time(observation.timestamp_ms);
            episode
                .tool_references
                .insert(format!("hook:{}", observation.seq));
            episode.tool_timestamps.insert(
                format!("hook:{}", observation.seq),
                observation.timestamp_ms,
            );
            evidence.tools.push(observation);
        }
        for node in request.snapshot.executions.values() {
            if !main_execution(&request, node.reference.execution) {
                continue;
            }
            if let Some(reference) = failure(node) {
                evidence
                    .episodes
                    .entry(node.reference.node.as_str().to_owned())
                    .or_default()
                    .failure_references
                    .insert(reference);
            }
        }
        // Imported checkpoint visits have no local completion references. Do not infer rounds
        // or absence of errors from a successor's partial execution history.
        if request.snapshot.execution_seed.is_empty() {
            let error_free = error_free_names(&request);
            evidence.review_repairs(&request, &request.admitted.graph.root, &error_free);
        }
        evidence
    }

    pub(super) fn references(&self) -> BTreeSet<String> {
        self.episodes
            .values()
            .flat_map(|episode| {
                episode
                    .failure_references
                    .iter()
                    .chain(&episode.tool_references)
            })
            .cloned()
            .collect()
    }

    fn review_repairs(
        &mut self,
        request: &MainRunFacts<'_>,
        node: &GraphNode,
        error_free: &BTreeSet<String>,
    ) {
        if let GraphNode::Loop(loop_node) = node {
            self.loop_repairs(request, loop_node, error_free);
        }
        for child in openengine_cluster_server::graph_verifier::graph_node_children(node) {
            self.review_repairs(request, child, error_free);
        }
    }

    fn loop_repairs(
        &mut self,
        request: &MainRunFacts<'_>,
        loop_node: &LoopNode,
        error_free: &BTreeSet<String>,
    ) {
        let Some(shape) = ReviewLoop::new(&loop_node.body, error_free) else {
            return;
        };
        let mut scopes = BTreeMap::<_, Vec<_>>::new();
        for node in request.snapshot.executions.values() {
            if shape.contains(node.reference.node.as_str()) {
                scopes
                    .entry(node.occurrence.map_indices.clone())
                    .or_default()
                    .push(node);
            }
        }
        for visits in scopes.values() {
            let Some(rounds) = shape.rounds(request, visits) else {
                continue;
            };
            let Some(rejected) = shape.rejections(loop_node, &rounds, error_free) else {
                continue;
            };
            if rejected.len() < MIN_FAILURES {
                continue;
            }
            for name in &shape.workers {
                self.episodes
                    .entry((*name).to_owned())
                    .or_default()
                    .failure_references
                    .extend(rejected.iter().cloned());
            }
        }
    }
}

/// Only a serial worker prefix followed by a flat, fully joined reviewer tail is supported.
/// Nested loops can overwrite intermediate signals; maps and arbitrary branches obscure which
/// worker participated. They are inspected independently by the recursive graph traversal.
struct ReviewLoop<'a> {
    workers: Vec<&'a str>,
    reviewers: BTreeSet<&'a str>,
}

impl<'a> ReviewLoop<'a> {
    fn new(body: &'a GraphNode, error_free: &BTreeSet<String>) -> Option<Self> {
        if !flat_body(body) {
            return None;
        }
        let GraphNode::Seq(sequence) = body else {
            return None;
        };
        let mut shape = Self {
            workers: Vec::new(),
            reviewers: BTreeSet::new(),
        };
        for child in sequence.children.as_slice() {
            if let GraphNode::Step(worker) = child {
                if !shape.reviewers.is_empty() {
                    return None;
                }
                shape.workers.push(worker.name.as_str());
            } else if !collect_reviewers(child, error_free, &mut shape.reviewers) {
                return None;
            }
        }
        (!shape.workers.is_empty() && !shape.reviewers.is_empty()).then_some(shape)
    }

    fn contains(&self, name: &str) -> bool {
        self.workers.contains(&name) || self.reviewers.contains(name)
    }

    fn visit<'b>(
        &self,
        request: &MainRunFacts<'_>,
        node: &'b NodeSnapshot,
    ) -> Option<ReviewVisit<'b>> {
        if !main_execution(request, node.reference.execution)
            || node.attempt.get() != 1
            || node.reference.node != node.occurrence.node
        {
            return None;
        }
        self.successful_leaf(node).then(|| ReviewVisit::new(node))?
    }

    fn successful_leaf(&self, node: &NodeSnapshot) -> bool {
        let name = node.reference.node.as_str();
        match node.outcome() {
            Some(WorkerOutcome::Verified { .. }) => self.workers.contains(&name),
            Some(WorkerOutcome::Verifier { .. }) => self.reviewers.contains(name),
            _ => false,
        }
    }

    fn rounds<'b>(
        &self,
        request: &MainRunFacts<'_>,
        visits: &[&'b NodeSnapshot],
    ) -> Option<Vec<Vec<ReviewVisit<'b>>>> {
        let mut visits = visits
            .iter()
            .map(|node| self.visit(request, node))
            .collect::<Option<Vec<_>>>()?;
        visits.sort_by_key(|visit| visit.start);
        let mut rounds: Vec<Vec<ReviewVisit<'b>>> = Vec::new();
        for visit in visits {
            if visit.node.reference.node.as_str() == self.workers[0] {
                rounds.push(Vec::new());
            }
            rounds.last_mut()?.push(visit);
        }
        if rounds.iter().any(|round| !self.complete_round(round)) {
            return None;
        }
        // Every leaf must settle before another worker prefix starts in this map item.
        if rounds
            .windows(2)
            .any(|pair| pair[0].iter().any(|visit| visit.finish >= pair[1][0].start))
        {
            return None;
        }
        Some(rounds)
    }

    fn complete_round(&self, visits: &[ReviewVisit<'_>]) -> bool {
        if visits.len() != self.workers.len() + self.reviewers.len() {
            return false;
        }
        let names = visits
            .iter()
            .map(|visit| visit.node.reference.node.as_str())
            .collect::<BTreeSet<_>>();
        names.len() == visits.len()
            && visits
                .iter()
                .zip(&self.workers)
                .all(|(visit, worker)| visit.node.reference.node.as_str() == *worker)
            && visits
                .windows(2)
                .take(self.workers.len())
                .all(|pair| pair[0].finish < pair[1].start)
    }

    fn rejections(
        &self,
        loop_node: &LoopNode,
        rounds: &[Vec<ReviewVisit<'_>>],
        error_free: &BTreeSet<String>,
    ) -> Option<BTreeSet<String>> {
        let until = loop_node.until.as_ref()?;
        let mut iteration = 1;
        let mut rejected = BTreeSet::new();
        for (index, round) in rounds.iter().enumerate() {
            let nodes = round.iter().map(|visit| visit.node).collect::<Vec<_>>();
            let converged = guard_value(until, &nodes, error_free)?;
            if !converged && iteration < loop_node.max_iterations.get() && index + 1 < rounds.len()
            {
                // A false error atom alone is not a rejected review. The false until must
                // depend on this verifier's own signal, with other signals left unknown.
                let anchor = nodes.iter().find(|node| {
                    self.reviewers.contains(node.reference.node.as_str())
                        && guard_value(until, &[], error_free).is_none()
                        && guard_value(until, &[*node], error_free) == Some(false)
                });
                if let Some(reference) = anchor.and_then(|node| completion_reference(node)) {
                    // One reference per round prevents simultaneous reviewers from counting
                    // as distinct repair cycles.
                    rejected.insert(reference);
                }
            }
            iteration = if converged || iteration == loop_node.max_iterations.get() {
                1
            } else {
                iteration + 1
            };
        }
        Some(rejected)
    }
}

struct ReviewVisit<'a> {
    node: &'a NodeSnapshot,
    start: u64,
    finish: u64,
}

impl<'a> ReviewVisit<'a> {
    fn new(node: &'a NodeSnapshot) -> Option<Self> {
        let NodeState::Completed { at, .. } = &node.state else {
            return None;
        };
        let start = cursor_sequence(&node.started_at).ok()?;
        let finish = cursor_sequence(at).ok()?;
        (start < finish).then_some(Self {
            node,
            start,
            finish,
        })
    }
}

pub(super) fn main_execution(request: &MainRunFacts<'_>, execution: ExecutionId) -> bool {
    request
        .snapshot
        .executions
        .get(&execution)
        .is_some_and(|node| {
            matches!(
                request.admitted.runtime.nodes().get(&node.reference.node),
                Some(NodeRuntimeBinding::Agent { .. })
            )
        })
}

fn valid_observation(request: &MainRunFacts<'_>, observation: &ToolObservation) -> bool {
    let Ok(execution) = ExecutionId::new(observation.execution_id) else {
        return false;
    };
    main_execution(request, execution)
        && request
            .snapshot
            .executions
            .get(&execution)
            .is_some_and(|node| node.reference.node.as_str() == observation.node_id)
        && observation.seq > 0
        && observation.timestamp_ms > 0
        && observation.tool_name.len() <= 128
        && observation.paths.len() <= 4
}

fn failure(node: &NodeSnapshot) -> Option<String> {
    match node.outcome()? {
        WorkerOutcome::Error {
            code: WorkerErrorCode::Timeout | WorkerErrorCode::Crash,
            ..
        } => completion_reference(node),
        _ => None,
    }
}

fn completion_reference(node: &NodeSnapshot) -> Option<String> {
    match &node.state {
        NodeState::Completed { at, .. } => Some(at.as_str().to_owned()),
        _ => None,
    }
}

/// A name is error-free only when every observed visit completed successfully. An unvisited,
/// active, voided or failed sibling remains unknown, including siblings in other map items.
fn error_free_names(request: &MainRunFacts<'_>) -> BTreeSet<String> {
    let mut names = BTreeMap::new();
    for node in request.snapshot.executions.values() {
        if main_execution(request, node.reference.execution) {
            let successful = matches!(
                node.outcome(),
                Some(WorkerOutcome::Verified { .. } | WorkerOutcome::Verifier { .. })
            );
            *names
                .entry(node.reference.node.as_str().to_owned())
                .or_insert(true) &= successful;
        }
    }
    names
        .into_iter()
        .filter_map(|(name, successful)| successful.then_some(name))
        .collect()
}

fn flat_body(node: &GraphNode) -> bool {
    !matches!(node, GraphNode::Loop(_) | GraphNode::Map(_))
        && openengine_cluster_server::graph_verifier::graph_node_children(node)
            .into_iter()
            .all(flat_body)
}

fn collect_reviewers<'a>(
    node: &'a GraphNode,
    error_free: &BTreeSet<String>,
    names: &mut BTreeSet<&'a str>,
) -> bool {
    match node {
        GraphNode::Verifier(verifier) => names.insert(verifier.name.as_str()),
        GraphNode::Seq(sequence) => sequence
            .children
            .as_slice()
            .iter()
            .all(|child| collect_reviewers(child, error_free, names)),
        GraphNode::Par(parallel) if matches!(parallel.join, Join::All {}) => parallel
            .branches
            .as_slice()
            .iter()
            .all(|child| collect_reviewers(child, error_free, names)),
        // Error-exit wrappers are traversable only when every preceding guard is known
        // false independently of reviewer signals. No absent sibling outcome is invented.
        GraphNode::Choice(choice) => {
            choice
                .branches
                .as_slice()
                .iter()
                .all(|branch| guard_value(&branch.when, &[], error_free) == Some(false))
                && choice
                    .otherwise
                    .as_deref()
                    .is_some_and(|child| collect_reviewers(child, error_free, names))
        }
        _ => false,
    }
}

/// Evaluate only supplied verifier signals and globally proved absent errors. All other controls
/// abstain; three-valued composition preserves a decisive known result without filling siblings.
fn guard_value(
    guard: &Guard,
    nodes: &[&NodeSnapshot],
    error_free: &BTreeSet<String>,
) -> Option<bool> {
    match guard {
        Guard::In { .. } => control_guard(guard, nodes, error_free),
        Guard::All { guards } => combine(
            guards
                .as_slice()
                .iter()
                .map(|guard| guard_value(guard, nodes, error_free)),
            false,
        ),
        Guard::Any { guards } => combine(
            guards
                .as_slice()
                .iter()
                .map(|guard| guard_value(guard, nodes, error_free)),
            true,
        ),
        Guard::Not { guard } => guard_value(guard, nodes, error_free).map(|value| !value),
        _ => None,
    }
}

fn control_guard(
    guard: &Guard,
    nodes: &[&NodeSnapshot],
    error_free: &BTreeSet<String>,
) -> Option<bool> {
    let Guard::In { value, .. } = guard else {
        return None;
    };
    match value.source {
        ControlSource::Error if error_free.contains(value.name.as_str()) => Some(false),
        ControlSource::Signal => signal_guard(guard, nodes),
        _ => None,
    }
}

fn signal_guard(guard: &Guard, nodes: &[&NodeSnapshot]) -> Option<bool> {
    let Guard::In { value, labels } = guard else {
        return None;
    };
    let node = nodes
        .iter()
        .find(|node| node.reference.node == value.name)?;
    let WorkerOutcome::Verifier { signals, .. } = node.outcome()? else {
        return None;
    };
    Some(
        labels
            .values()
            .contains(signals.get(value.field.as_ref()?)?),
    )
}

fn combine(values: impl Iterator<Item = Option<bool>>, decisive: bool) -> Option<bool> {
    let values = values.collect::<Vec<_>>();
    if values.contains(&Some(decisive)) {
        return Some(decisive);
    }
    if values.contains(&None) {
        return None;
    }
    Some(!decisive)
}

#[cfg(test)]
#[path = "tests/impact.rs"]
mod tests;
