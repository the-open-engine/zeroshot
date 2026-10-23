use std::collections::{BTreeSet, VecDeque};

use serde_json::{Value, json};

use super::{MainRunFacts, PostmortemError};
use super::impact::{ImpactEvidence, main_execution};
use crate::active_memory::ToolObservation;
use crate::native_v2_runner::bounded_log_text;
use crate::v2_run_ledger::{RunEvent, StoredRunEvent, cursor_for, cursor_sequence};

const REPLAY_WINDOW: u64 = 4096;
const LOG_BUDGET: usize = 32 * 1024;

pub(super) struct FrozenEvidence {
    pub(super) document: Value,
    pub(super) references: BTreeSet<String>,
    pub(super) impact: ImpactEvidence,
}

#[derive(Default)]
struct EvidenceTail {
    records: VecDeque<(String, String)>,
    bytes: usize,
    truncated: bool,
}

impl EvidenceTail {
    fn push(
        &mut self,
        request: &MainRunFacts<'_>,
        stored: StoredRunEvent,
    ) -> Result<(), PostmortemError> {
        if !main_event(request, &stored.event) {
            return Ok(());
        }
        let text = serde_json::to_string(&stored.event).map_err(|_| PostmortemError::Storage)?;
        self.truncated |= text.len() > 1024;
        let text = bounded_log_text(text, 1024);
        self.bytes += text.len();
        self.records
            .push_back((stored.cursor.as_str().to_owned(), text));
        while self.bytes > LOG_BUDGET {
            if let Some((_, text)) = self.records.pop_front() {
                self.bytes = self.bytes.saturating_sub(text.len());
                self.truncated = true;
            }
        }
        Ok(())
    }
}

pub(super) async fn collect<'a>(
    request: impl Into<MainRunFacts<'a>>,
    observations: Vec<ToolObservation>,
) -> Result<FrozenEvidence, PostmortemError> {
    let request = request.into();
    let stop = cursor_sequence(&request.snapshot.cursor).map_err(|_| PostmortemError::Storage)?;
    let start = stop.saturating_sub(REPLAY_WINDOW);
    let mut cursor = cursor_for(start);
    let mut tail = EvidenceTail {
        truncated: start > 0,
        ..Default::default()
    };
    loop {
        let page = request
            .ledger
            .snapshot_and_tail(request.run_id, Some(&cursor))
            .await
            .map_err(|_| PostmortemError::Storage)?;
        if page.events.is_empty() {
            break;
        }
        if let Some(next) = append_page(&request, &mut tail, page.events, stop)? {
            cursor = next;
        } else {
            break;
        }
        if cursor_sequence(&cursor).map_err(|_| PostmortemError::Storage)? >= stop {
            break;
        }
    }
    Ok(render(
        &request,
        tail,
        ImpactEvidence::new(request, observations),
    ))
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
        tail.push(request, stored)?;
    }
    Ok(cursor)
}

fn main_event(request: &MainRunFacts<'_>, event: &RunEvent) -> bool {
    match event {
        RunEvent::SafeLog {
            execution: Some(execution),
            ..
        } => main_execution(request, *execution),
        RunEvent::NodeCompleted { completion } => {
            main_execution(request, completion.reference.execution)
        }
        _ => false,
    }
}

fn render(
    request: &MainRunFacts<'_>,
    mut tail: EvidenceTail,
    impact: ImpactEvidence,
) -> FrozenEvidence {
    let mut references = tail
        .records
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
        "executions":executions, "records":tail.records.into_iter().map(|(reference,text)|json!({"reference":reference,"event":text})).collect::<Vec<_>>(),
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
