//! The cause of a failed foreground run.
//!
//! A foreground run keeps the structured context from its terminal status. Older runs without
//! context fall back to durable error logs within a deadline and the caller's Ctrl-C signal.

use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use openengine_cluster_protocol::{
    BoundedLogMessage, EnumLabel, FailureContext, FailureDiagnostic, LogLevel, RunId,
    RunLogEventNotification, RunStatus, TerminalResult,
};

use super::super::{
    CliOutcome, CliRunStatus, CliRunWatchEventNotification, NativeV2CliBackend, NativeV2CliError,
};
use super::{DurableFollow, DurableFollowKind, DurableOutput, follow_durable_with_detach};

/// Most recent error log records a failed foreground run reports; `zeroshot logs` keeps them all.
const MAX_REPORTED_ERRORS: usize = 5;
/// Bounds how long a failed foreground run reads its error logs before it exits.
const ERROR_LOG_DEADLINE: Duration = Duration::from_secs(5);

/// The latest error log records of a failed run and the command that shows its complete log.
///
/// Each record is a bounded protocol message without control characters, so it prints safely.
#[derive(Debug)]
pub struct RunFailure {
    run_id: RunId,
    target: Option<String>,
    errors: VecDeque<BoundedLogMessage>,
    omitted: usize,
    complete: bool,
    context: Option<(EnumLabel, Box<FailureContext>)>,
}

/// Writes the ordinary watch output while retaining its terminal failure context.
pub(super) struct FailureOutput<'a, W> {
    output: &'a mut W,
    pub(super) context: Option<(EnumLabel, Box<FailureContext>)>,
}

impl<'a, W> FailureOutput<'a, W> {
    pub(super) fn new(output: &'a mut W) -> Self {
        Self {
            output,
            context: None,
        }
    }

    pub(super) fn observe(&mut self, status: &CliRunStatus) {
        if let CliRunStatus::Target(RunStatus::Finished {
            terminal_result:
                TerminalResult::Failed {
                    reason,
                    context: Some(context),
                },
            ..
        }) = status
        {
            self.context = Some((reason.clone(), context.clone()));
        }
    }
}

impl<W: std::io::Write> DurableOutput for FailureOutput<'_, W> {
    fn watch_event(&mut self, event: CliRunWatchEventNotification) -> Result<(), NativeV2CliError> {
        self.observe(&event.status);
        super::write_json(self.output, &event)
    }

    fn log_event(&mut self, event: RunLogEventNotification) -> Result<(), NativeV2CliError> {
        super::write_json(self.output, &event)
    }
}

impl RunFailure {
    pub(super) fn from_context(
        run_id: RunId,
        target: Option<&str>,
        context: (EnumLabel, Box<FailureContext>),
    ) -> Self {
        Self {
            run_id,
            target: target.map(str::to_owned),
            complete: !context.1.incomplete,
            context: Some(context),
            errors: VecDeque::new(),
            omitted: 0,
        }
    }

    #[cfg(test)]
    pub(super) fn read_for_test(run_id: RunId, target: Option<&str>) -> Self {
        Self {
            run_id,
            target: target.map(str::to_owned),
            errors: VecDeque::new(),
            omitted: 0,
            complete: false,
            context: None,
        }
    }

    pub(super) async fn read<B, D>(
        backend: &B,
        target: Option<&str>,
        run_id: RunId,
        detach: Pin<&mut D>,
    ) -> Self
    where
        B: NativeV2CliBackend,
        D: Future<Output = ()> + ?Sized,
    {
        let mut failure = Self {
            run_id: run_id.clone(),
            target: target.map(str::to_owned),
            errors: VecDeque::new(),
            omitted: 0,
            complete: false,
            context: None,
        };
        let follow = DurableFollow {
            backend,
            target,
            run_id,
            kind: DurableFollowKind::Logs,
            initial_cursor: None,
            execution: None,
        };
        let read = follow_durable_with_detach(follow, detach, &mut failure);
        let read = tokio::time::timeout(ERROR_LOG_DEADLINE, read).await;
        failure.complete = matches!(read, Ok(Ok(CliOutcome::Completed)));
        failure
    }
}

impl DurableOutput for RunFailure {
    fn watch_event(
        &mut self,
        _event: CliRunWatchEventNotification,
    ) -> Result<(), NativeV2CliError> {
        Ok(())
    }

    fn log_event(&mut self, event: RunLogEventNotification) -> Result<(), NativeV2CliError> {
        if event.record.level == LogLevel::Error {
            if self.errors.len() == MAX_REPORTED_ERRORS {
                self.errors.pop_front();
                self.omitted += 1;
            }
            self.errors.push_back(event.record.message);
        }
        Ok(())
    }
}

impl fmt::Display for RunFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("run finished unsuccessfully")?;
        if let Some((reason, context)) = &self.context {
            write!(formatter, ": {}", reason.as_str())?;
            if let Some(primary) = &context.primary {
                formatter.write_str("\nFailure:")?;
                write_diagnostic(formatter, primary)?;
            }
            if !context.recorded.is_empty() {
                formatter.write_str("\nRecorded failures:")?;
                for diagnostic in &context.recorded {
                    write_diagnostic(formatter, diagnostic)?;
                }
            }
        } else if !self.errors.is_empty() {
            let shown = self.errors.len();
            if self.omitted == 0 {
                formatter.write_str("\nError logs:")?;
            } else {
                let total = shown + self.omitted;
                write!(formatter, "\nLatest {shown} of {total} error logs:")?;
            }
            for error in &self.errors {
                write!(formatter, "\n  {}", error.as_str())?;
            }
        }
        if !self.complete {
            formatter.write_str(if self.context.is_some() {
                "\nAdditional failure details may be missing; see the full run log."
            } else {
                "\nThe run log could not be read completely."
            })?;
        }
        write!(
            formatter,
            "\nFull run log: zeroshot logs {}",
            self.run_id.as_str()
        )?;
        if let Some(target) = &self.target {
            write!(formatter, " --target {target}")?;
        }
        Ok(())
    }
}

fn write_diagnostic(
    formatter: &mut fmt::Formatter<'_>,
    diagnostic: &FailureDiagnostic,
) -> fmt::Result {
    formatter.write_str("\n  ")?;
    if let Some(execution) = &diagnostic.execution {
        write!(formatter, "{}: ", execution.node.as_str())?;
    }
    formatter.write_str(diagnostic.message.as_str())?;
    if let Some(cursor) = &diagnostic.cursor {
        write!(formatter, " (log {})", cursor.as_str())?;
    }
    Ok(())
}
