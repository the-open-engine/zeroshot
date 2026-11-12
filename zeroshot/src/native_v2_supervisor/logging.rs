//! Public run logs and failure summaries across all placements.

use std::time::Duration;

use openengine_cluster_protocol::{
    EnumLabel, FailureDiagnosticKind, TerminalResult, WorkerFailureReason, WorkerOutcome,
};

use crate::native_v2_contract::{ExecutionRef, NodeCompletion};
use crate::v2_run_ledger::{
    PrimaryGraphCompletion, RunEvent, RunLedger, RunLedgerError, RunSnapshot, SafeLogLine,
    SafeLogStream, StoredRun, cursor_sequence,
};

/// Record the executing build before preparation or dispatch can produce other logs.
/// A successor has its own empty ledger; reopening a run preserves its original version.
pub(crate) async fn record_run_version(
    ledger: &dyn RunLedger,
    stored: &mut StoredRun,
) -> Result<(), RunLedgerError> {
    if cursor_sequence(&stored.snapshot.cursor)? == 0 {
        stored.snapshot = ledger
            .append(
                &stored.snapshot.run_id,
                vec![RunEvent::SafeLog {
                    execution: None,
                    diagnostic: None,
                    timestamp: crate::native_v2_runner::current_timestamp(),
                    stream: SafeLogStream::System,
                    line: SafeLogLine::new(concat!(
                        "Zeroshot version: ",
                        env!("CARGO_PKG_VERSION")
                    ))?,
                }],
            )
            .await?
            .snapshot;
    }
    Ok(())
}

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
        if *reason == WorkerFailureReason::Stopped {
            return error_log(
                Some(reference.execution),
                format!("Node {} was stopped{timing}", reference.node.as_str()),
                Some(FailureDiagnosticKind::Failure),
            )
            .map(Some);
        }
        let reason = match reason {
            WorkerFailureReason::DeclaredFailure => "",
            WorkerFailureReason::PolicyDenied => ": policy denied",
            WorkerFailureReason::InteractiveInputRequired => ": interactive input required",
            WorkerFailureReason::AuthenticationRequired => ": authentication required",
            WorkerFailureReason::ModelUnavailable => ": model unavailable",
            WorkerFailureReason::Stopped => ": stopped",
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

pub(super) fn primary_graph_events(
    primary: PrimaryGraphCompletion,
) -> Result<Vec<RunEvent>, RunLedgerError> {
    let (stream, mut message) = match &primary.result {
        TerminalResult::Failed { reason, .. } => (
            SafeLogStream::Error,
            format!("Run failed: {}", reason.as_str()),
        ),
        TerminalResult::Succeeded { .. } => (SafeLogStream::System, "Run succeeded".to_owned()),
    };
    if primary.auxiliary_pending {
        message.push_str("; Active Memory postmortem is running");
    }
    let mut events = vec![RunEvent::SafeLog {
        execution: None,
        diagnostic: None,
        timestamp: crate::native_v2_runner::current_timestamp(),
        stream,
        line: SafeLogLine::new(message)?,
    }];
    events.push(RunEvent::PrimaryGraphCompleted {
        result: primary.result,
        auxiliary_pending: primary.auxiliary_pending,
        delivery_execution: primary.delivery_execution,
    });
    Ok(events)
}

pub(crate) fn terminal_failure_events(
    snapshot: &RunSnapshot,
    reason: &str,
    outcome: WorkerOutcome,
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
                outcome: outcome.clone(),
            },
            None,
            Some(&cause),
        )?);
    }
    clear_diagnostics(&mut events);
    if snapshot.primary_graph.is_none()
        && let TerminalResult::Failed { reason, .. } = &terminal
    {
        events.push(error_log(
            None,
            format!("Run failed: {}", reason.as_str()),
            None,
        )?);
    }
    events.push(RunEvent::Terminal { result: terminal });
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
