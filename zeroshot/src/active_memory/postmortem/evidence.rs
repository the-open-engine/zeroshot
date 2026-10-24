use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::{Value, json};

use super::{MainRunFacts, PostmortemError};
use super::impact::{ImpactEvidence, main_execution};
use crate::active_memory::ToolObservation;
use crate::native_v2_runner::bounded_log_text;
use crate::v2_run_ledger::{RunEvent, StoredRunEvent, cursor_for, cursor_sequence};

const LOG_BUDGET: usize = 32 * 1024;

#[cfg(test)]
mod tests;

pub(super) struct FrozenEvidence {
    pub(super) document: Value,
    pub(super) references: BTreeSet<String>,
    pub(super) impact: ImpactEvidence,
}

#[derive(Default)]
struct EvidenceTail {
    executions: BTreeMap<crate::native_v2_contract::ExecutionId, ExecutionTail>,
    bytes: usize,
    truncated: bool,
}

#[derive(Default)]
struct ExecutionTail {
    records: VecDeque<(u64, String)>,
    bytes: usize,
}

impl EvidenceTail {
    fn push(
        &mut self,
        execution: crate::native_v2_contract::ExecutionId,
        stored: StoredRunEvent,
    ) -> Result<(), PostmortemError> {
        let sequence = cursor_sequence(&stored.cursor).map_err(|_| PostmortemError::Storage)?;
        let text = serde_json::to_string(&stored.event).map_err(|_| PostmortemError::Storage)?;
        self.truncated |= text.len() > 1024;
        let text = bounded_log_text(text, 1024);
        self.bytes += text.len();
        let tail = self.executions.entry(execution).or_default();
        tail.bytes += text.len();
        tail.records.push_back((sequence, text));
        // A busy late reviewer must not evict every earlier worker record.
        // Reclaim from the largest consumer, retaining each execution's most recent evidence.
        while self.bytes > LOG_BUDGET {
            let Some((&largest, _)) = self.executions.iter().max_by_key(|(_, tail)| tail.bytes)
            else {
                break;
            };
            let tail = self
                .executions
                .get_mut(&largest)
                .ok_or(PostmortemError::Storage)?;
            if let Some((_, text)) = tail.records.pop_front() {
                tail.bytes -= text.len();
                self.bytes -= text.len();
                self.truncated = true;
            }
            if tail.records.is_empty() {
                self.executions.remove(&largest);
            }
        }
        Ok(())
    }

    fn records(&self) -> Vec<(String, String)> {
        let mut records = self
            .executions
            .values()
            .flat_map(|tail| tail.records.iter())
            .collect::<Vec<_>>();
        records.sort_by_key(|(sequence, _)| *sequence);
        records
            .into_iter()
            .map(|(sequence, text)| (cursor_for(*sequence).as_str().to_owned(), text.clone()))
            .collect()
    }
}

pub(super) async fn collect<'a>(
    request: impl Into<MainRunFacts<'a>>,
    observations: Vec<ToolObservation>,
) -> Result<FrozenEvidence, PostmortemError> {
    collect_inner(request.into(), observations, None)
        .await?
        .ok_or(PostmortemError::Preparation)
}

pub(super) async fn collect_cancellable(
    request: &super::PostmortemRequest,
    observations: Vec<ToolObservation>,
    cancellation: &tokio::sync::watch::Receiver<bool>,
) -> Result<Option<FrozenEvidence>, PostmortemError> {
    collect_inner(request.into(), observations, Some(cancellation)).await
}

fn collection_cancelled(cancellation: Option<&tokio::sync::watch::Receiver<bool>>) -> bool {
    cancellation.is_some_and(|receiver| *receiver.borrow() || receiver.has_changed().is_err())
}

async fn collect_inner(
    request: MainRunFacts<'_>,
    observations: Vec<ToolObservation>,
    cancellation: Option<&tokio::sync::watch::Receiver<bool>>,
) -> Result<Option<FrozenEvidence>, PostmortemError> {
    let stop = cursor_sequence(&request.snapshot.cursor).map_err(|_| PostmortemError::Storage)?;
    // Replay in bounded pages from the beginning: a global last-N window can contain
    // only reviewer deltas and omit the worker before byte budgeting even begins.
    let mut cursor = cursor_for(0);
    let mut tail = EvidenceTail::default();
    while cursor_sequence(&cursor).map_err(|_| PostmortemError::Storage)? < stop {
        // Drain at most the current bounded read; never detach a blocking SQLite operation.
        if collection_cancelled(cancellation) {
            return Ok(None);
        }
        let page = request
            .ledger
            .snapshot_and_tail(request.run_id, Some(&cursor))
            .await
            .map_err(|_| PostmortemError::Storage)?;
        let Some(next) = append_page(&request, &mut tail, page.events, stop)? else {
            break;
        };
        cursor = next;
    }
    if collection_cancelled(cancellation) {
        return Ok(None);
    }
    Ok(Some(render(
        &request,
        tail,
        ImpactEvidence::new(request, observations),
    )))
}

fn append_page(
    request: &MainRunFacts<'_>,
    tail: &mut EvidenceTail,
    events: Vec<StoredRunEvent>,
    stop: u64,
) -> Result<Option<openengine_cluster_protocol::Cursor>, PostmortemError> {
    let mut cursor = None;
    for stored in events {
        if cursor_sequence(&stored.cursor).map_err(|_| PostmortemError::Storage)? > stop {
            break;
        }
        cursor = Some(stored.cursor.clone());
        if let Some(execution) = main_event(request, &stored.event) {
            tail.push(execution, stored)?;
        }
    }
    Ok(cursor)
}

fn main_event(
    request: &MainRunFacts<'_>,
    event: &RunEvent,
) -> Option<crate::native_v2_contract::ExecutionId> {
    match event {
        RunEvent::SafeLog {
            execution: Some(execution),
            ..
        } => main_execution(request, *execution).then_some(*execution),
        RunEvent::NodeCompleted { completion } => {
            main_execution(request, completion.reference.execution)
                .then_some(completion.reference.execution)
        }
        _ => None,
    }
}

fn render(
    request: &MainRunFacts<'_>,
    mut tail: EvidenceTail,
    impact: ImpactEvidence,
) -> FrozenEvidence {
    let execution_by_reference = tail
        .executions
        .iter()
        .flat_map(|(execution, tail)| {
            tail.records
                .iter()
                .map(move |(sequence, _)| (cursor_for(*sequence).as_str().to_owned(), *execution))
        })
        .collect::<BTreeMap<_, _>>();
    let records = tail.records();
    let mut references = records
        .iter()
        .map(|(reference, _)| reference.clone())
        .collect::<BTreeSet<_>>();
    references.extend(impact.references());
    let executions = request
        .snapshot
        .executions
        .values()
        .rev()
        .filter(|node| main_execution(request, node.reference.execution))
        .take(128)
        .map(|node| {
            references.insert(format!("node:{}", node.reference.node.as_str()));
            json!({"node":node.reference.node,"execution":node.reference.execution,
            "state":bounded_json(&node.state,1024),"input":bounded_json(&node.input,512)})
        })
        .collect::<Vec<_>>();
    tail.truncated |= request.snapshot.executions.len() > 128;
    let document = json!({
        "run_id":request.run_id, "title":request.admitted.title,
        "primary_terminal":request.primary_terminal, "initial_input":bounded_json(&request.admitted.initial_input,4096),
        "authored_graph":bounded_json(&request.admitted.graph,16384),
        "impact_episodes":impact.episodes,"tool_observations":impact.tools,
        "impact_policy":{"minimum_failures_or_rejected_repairs":2,"minimum_exploration_ms":300000,
            "minimum_distinct_tool_observations":3,"exploration_eligible_for":"repository",
            "ordinary_output_corrections_count_as_failures":false},
        "executions":executions, "records":records.into_iter().map(|(reference,text)|json!({"execution":execution_by_reference.get(&reference),"reference":reference,"event":text})).collect::<Vec<_>>(),
        "evidence_truncated":tail.truncated,
    });
    FrozenEvidence {
        document,
        references,
        impact,
    }
}

fn bounded_json(value: &impl serde::Serialize, budget: usize) -> String {
    bounded_log_text(
        serde_json::to_string(value).unwrap_or_else(|_| "[unavailable]".to_owned()),
        budget,
    )
}
