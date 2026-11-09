//! Bounded, credential-private configuration queries using the provider's own resolver.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::time::{Instant, error::Elapsed, timeout_at};

use crate::execution::process::{
    ProcessFrame, ProcessRunnerError, ProcessSessionCommand, ProcessSessionOutput, ProcessStdout,
};
use crate::native_v2_capsule::provider_json_lines::{ProviderJsonLine, ProviderJsonLines};
use crate::native_v2_runner::{
    DriverControl, LiveOutput, LiveOutputStream, NodeRunnerError, bounded_log_text,
};

use super::diagnostic::MAX_PROVIDER_DIAGNOSTIC_BYTES;
use super::{
    ProviderExecutionFiles, ProviderProcess, open_provider_process, process_failure_evidence,
    report_cleanup_failure, require_process_cleanup, safe_provider_text,
};

// These limits apply only to configuration discovery, never to a model turn's output or duration.
const INSPECTION_BUDGET: Duration = Duration::from_secs(60);
const MAX_CONFIGURATION_BYTES: usize = 4 * 1024 * 1024;

/// Result of reading native policy. Only `Unset` permits an adapter-supplied default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PermissionPolicy {
    Unset,
    /// Authored settings or native constraints must retain their existing behavior.
    Configured,
    /// Inspection failed, is unsupported, or returned an incomplete configuration.
    Unavailable,
}

pub(crate) struct ConfigurationRequest {
    pub(crate) messages: Vec<Value>,
    pub(crate) response_pointer: &'static str,
    pub(crate) response_id: Value,
}

/// Returns every response, or why inspection is unavailable. The cause may quote native stderr,
/// so publish it only through [`report_unavailable_inspection`]; responses are never published.
pub(crate) async fn inspect_configuration(
    files: Arc<ProviderExecutionFiles>,
    command: ProcessSessionCommand,
    control: &DriverControl,
    requests: Vec<ConfigurationRequest>,
) -> Result<Result<Vec<Value>, String>, NodeRunnerError> {
    let result = inspect_native_configuration(files, command, control, requests).await;
    if matches!(&result, Err(NodeRunnerError::CleanupUnconfirmed)) {
        report_cleanup_failure(
            control,
            "Native configuration inspection process cleanup could not be confirmed; model turn was not started."
                .to_owned(),
        )
        .await;
    }
    result
}

/// Unattended turns cannot answer native permission prompts, so this fallback is never debug-only.
/// It is not a failure diagnostic: native permissions apply and the turn's own result decides.
pub(crate) async fn report_unavailable_inspection(
    control: &DriverControl,
    harness: &str,
    cause: &str,
    redactions: &[String],
) -> Result<(), NodeRunnerError> {
    let message = format!(
        "Could not inspect {harness} permission settings: {}. Continuing without Zeroshot's automatic permission bypass; {harness}'s native permission behavior applies.",
        safe_provider_text(cause, redactions).trim(),
    );
    control
        .emit(LiveOutput::new(
            LiveOutputStream::Error,
            bounded_log_text(message, MAX_PROVIDER_DIAGNOSTIC_BYTES),
        )?)
        .await
}

async fn inspect_native_configuration(
    files: Arc<ProviderExecutionFiles>,
    mut command: ProcessSessionCommand,
    control: &DriverControl,
    requests: Vec<ConfigurationRequest>,
) -> Result<Result<Vec<Value>, String>, NodeRunnerError> {
    let deadline = Instant::now() + INSPECTION_BUDGET;
    command.deadline = Some(deadline);
    let mut process = match open_provider_process(files, command, control).await? {
        Ok(process) => process,
        Err(_) if control.is_cancelled() => return Err(NodeRunnerError::Cancelled),
        Err(error) => {
            return Ok(Err(format!(
                "the inspection process could not start: {error}"
            )));
        }
    };
    let mut reader = ConfigurationReader::new(process.detach_stdout());
    let result = timeout_at(deadline, exchange(&mut process, &mut reader, requests)).await;
    // Keep draining after the decision; cancellation/timeout never releases the files early.
    let (completion, ()) = tokio::join!(process.release(), reader.drain());
    resolve_configuration(result, &completion, control)
}

fn resolve_configuration(
    result: Result<Result<Vec<Value>, &'static str>, Elapsed>,
    completion: &Result<ProcessSessionOutput, ProcessRunnerError>,
    control: &DriverControl,
) -> Result<Result<Vec<Value>, String>, NodeRunnerError> {
    let output = require_process_cleanup(completion)?;
    if control.is_cancelled() {
        return Err(NodeRunnerError::Cancelled);
    }
    // The process deadline can close stdout before the exchange timer observes expiry.
    let exchanged = match result {
        Ok(exchanged) if !output.timed_out => exchanged,
        _ => {
            return Ok(Err(format!(
                "the inspection did not finish within {}s",
                INSPECTION_BUDGET.as_secs()
            )));
        }
    };
    let mut cause = match exchanged {
        Ok(responses) if output.post_launch_error.is_none() => return Ok(Ok(responses)),
        Ok(_) => "the inspection process failed after launch",
        Err(cause) => cause,
    }
    .to_owned();
    // Process facts and stderr are reported only when the process itself failed.
    if let Some(failure) = process_failure_evidence(output, false) {
        cause.push_str("; ");
        cause.push_str(&failure.detail);
    }
    Ok(Err(cause))
}

async fn exchange(
    process: &mut ProviderProcess,
    reader: &mut ConfigurationReader,
    requests: Vec<ConfigurationRequest>,
) -> Result<Vec<Value>, &'static str> {
    const UNENCODABLE: &str = "the inspection request could not be encoded";
    let mut responses = Vec::new();
    for request in requests {
        let mut input = Vec::new();
        for message in &request.messages {
            serde_json::to_writer(&mut input, message).map_err(|_| UNENCODABLE)?;
            input.push(b'\n');
        }
        let frame = ProcessFrame::new(input).map_err(|_| UNENCODABLE)?;
        let (sent, response) = tokio::join!(process.send(frame), reader.response(&request));
        // Writer failures are recorded as the process's post-launch error.
        sent.map_err(|_| "the inspection request could not be sent")?;
        responses.push(response?);
    }
    Ok(responses)
}

struct ConfigurationReader {
    stdout: ProcessStdout,
    lines: ProviderJsonLines,
    pending: VecDeque<Value>,
    received: usize,
}

impl ConfigurationReader {
    fn new(stdout: ProcessStdout) -> Self {
        Self {
            stdout,
            lines: ProviderJsonLines::new(),
            pending: VecDeque::new(),
            received: 0,
        }
    }

    async fn response(&mut self, request: &ConfigurationRequest) -> Result<Value, &'static str> {
        loop {
            while let Some(value) = self.pending.pop_front() {
                if value.pointer(request.response_pointer) == Some(&request.response_id) {
                    return Ok(value);
                }
            }
            match self.stdout.recv().await {
                Some(chunk) => self.record_chunk(chunk.as_slice())?,
                None => {
                    if let Some(line) = self.lines.finish() {
                        self.record(line)?;
                    }
                    return self
                        .pending
                        .drain(..)
                        .find(|value| {
                            value.pointer(request.response_pointer) == Some(&request.response_id)
                        })
                        .ok_or("the inspection process ended before answering");
                }
            }
        }
    }

    fn record_chunk(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        const OVERSIZED: &str = "the configuration response exceeded the 4 MiB inspection limit";
        self.received = self.received.checked_add(bytes.len()).ok_or(OVERSIZED)?;
        if self.received > MAX_CONFIGURATION_BYTES {
            return Err(OVERSIZED);
        }
        for line in self.lines.push(bytes) {
            self.record(line)?;
        }
        Ok(())
    }

    fn record(&mut self, line: ProviderJsonLine) -> Result<(), &'static str> {
        const MALFORMED: &str = "the configuration response was not valid JSON Lines";
        let ProviderJsonLine::Record(bytes) = line else {
            return Err(MALFORMED);
        };
        if !bytes.iter().all(u8::is_ascii_whitespace) {
            self.pending
                .push_back(serde_json::from_slice(&bytes).map_err(|_| MALFORMED)?);
        }
        Ok(())
    }

    async fn drain(&mut self) {
        self.pending.clear();
        self.lines.discard();
        while self.stdout.recv().await.is_some() {}
    }
}

#[cfg(all(test, unix))]
mod tests;
