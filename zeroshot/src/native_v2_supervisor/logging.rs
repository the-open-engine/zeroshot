//! Public failure summaries share the completion/terminal transaction across all placements.

use std::time::Duration;

use openengine_cluster_protocol::{
    EnumLabel, TerminalResult, WorkerErrorCode, WorkerFailureReason, WorkerOutcome,
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
        let cause = cause.map_or_else(String::new, |cause| format!(": {cause}"));
        return error_log(
            Some(reference.execution),
            format!(
                "Node {} failed: {}{timing}{reason}{cause}",
                reference.node.as_str(),
                code.as_str(),
            ),
        )
        .map(Some);
    }
    Ok(None)
}

pub(super) fn terminal_events(result: TerminalResult) -> Result<Vec<RunEvent>, RunLedgerError> {
    let mut events = Vec::new();
    if let TerminalResult::Failed { reason } = &result {
        events.push(error_log(None, format!("Run failed: {}", reason.as_str()))?);
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
    events.extend(terminal_events(terminal)?);
    Ok(events)
}

fn error_log(
    execution: Option<crate::full_v1_reducer::ExecutionId>,
    message: String,
) -> Result<RunEvent, RunLedgerError> {
    Ok(RunEvent::SafeLog {
        execution,
        timestamp: crate::native_v2_runner::current_timestamp(),
        stream: SafeLogStream::Error,
        line: SafeLogLine::new(message)?,
    })
}
