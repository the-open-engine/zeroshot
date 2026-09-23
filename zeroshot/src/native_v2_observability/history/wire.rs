//! Versioned browser/archive wire records. Pages inherit the ledger's 256-event/1 MiB bound;
//! derived control updates have the canonical projector's separate 4 MiB limit.
use openengine_cluster_protocol::{
    Cursor, GraphSpec, ResolvedSource, RunId, RunTitle, RuntimePlan, TerminalResult,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ControlRecord, RuntimeFailure};
use crate::v2_run_ledger::RunPhase;

/// Maximum entries in one v1 run-history list response.
pub const RUN_HISTORY_LIST_PAGE_SIZE: usize = 50;
/// Maximum encoded bytes in one v1 list response.
pub const RUN_HISTORY_LIST_MAX_BYTES: usize = 4 * 1024 * 1024;
/// Maximum encoded bytes in one v1 definition response.
pub const RUN_HISTORY_DEFINITION_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Maximum encoded bytes in one v1 history page response.
pub const RUN_HISTORY_PAGE_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Maximum encoded bytes in one v1 problem response.
pub const RUN_HISTORY_PROBLEM_MAX_BYTES: usize = 64 * 1024;

/// Closed HTTP problem vocabulary shared by native history hosts and readers.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(usize)]
pub enum HistoryProblemCode {
    RunNotFound,
    NotFound,
    Forbidden,
    InvalidCursor,
    HistoryGap,
    RuntimeUnavailable,
    HistoryUnavailable,
    HistoryIncomplete,
    HistoryPending,
    HistoryInvalid,
    HistoryExpired,
    HistoryIncompatible,
}

const HISTORY_PROBLEM_CODES: [HistoryProblemCode; 12] = [
    HistoryProblemCode::RunNotFound,
    HistoryProblemCode::NotFound,
    HistoryProblemCode::Forbidden,
    HistoryProblemCode::InvalidCursor,
    HistoryProblemCode::HistoryGap,
    HistoryProblemCode::RuntimeUnavailable,
    HistoryProblemCode::HistoryUnavailable,
    HistoryProblemCode::HistoryIncomplete,
    HistoryProblemCode::HistoryPending,
    HistoryProblemCode::HistoryInvalid,
    HistoryProblemCode::HistoryExpired,
    HistoryProblemCode::HistoryIncompatible,
];

const HISTORY_PROBLEM_LABELS: [&str; 12] = [
    "run_not_found",
    "not_found",
    "forbidden",
    "invalid_cursor",
    "history_gap",
    "runtime_unavailable",
    "history_unavailable",
    "history_incomplete",
    "history_pending",
    "history_invalid",
    "history_expired",
    "history_incompatible",
];

impl HistoryProblemCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        HISTORY_PROBLEM_LABELS[self as usize]
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        HISTORY_PROBLEM_CODES
            .iter()
            .copied()
            .find(|code| code.as_str() == value)
    }
}

/// Host-only queue state plus the native phases used after admission.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunHistoryPhase {
    Queued,
    Admitted,
    Running,
    Stopping,
    Finished,
    Unavailable,
}

impl From<RunPhase> for RunHistoryPhase {
    fn from(value: RunPhase) -> Self {
        match value {
            RunPhase::Admitted => Self::Admitted,
            RunPhase::Running => Self::Running,
            RunPhase::Stopping => Self::Stopping,
            RunPhase::Finished => Self::Finished,
        }
    }
}

/// A bounded terminal summary. Successful output remains in run detail/history only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "status")]
pub enum RunHistoryTerminalSynopsis {
    Succeeded {},
    Failed { reason: String },
}

impl From<&TerminalResult> for RunHistoryTerminalSynopsis {
    fn from(value: &TerminalResult) -> Self {
        match value {
            TerminalResult::Succeeded { .. } => Self::Succeeded {},
            TerminalResult::Failed { reason } => Self::Failed {
                reason: reason.as_str().to_owned(),
            },
        }
    }
}

/// One bounded run-menu entry shared by local, direct-target, and hosted history.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunHistorySummary {
    pub run_id: RunId,
    pub title: RunTitle,
    pub phase: RunHistoryPhase,
    pub cursor: Option<Cursor>,
    pub terminal: Option<RunHistoryTerminalSynopsis>,
    pub source: Option<ResolvedSource>,
    pub created_at: Option<u64>,
    pub history_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_failure: Option<RuntimeFailure>,
}

/// One UUIDv7-descending run-menu page.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunHistoryList {
    pub runs: Vec<RunHistorySummary>,
    pub next_cursor: Option<RunId>,
}

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
    /// Accepts pre-v1 archives that embedded the unbounded accumulated snapshot. New producers
    /// omit it, and serialization strips it so one large run cannot expand the detail response.
    #[doc(hidden)]
    #[serde(default, rename = "snapshot", skip_serializing)]
    pub legacy_snapshot: Option<Value>,
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

#[cfg(test)]
mod tests {
    use openengine_cluster_testkit::assertions::AssertValue;
    use serde_json::json;

    use super::*;

    #[test]
    fn terminal_synopsis_never_serializes_success_output() {
        let terminal = TerminalResult::Succeeded {
            output: json!({"private": "large output"}),
        };
        let synopsis = RunHistoryTerminalSynopsis::from(&terminal);
        assert_eq!(
            serde_json::to_value(synopsis).assert_value(),
            json!({"status": "succeeded"})
        );
        assert!(
            serde_json::from_value::<RunHistoryTerminalSynopsis>(json!({
                "status": "succeeded",
                "output": {"private": "large output"}
            }))
            .is_err()
        );
    }

    #[test]
    fn run_history_list_rejects_unknown_summary_fields() {
        let valid = json!({
            "runs": [{
                "runId": "018f5e78-7f95-7c22-8d98-3f15af20c991",
                "title": "Queued run",
                "phase": "queued",
                "cursor": null,
                "terminal": null,
                "source": null,
                "createdAt": null,
                "historyAvailable": false
            }],
            "nextCursor": null
        });
        serde_json::from_value::<RunHistoryList>(valid.clone()).assert_value();
        let mut unknown = valid;
        unknown["runs"][0]["output"] = json!("not a summary field");
        assert!(serde_json::from_value::<RunHistoryList>(unknown).is_err());
    }

    #[test]
    fn history_problem_vocabulary_round_trips_including_incomplete() {
        assert!(HISTORY_PROBLEM_CODES.contains(&HistoryProblemCode::HistoryIncomplete));
        for code in HISTORY_PROBLEM_CODES {
            assert_eq!(HistoryProblemCode::parse(code.as_str()), Some(code));
            assert_eq!(
                serde_json::to_value(code).assert_value(),
                json!(code.as_str())
            );
        }
    }
}
