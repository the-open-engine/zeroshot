//! Derived control activity stays beside native events. Every update names the exact durable
//! prefix used by the canonical reducer; it is never written back as a fabricated ledger event.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::Range;
use std::sync::Arc;

use openengine_cluster_server::admission::VerifiedGraph;
use tokio::sync::Mutex;

use super::*;
use serde::{Deserialize, Serialize};
use crate::v2_run_ledger::StoredRun;
use crate::full_v1_reducer::{FullV1Reducer, ReductionInput, StructuralTrace, StructuralTraceState};
use crate::native_v2_contract::AdmittedRun;
use crate::native_v2_supervisor::{durable_history, next_execution, next_node_instance};
use crate::v2_run_ledger::{apply_event, RunEvent, RunPhase, StoredRunEvent};

const MAX_CACHED_RUNS: usize = 4;
const MAX_REPLAY_EVENTS: usize = 100_000;
const MAX_REPLAY_BYTES: usize = 64 * 1024 * 1024;
const MAX_LIFECYCLE_REDUCTIONS: usize = 2_048;
const MAX_REDUCER_EVALUATIONS: usize = 2_000_000;
const MAX_CONTROL_RECORDS: usize = 16_384;
const MAX_CONTROL_BYTES: usize = 4 * 1024 * 1024;
const PROJECTION_FAILED: &str = "control_projection_unavailable";
const PROJECTION_LIMIT: &str = "control_projection_limit";

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlRecord {
    pub cursor: Cursor,
    pub node: String,
    pub map_indices: Vec<u64>,
    pub visit_id: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(
        default,
        deserialize_with = "present_output",
        skip_serializing_if = "Option::is_none"
    )]
    pub output: Option<Value>,
}

// An explicit null output is authored data; an absent output means this visit has no output.
fn present_output<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

impl ControlRecord {
    fn from_trace(trace: StructuralTrace, cursor: &Cursor) -> Result<Self, &'static str> {
        let visit_id =
            serde_json::to_string(&(&trace.node, &trace.map_indices, &trace.loop_iterations))
                .map_err(|_| PROJECTION_FAILED)?;
        Ok(Self {
            cursor: cursor.clone(),
            node: trace.node.as_str().to_owned(),
            map_indices: trace.map_indices,
            visit_id,
            state: match trace.state {
                StructuralTraceState::Entered => "entered",
                StructuralTraceState::Completed => "completed",
                StructuralTraceState::Succeeded => "succeeded",
                StructuralTraceState::Failed => "failed",
            }
            .into(),
            branch: trace.branch.map(|name| name.as_str().to_owned()),
            detail: trace.detail,
            output: trace.output,
        })
    }

    fn same_value(&self, other: &Self) -> bool {
        self.node == other.node
            && self.map_indices == other.map_indices
            && self.visit_id == other.visit_id
            && self.state == other.state
            && self.branch == other.branch
            && self.detail == other.detail
            && self.output == other.output
    }
}

pub(crate) struct ControlPage {
    pub records: Vec<ControlRecord>,
    pub error: Option<String>,
}

type SharedReplay = Arc<Mutex<Option<ControlReplay>>>;

/// A small shared cache makes ordinary pagination, live delivery and reconnects incremental.
/// Eviction changes performance only: replaying the immutable prefix produces identical updates.
#[derive(Clone, Default)]
pub(crate) struct ControlCache {
    runs: Arc<Mutex<VecDeque<(RunId, SharedReplay)>>>,
}

impl ControlCache {
    async fn entry(&self, id: &RunId) -> SharedReplay {
        let mut runs = self.runs.lock().await;
        if let Some(index) = runs.iter().position(|(existing, _)| existing == id) {
            let entry = runs.remove(index).expect("the matching cache entry exists");
            let replay = entry.1.clone();
            runs.push_back(entry);
            return replay;
        }
        let replay = Arc::new(Mutex::new(None));
        if runs.len() >= MAX_CACHED_RUNS {
            runs.pop_front();
        }
        runs.push_back((id.clone(), replay.clone()));
        replay
    }

    pub(crate) async fn project(
        &self,
        ledger: &dyn RunLedger,
        id: &RunId,
        range: Range<u64>,
    ) -> ControlPage {
        match self.project_inner(ledger, id, range).await {
            Ok(page) => page,
            Err(error) => ControlPage {
                records: Vec::new(),
                error: Some(error.to_owned()),
            },
        }
    }

    async fn project_inner(
        &self,
        ledger: &dyn RunLedger,
        id: &RunId,
        range: Range<u64>,
    ) -> Result<ControlPage, &'static str> {
        let entry = self.entry(id).await;
        let mut cached = entry.lock().await;
        let mut replay = match cached.take() {
            Some(replay) => replay,
            None => ControlReplay::new(
                ledger
                    .get(id)
                    .await
                    .map_err(|_| PROJECTION_FAILED)?
                    .ok_or(PROJECTION_FAILED)?,
            ),
        };
        while replay.sequence < range.end && replay.error.is_none() {
            let page = ledger
                .snapshot_and_tail(id, Some(&replay.snapshot.cursor))
                .await
                .map_err(|_| PROJECTION_FAILED)?;
            let events = page
                .events
                .into_iter()
                .take_while(|event| {
                    cursor_sequence(&event.cursor).is_ok_and(|sequence| sequence <= range.end)
                })
                .collect::<Vec<_>>();
            if events.is_empty() {
                replay.fail(PROJECTION_FAILED);
                break;
            }
            // Pure graph work never occupies an async reactor thread. The cache lock retains
            // ownership across this job; cancellation safely leaves a reconstructible empty entry.
            replay = tokio::task::spawn_blocking(move || {
                replay.advance(events);
                replay
            })
            .await
            .map_err(|_| PROJECTION_FAILED)?;
        }
        let page = replay.page(range);
        *cached = Some(replay);
        Ok(page)
    }
}

struct ControlReplay {
    admitted: AdmittedRun,
    snapshot: RunSnapshot,
    sequence: u64,
    current: BTreeMap<String, ControlRecord>,
    updates: Vec<(u64, ControlRecord)>,
    error: Option<(u64, &'static str)>,
    scanned: usize,
    source_bytes: usize,
    reductions: usize,
    control_bytes: usize,
    evaluations: usize,
}

impl ControlReplay {
    fn new(stored: StoredRun) -> Self {
        Self {
            admitted: stored.admitted,
            snapshot: stored.snapshot.replay_seed(),
            sequence: 0,
            current: BTreeMap::new(),
            updates: Vec::new(),
            error: None,
            scanned: 0,
            source_bytes: 0,
            reductions: 0,
            control_bytes: 0,
            evaluations: 0,
        }
    }

    fn fail(&mut self, reason: &'static str) {
        self.error = Some((self.sequence, reason));
    }

    fn advance(&mut self, events: Vec<StoredRunEvent>) {
        for stored in events {
            if let Err(error) = self.apply(stored) {
                self.fail(error);
                break;
            }
        }
    }

    fn apply(&mut self, stored: StoredRunEvent) -> Result<(), &'static str> {
        let sequence = cursor_sequence(&stored.cursor).map_err(|_| PROJECTION_FAILED)?;
        if sequence != self.sequence + 1 {
            return Err(PROJECTION_FAILED);
        }
        self.sequence = sequence;
        self.scanned += 1;
        self.source_bytes = self.source_bytes.saturating_add(
            serde_json::to_vec(&stored)
                .map_err(|_| PROJECTION_FAILED)?
                .len(),
        );
        if self.scanned > MAX_REPLAY_EVENTS || self.source_bytes > MAX_REPLAY_BYTES {
            return Err(PROJECTION_LIMIT);
        }
        apply_event(&mut self.snapshot, &stored.event, sequence).map_err(|_| PROJECTION_FAILED)?;
        self.project_event(stored.event)
    }

    fn project_event(&mut self, event: RunEvent) -> Result<(), &'static str> {
        match event {
            RunEvent::SafeLog { .. } | RunEvent::TokenUsageObserved { .. } => Ok(()),
            RunEvent::ForceStopRequested | RunEvent::Terminal { .. } => self.stop_pending(None),
            _ if self.snapshot.force_stop_requested => Ok(()),
            _ if self.snapshot.phase == RunPhase::Running => self.reduce(),
            _ => Ok(()),
        }
    }

    fn reduce(&mut self) -> Result<(), &'static str> {
        self.reductions += 1;
        if self.reductions > MAX_LIFECYCLE_REDUCTIONS {
            return Err(PROJECTION_LIMIT);
        }
        let executions = durable_history(&self.snapshot).map_err(|_| PROJECTION_FAILED)?;
        let verified = VerifiedGraph {
            compiled_ir: self.admitted.graph.clone(),
            diagnostics: Vec::new(),
        };
        let traced = FullV1Reducer::native_v2(&verified)
            .reduce_with_trace(ReductionInput {
                initial_input: &self.admitted.initial_input,
                executions: &executions,
                next_node_instance: next_node_instance(&executions)
                    .map_err(|_| PROJECTION_FAILED)?,
                next_execution: next_execution(&executions).map_err(|_| PROJECTION_FAILED)?,
            })
            .map_err(|error| match error {
                crate::full_v1_reducer::ReducerError::TraceLimit => PROJECTION_LIMIT,
                _ => PROJECTION_FAILED,
            })?;
        self.evaluations = self.evaluations.saturating_add(traced.evaluations);
        if self.evaluations > MAX_REDUCER_EVALUATIONS {
            return Err(PROJECTION_LIMIT);
        }
        let mut present = BTreeSet::new();
        // All traversal records at this prefix are derived simultaneously. Publish only the
        // final state of each visit; probes cannot add duplicate timeline stops.
        for trace in traced.trace {
            let record = ControlRecord::from_trace(trace, &self.snapshot.cursor)?;
            present.insert(record.visit_id.clone());
            self.record(record)?;
        }
        self.stop_pending(Some(&present))
    }

    fn record(&mut self, record: ControlRecord) -> Result<(), &'static str> {
        if self
            .current
            .get(&record.visit_id)
            .is_some_and(|existing| existing.same_value(&record))
        {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&record)
            .map_err(|_| PROJECTION_FAILED)?
            .len();
        if self.updates.len() >= MAX_CONTROL_RECORDS
            || self.control_bytes.saturating_add(bytes) > MAX_CONTROL_BYTES
        {
            return Err(PROJECTION_LIMIT);
        }
        self.control_bytes += bytes;
        self.current.insert(record.visit_id.clone(), record.clone());
        self.updates.push((self.sequence, record));
        Ok(())
    }

    fn stop_pending(&mut self, present: Option<&BTreeSet<String>>) -> Result<(), &'static str> {
        let stopped = self
            .current
            .values()
            .filter(|record| {
                record.state == "entered"
                    && present.is_none_or(|ids| !ids.contains(&record.visit_id))
            })
            .cloned()
            .collect::<Vec<_>>();
        for mut record in stopped {
            record.cursor = self.snapshot.cursor.clone();
            record.state = "stopped".into();
            self.record(record)?;
        }
        Ok(())
    }

    fn page(&self, range: Range<u64>) -> ControlPage {
        ControlPage {
            records: self
                .updates
                .iter()
                .filter(|(sequence, _)| *sequence > range.start && *sequence <= range.end)
                .map(|(_, record)| record.clone())
                .collect(),
            error: self
                .error
                .filter(|(at, _)| *at <= range.end)
                .map(|(_, reason)| reason.to_owned()),
        }
    }
}
