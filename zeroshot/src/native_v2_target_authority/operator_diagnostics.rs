use std::collections::VecDeque;
use std::fmt;
use std::sync::Mutex;

use openengine_cluster_protocol::{RunId, TargetOperatorDiagnostic, TargetOperatorDiagnostics};

const MAX_BUFFERED_DIAGNOSTICS: usize = 2;
pub(crate) const MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES: usize = 4 * 1024;

pub(crate) struct NewOperatorDiagnostic {
    pub run_id: RunId,
    pub code: &'static str,
    pub operation: &'static str,
    pub exit_status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Default)]
struct DiagnosticState {
    next_id: u64,
    diagnostics: VecDeque<TargetOperatorDiagnostic>,
}

#[derive(Default)]
pub(crate) struct OperatorDiagnosticStore {
    state: Mutex<DiagnosticState>,
}

impl fmt::Debug for OperatorDiagnosticStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OperatorDiagnosticStore(..)")
    }
}

impl OperatorDiagnosticStore {
    pub(crate) fn record(&self, mut diagnostic: NewOperatorDiagnostic) {
        let (stdout, stdout_truncated) = bounded_text(diagnostic.stdout);
        let (stderr, stderr_truncated) = bounded_text(diagnostic.stderr);
        diagnostic.stdout = stdout;
        diagnostic.stderr = stderr;
        diagnostic.stdout_truncated |= stdout_truncated;
        diagnostic.stderr_truncated |= stderr_truncated;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.next_id = state.next_id.saturating_add(1);
        if state.diagnostics.len() == MAX_BUFFERED_DIAGNOSTICS {
            state.diagnostics.pop_front();
        }
        let id = state.next_id.to_string();
        state.diagnostics.push_back(TargetOperatorDiagnostic {
            id,
            run_id: diagnostic.run_id,
            code: diagnostic.code.to_owned(),
            operation: diagnostic.operation.to_owned(),
            exit_status: diagnostic.exit_status,
            stdout: diagnostic.stdout,
            stderr: diagnostic.stderr,
            stdout_truncated: diagnostic.stdout_truncated,
            stderr_truncated: diagnostic.stderr_truncated,
        });
    }

    pub(crate) fn snapshot(&self, run_id: &RunId) -> TargetOperatorDiagnostics {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        TargetOperatorDiagnostics {
            diagnostics: state
                .diagnostics
                .iter()
                .filter(|diagnostic| &diagnostic.run_id == run_id)
                .cloned()
                .collect(),
        }
    }
}

fn bounded_text(text: String) -> (String, bool) {
    let mut normalized = text
        .chars()
        .map(|character| match character {
            '\n' | '\r' | '\t' => character,
            character if character.is_control() => '\u{fffd}',
            character => character,
        })
        .collect::<String>();
    if normalized.len() <= MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES {
        return (normalized, false);
    }
    let mut boundary = MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES;
    while !normalized.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    normalized.truncate(boundary);
    (normalized, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_id(value: &str) -> RunId {
        RunId::new(value)
    }

    fn diagnostic(run_id: RunId, text: String) -> NewOperatorDiagnostic {
        NewOperatorDiagnostic {
            run_id,
            code: "git_push_failed",
            operation: "delivery.git_push",
            exit_status: Some(128),
            stdout: text.clone(),
            stderr: text,
            stdout_truncated: false,
            stderr_truncated: false,
        }
    }

    #[test]
    fn snapshots_are_run_scoped_and_the_global_queue_is_bounded() {
        let store = OperatorDiagnosticStore::default();
        let requested = run_id("018f5e78-7f95-7c22-8d98-3f15af20c991");
        let other = run_id("018f5e78-7f95-7c22-8d98-3f15af20c992");
        store.record(diagnostic(requested.clone(), "evicted".to_owned()));
        store.record(diagnostic(other, "other".to_owned()));
        store.record(diagnostic(requested.clone(), "retained".to_owned()));

        let snapshot = store.snapshot(&requested);

        assert_eq!(snapshot.diagnostics.len(), 1);
        assert_eq!(snapshot.diagnostics[0].id, "3");
        assert_eq!(snapshot.diagnostics[0].stdout, "retained");
        let debug = format!("{store:?}");
        assert!(!debug.contains("retained"));
    }

    #[test]
    fn worst_case_snapshot_stays_below_the_transport_response_limit() {
        let store = OperatorDiagnosticStore::default();
        let requested = run_id("018f5e78-7f95-7c22-8d98-3f15af20c991");
        let escaped = "\n".repeat(MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES);
        store.record(diagnostic(requested.clone(), escaped.clone()));
        store.record(diagnostic(requested.clone(), escaped));

        let encoded = serde_json::to_vec(&store.snapshot(&requested)).expect("serialize snapshot");

        assert!(encoded.len() < 64 * 1024, "{} bytes", encoded.len());
    }
}
