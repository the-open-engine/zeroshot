//! Credential-free native history projection over an already-owned ledger.
//! Reading this service never starts, initializes, or recovers a controller.
use std::sync::Arc;

use openengine_cluster_protocol::{is_canonical_uuid_v7, Cursor, GraphSpec, RunId};
use serde_json::{json, Value};

use crate::v2_run_ledger::{cursor_sequence, initial_cursor, RunLedger, RunLedgerError, RunSnapshot};

pub(crate) mod control;
pub(crate) mod status;
mod wire;
pub use control::ControlRecord;
pub use status::RuntimeFailure;
pub use wire::{HistoryMetadata, HistoryPage, Observation, ObservationState, RunDefinition};
use control::ControlCache;
use status::RuntimeStatusReader;

/// Safe HTTP-compatible failure, shared by native and host adapters.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct HistoryError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

/// Native projection owned by a host that already owns its ledger and observations.
#[derive(Clone)]
pub struct RunHistoryService {
    ledger: Arc<dyn RunLedger>,
    control: ControlCache,
    status: Option<RuntimeStatusReader>,
}

impl RunHistoryService {
    pub(crate) fn with_sources(
        ledger: Arc<dyn RunLedger>,
        control: ControlCache,
        status: Option<RuntimeStatusReader>,
    ) -> Self {
        Self {
            ledger,
            control,
            status,
        }
    }

    /// Returns the admitted definition, native snapshot and separate observation availability.
    pub async fn definition(&self, id: &RunId) -> Result<RunDefinition, HistoryError> {
        validate_run_id(id)?;
        let stored = self
            .ledger
            .get(id)
            .await
            .map_err(ledger_error)?
            .ok_or_else(not_found)?;
        let runtime = status::runtime_observation(self.status.as_ref(), &stored.snapshot).await;
        let observation = history_observation(&stored.snapshot, runtime.is_ok());
        let runtime_failure = runtime.ok().flatten();
        let graph = GraphSpec {
            profile: stored.admitted.graph.profile,
            initial_input: stored.admitted.graph.initial_input,
            policy: stored.admitted.graph.policy,
            root: stored.admitted.graph.root,
        };
        let mut snapshot = serde_json::to_value(&stored.snapshot).map_err(|_| unavailable())?;
        if let Some(executions) = snapshot["executions"].as_object_mut() {
            for node in executions.values_mut() {
                stringify_reference(&mut node["reference"]);
            }
        }
        let mut value = json!({
            "version":1, "projectionVersion":1,
            "runId":id, "title":stored.admitted.title, "createdAt":created_at(id),
            "phase":stored.snapshot.phase,"cursor":stored.snapshot.cursor,
            "terminal":stored.snapshot.terminal,"historyAvailable":true,
            "graph":graph, "runtime":stored.admitted.runtime,
            "initialInput":stored.admitted.initial_input,"source":stored.admitted.source,
            "snapshot":snapshot, "observation":observation,
            "history":{
                "initialCursor":initial_cursor(),"cursor":stored.snapshot.cursor,
                "complete":stored.snapshot.terminal.is_some(),
                "limitations":["control_flow_projected_by_reducer","retained_provider_output_only"]
            }
        });
        if let Some(failure) = runtime_failure {
            failure.apply(&mut value);
        }
        serde_json::from_value(value).map_err(|_| unavailable())
    }

    /// Returns one bounded native page. Execution cursors never use host status cursors.
    pub async fn page(
        &self,
        id: &RunId,
        after: Option<Cursor>,
    ) -> Result<HistoryPage, HistoryError> {
        validate_run_id(id)?;
        read_page(
            self.ledger.as_ref(),
            id,
            after.unwrap_or_else(initial_cursor),
            self,
        )
        .await
    }
}

fn history_observation(snapshot: &RunSnapshot, runtime_available: bool) -> Observation {
    if snapshot.terminal.is_some() {
        Observation {
            state: ObservationState::Complete,
            code: None,
        }
    } else if runtime_available {
        Observation {
            state: ObservationState::Active,
            code: None,
        }
    } else {
        Observation {
            state: ObservationState::Incomplete,
            code: Some("runtime_unavailable".into()),
        }
    }
}

pub(crate) fn validate_run_id(id: &RunId) -> Result<(), HistoryError> {
    if is_canonical_uuid_v7(id) {
        Ok(())
    } else {
        Err(not_found())
    }
}

async fn read_page(
    ledger: &dyn RunLedger,
    id: &RunId,
    after: Cursor,
    source: &RunHistoryService,
) -> Result<HistoryPage, HistoryError> {
    let start = cursor_sequence(&after).map_err(ledger_error)?;
    let observation = ledger
        .snapshot_and_tail(id, Some(&after))
        .await
        .map_err(ledger_error)?;
    let head = cursor_sequence(&observation.snapshot.cursor).map_err(ledger_error)?;
    let (events, next, sequence) = project_page(observation.events, after, head)?;
    let controls = source.control.project(ledger, id, start..sequence).await;
    let runtime = status::runtime_observation(source.status.as_ref(), &observation.snapshot).await;
    let runtime_available = runtime.is_ok();
    let runtime_failure = runtime.ok().flatten();
    let finished = observation.snapshot.terminal.is_some() || runtime_failure.is_some();
    let availability = history_observation(&observation.snapshot, runtime_available);
    Ok(HistoryPage {
        events,
        next_cursor: next,
        head_cursor: observation.snapshot.cursor,
        complete: sequence == head,
        finished,
        runtime_failure,
        observation: availability,
        control: controls.records,
        control_error: controls.error,
    })
}

fn project_page(
    source: Vec<crate::v2_run_ledger::StoredRunEvent>,
    after: Cursor,
    head: u64,
) -> Result<(Vec<Value>, Cursor, u64), HistoryError> {
    let mut next = after;
    let mut sequence = cursor_sequence(&next).map_err(ledger_error)?;
    let mut events = Vec::with_capacity(source.len());
    for stored in source {
        let at = cursor_sequence(&stored.cursor).map_err(ledger_error)?;
        // Snapshot and event reads may race an append. Pin this page to its observed head.
        if at > head {
            break;
        }
        if at != sequence + 1 {
            return Err(history_gap());
        }
        sequence = at;
        next = stored.cursor.clone();
        let mut value = serde_json::to_value(stored).map_err(|_| unavailable())?;
        project_event(&mut value["event"]);
        events.push(value);
    }
    if events.is_empty() && sequence < head {
        return Err(history_gap());
    }
    Ok((events, next, sequence))
}

pub(crate) fn created_at(id: &RunId) -> Option<u64> {
    let (seconds, nanos) = uuid::Uuid::parse_str(id.as_str())
        .ok()?
        .get_timestamp()?
        .to_unix();
    seconds
        .checked_mul(1000)?
        .checked_add(u64::from(nanos / 1_000_000))
}

// Execution identifiers are opaque strings in the browser. Native u64 values must never pass
// through a JavaScript number, even though current short runs usually have small identities.
fn stringify_reference(reference: &mut Value) {
    for key in ["execution", "nodeInstance"] {
        if let Some(id) = reference[key].as_u64() {
            reference[key] = Value::String(id.to_string());
        }
    }
}

fn project_event(event: &mut Value) {
    match event["kind"].as_str() {
        Some("node_started" | "execution_voided") => stringify_reference(&mut event["reference"]),
        Some("node_completed") => stringify_reference(&mut event["completion"]["reference"]),
        Some("safe_log" | "token_usage_observed") => {
            if let Some(id) = event["execution"].as_u64() {
                event["execution"] = Value::String(id.to_string());
            }
        }
        _ => {}
    }
}

pub(crate) fn ledger_error(error: RunLedgerError) -> HistoryError {
    match error {
        RunLedgerError::RunNotFound => not_found(),
        RunLedgerError::InvalidCursor | RunLedgerError::CursorAhead => invalid_cursor(),
        _ => unavailable(),
    }
}

pub(crate) fn not_found() -> HistoryError {
    HistoryError {
        status: 404,
        code: "run_not_found",
        message: "This run has no retained history.".into(),
    }
}
pub(crate) fn unavailable() -> HistoryError {
    HistoryError {
        status: 503,
        code: "history_unavailable",
        message: "This run's history is unavailable or uses an unsupported format.".into(),
    }
}
#[cfg(feature = "ui")]
pub(crate) fn runtime_unavailable() -> HistoryError {
    HistoryError {
        status: 503,
        code: "runtime_unavailable",
        message:
            "The run controller is unavailable. Retained history is shown; retry to reconnect."
                .into(),
    }
}
fn history_gap() -> HistoryError {
    HistoryError {
        status: 409,
        code: "history_gap",
        message: "The retained history has a gap; replay cannot continue.".into(),
    }
}
pub(crate) fn invalid_cursor() -> HistoryError {
    HistoryError {
        status: 400,
        code: "invalid_cursor",
        message: "The history cursor is invalid or ahead of this run.".into(),
    }
}
