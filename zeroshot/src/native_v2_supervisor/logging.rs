//! Public failure summaries share the completion/terminal transaction across all placements.

use std::time::Duration;

use openengine_cluster_protocol::{
    EnumLabel, FailureDiagnosticKind, TerminalResult, WorkerErrorCode, WorkerFailureReason,
    WorkerOutcome,
};

use crate::native_v2_contract::{ExecutionRef, NodeCompletion};
use crate::v2_run_ledger::{RunEvent, RunLedgerError, RunSnapshot, SafeLogLine, SafeLogStream};

pub(super) fn completion_events(
    completion: NodeCompletion,
    elapsed: Option<Duration>,
    cause: Option<&str>,
) -> Result<Vec<RunEvent>, RunLedgerError> {
    let mut events = Vec::new();
    if let Some(log) = node_failure_log(&completion.reference, &completion.outcome, elapsed, cause)?
    {
        events.push(log);
    }
    events.push(RunEvent::NodeCompleted { completion });
    Ok(events)
}

pub(super) fn node_failure_log(
    reference: &ExecutionRef,
    outcome: &WorkerOutcome,
    elapsed: Option<Duration>,
    cause: Option<&str>,
) -> Result<Option<RunEvent>, RunLedgerError> {
    if let WorkerOutcome::Error { code, reason } = outcome {
        let timing = elapsed.map_or_else(String::new, |duration| {
            format!(" after {:.1}s", duration.as_secs_f64())
        });
        let reason = match reason {
            WorkerFailureReason::DeclaredFailure => "",
            WorkerFailureReason::PolicyDenied => ": policy denied",
            WorkerFailureReason::InteractiveInputRequired => ": interactive input required",
            WorkerFailureReason::AuthenticationRequired => ": authentication required",
            WorkerFailureReason::MalformedResult => ": result did not match the node contract",
        };
        let cause_text = cause.map_or_else(String::new, |cause| format!(": {cause}"));
        return error_log(
            Some(reference.execution),
            format!(
                "Node {} failed: {}{timing}{reason}{cause_text}",
                reference.node.as_str(),
                code.as_str(),
            ),
            Some(FailureDiagnosticKind::Failure),
        )
        .map(Some);
    }
    Ok(None)
}

pub(super) fn runtime_failure_log(cause: &str) -> Result<RunEvent, RunLedgerError> {
    error_log(
        None,
        format!("Runtime failed: {cause}"),
        Some(FailureDiagnosticKind::Failure),
    )
}

pub(crate) fn terminal_events(result: TerminalResult) -> Result<Vec<RunEvent>, RunLedgerError> {
    let mut events = Vec::new();
    if let TerminalResult::Failed { reason, .. } = &result {
        events.push(error_log(
            None,
            format!("Run failed: {}", reason.as_str()),
            None,
        )?);
    }
    events.push(RunEvent::Terminal { result });
    Ok(events)
}

pub(crate) fn terminal_failure_events(
    snapshot: &RunSnapshot,
    reason: &str,
    code: WorkerErrorCode,
) -> Result<Vec<RunEvent>, RunLedgerError> {
    let terminal = TerminalResult::Failed {
        context: None,
        reason: EnumLabel::new(reason).map_err(|_| RunLedgerError::Corrupt)?,
    };
    let cause = format!("run {reason}");
    let mut events = Vec::new();
    for node in snapshot.active_executions() {
        events.extend(completion_events(
            NodeCompletion {
                reference: node.reference.clone(),
                outcome: WorkerOutcome::declared_failure(code),
            },
            None,
            Some(&cause),
        )?);
    }
    clear_diagnostics(&mut events);
    events.extend(terminal_events(terminal)?);
    Ok(events)
}

fn error_log(
    execution: Option<crate::full_v1_reducer::ExecutionId>,
    message: String,
    diagnostic: Option<FailureDiagnosticKind>,
) -> Result<RunEvent, RunLedgerError> {
    Ok(RunEvent::SafeLog {
        execution,
        diagnostic,
        timestamp: crate::native_v2_runner::current_timestamp(),
        stream: SafeLogStream::Error,
        line: SafeLogLine::new(crate::native_v2_runner::bounded_log_text(
            message,
            crate::v2_run_ledger::MAX_SAFE_LOG_BYTES / 2,
        ))?,
    })
}

/// Synthetic run-stop completions and duplicate fallback logs are not new diagnostic causes.
pub(super) fn clear_diagnostics(events: &mut [RunEvent]) {
    for event in events {
        if let RunEvent::SafeLog { diagnostic, .. } = event {
            *diagnostic = None;
        }
    }
}

/// A producer detail supersedes a generic settlement summary, never a later concrete error.
pub(super) fn clear_recorded_diagnostics(
    events: &mut [RunEvent],
    snapshot: &RunSnapshot,
    has_detail: bool,
) {
    if has_detail {
        return;
    }
    for event in events {
        if let RunEvent::SafeLog {
            execution: Some(id),
            diagnostic,
            ..
        } = event
            && snapshot
                .executions
                .get(id)
                .is_some_and(|node| node.failure_diagnostic)
        {
            *diagnostic = None;
        }
    }
}
