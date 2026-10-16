//! The cause of a failed foreground run.
//!
//! The run's durable error logs already explain its failure. A failed foreground run follows them
//! like `zeroshot logs`, within a deadline and its Ctrl-C signal, so the process error names the
//! cause. Reading them is best effort and never replaces the failure itself.

use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use openengine_cluster_protocol::{BoundedLogMessage, LogLevel, RunId, RunLogEventNotification};

use super::super::{CliOutcome, CliRunWatchEventNotification, NativeV2CliBackend, NativeV2CliError};
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
}

impl RunFailure {
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
        if !self.errors.is_empty() {
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
            formatter.write_str("\nThe run log could not be read completely.")?;
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
