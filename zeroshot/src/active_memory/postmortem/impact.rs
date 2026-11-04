//! Observed main-run evidence; specialists assess significance and causal attribution.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::MainRunFacts;
use crate::active_memory::ToolObservation;
use crate::native_v2_contract::{ExecutionId, NodeRuntimeBinding};
use crate::v2_run_ledger::{NodeSnapshot, NodeState};

#[derive(Default, Serialize)]
pub(super) struct ImpactEpisode {
    pub(super) completion_references: BTreeSet<String>,
    pub(super) tool_references: BTreeSet<String>,
}

impl ImpactEpisode {
    pub(super) fn allows(&self, references: &[String]) -> bool {
        references.iter().any(|reference| {
            self.completion_references.contains(reference)
                || self.tool_references.contains(reference)
        })
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
            evidence
                .episodes
                .entry(observation.node_id.clone())
                .or_default()
                .tool_references
                .insert(format!("hook:{}", observation.seq));
            evidence.tools.push(observation);
        }
        for node in request.snapshot.executions.values() {
            if main_execution(&request, node.reference.execution)
                && let Some(reference) = completion_reference(node)
            {
                evidence
                    .episodes
                    .entry(node.reference.node.as_str().to_owned())
                    .or_default()
                    .completion_references
                    .insert(reference);
            }
        }
        evidence
    }

    pub(super) fn references(&self) -> BTreeSet<String> {
        self.episodes
            .values()
            .flat_map(|episode| {
                episode
                    .completion_references
                    .iter()
                    .chain(&episode.tool_references)
            })
            .cloned()
            .collect()
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

fn completion_reference(node: &NodeSnapshot) -> Option<String> {
    match &node.state {
        NodeState::Completed { at, .. } => Some(at.as_str().to_owned()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "tests/impact.rs"]
mod tests;
