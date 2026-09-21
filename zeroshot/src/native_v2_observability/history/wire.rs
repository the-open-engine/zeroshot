//! Versioned browser/archive wire records. Pages inherit the ledger's 256-event/1 MiB bound;
//! derived control updates have the canonical projector's separate 4 MiB limit.
use openengine_cluster_protocol::{
    Cursor, GraphSpec, ResolvedSource, RunId, RunTitle, RuntimePlan, TerminalResult,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::v2_run_ledger::RunPhase;
use super::{ControlRecord, RuntimeFailure};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunDefinition {
    pub version: u8,
    pub projection_version: u8,
    pub run_id: RunId,
    pub title: RunTitle,
    pub created_at: Option<u64>,
    pub phase: RunPhase,
    pub cursor: Cursor,
    pub terminal: Option<TerminalResult>,
    pub history_available: bool,
    pub graph: GraphSpec,
    pub runtime: RuntimePlan,
    pub initial_input: Value,
    pub source: ResolvedSource,
    pub snapshot: Value,
    pub history: HistoryMetadata,
    pub observation: Observation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_failure: Option<RuntimeFailure>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryMetadata {
    pub initial_cursor: Cursor,
    pub cursor: Cursor,
    pub complete: bool,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryPage {
    pub events: Vec<Value>,
    pub next_cursor: Cursor,
    pub head_cursor: Cursor,
    pub complete: bool,
    pub finished: bool,
    pub observation: Observation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_failure: Option<RuntimeFailure>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub control: Vec<ControlRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_error: Option<String>,
}

/// Observation availability is independent of current run state and replay position.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Observation {
    pub state: ObservationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationState {
    Active,
    Collecting,
    Complete,
    Incomplete,
    Expired,
    Unavailable,
}
