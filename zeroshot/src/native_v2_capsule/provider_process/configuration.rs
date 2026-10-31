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
use crate::native_v2_runner::{DriverControl, LiveOutput, LiveOutputStream, NodeRunnerError};

use super::{
    ProviderExecutionFiles, ProviderProcess, open_provider_process, report_cleanup_failure,
    require_process_cleanup,
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

pub(crate) async fn inspect_configuration(
    files: Arc<ProviderExecutionFiles>,
    command: ProcessSessionCommand,
    control: &DriverControl,
    requests: Vec<ConfigurationRequest>,
) -> Result<Option<Vec<Value>>, NodeRunnerError> {
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

async fn inspect_native_configuration(
    files: Arc<ProviderExecutionFiles>,
    mut command: ProcessSessionCommand,
    control: &DriverControl,
    requests: Vec<ConfigurationRequest>,
) -> Result<Option<Vec<Value>>, NodeRunnerError> {
    let deadline = Instant::now() + INSPECTION_BUDGET;
    command.deadline = Some(deadline);
    let mut process = match open_provider_process(files, command, control).await? {
        Ok(process) => process,
        Err(_) if control.is_cancelled() => return Err(NodeRunnerError::Cancelled),
        Err(_) => return Ok(None),
    };
    let mut reader = ConfigurationReader::new(process.detach_stdout());
    let result = timeout_at(deadline, exchange(&mut process, &mut reader, requests)).await;
    // Keep draining after the decision; cancellation/timeout never releases the files early.
    let (completion, ()) = tokio::join!(process.release(), reader.drain());
    resolve_configuration(result, &completion, control).await
}

async fn resolve_configuration(
    result: Result<Option<Vec<Value>>, Elapsed>,
    completion: &Result<ProcessSessionOutput, ProcessRunnerError>,
    control: &DriverControl,
) -> Result<Option<Vec<Value>>, NodeRunnerError> {
    let output = require_process_cleanup(completion)?;
    if control.is_cancelled() {
        return Err(NodeRunnerError::Cancelled);
    }
    // The process deadline can close stdout before the exchange timer observes expiry.
    if result.is_err() || output.timed_out {
        control
            .emit(LiveOutput::new(
                LiveOutputStream::System,
                format!(
                    "Native configuration inspection timed out after {}s. Continuing without Zeroshot's automatic permission bypass.",
                    INSPECTION_BUDGET.as_secs(),
                ),
            )?)
            .await?;
        return Ok(None);
    }
    if output.post_launch_error.is_some() {
        return Ok(None);
    }
    Ok(result.ok().flatten())
}

async fn exchange(
    process: &mut ProviderProcess,
    reader: &mut ConfigurationReader,
    requests: Vec<ConfigurationRequest>,
) -> Option<Vec<Value>> {
    let mut responses = Vec::new();
    for request in requests {
        let mut input = Vec::new();
        for message in &request.messages {
            serde_json::to_writer(&mut input, message).ok()?;
            input.push(b'\n');
        }
        let frame = ProcessFrame::new(input).ok()?;
        let (sent, response) = tokio::join!(process.send(frame), reader.response(&request));
        sent.ok()?;
        responses.push(response?);
    }
    Some(responses)
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

    async fn response(&mut self, request: &ConfigurationRequest) -> Option<Value> {
        loop {
            while let Some(value) = self.pending.pop_front() {
                if value.pointer(request.response_pointer) == Some(&request.response_id) {
                    return Some(value);
                }
            }
            match self.stdout.recv().await {
                Some(chunk) => {
                    self.record_chunk(chunk.as_slice())?;
                }
                None => {
                    if let Some(line) = self.lines.finish() {
                        self.record(line)?;
                    }
                    return self.pending.drain(..).find(|value| {
                        value.pointer(request.response_pointer) == Some(&request.response_id)
                    });
                }
            }
        }
    }

    fn record_chunk(&mut self, bytes: &[u8]) -> Option<()> {
        self.received = self.received.checked_add(bytes.len())?;
        if self.received > MAX_CONFIGURATION_BYTES {
            return None;
        }
        for line in self.lines.push(bytes) {
            self.record(line)?;
        }
        Some(())
    }

    fn record(&mut self, line: ProviderJsonLine) -> Option<()> {
        let ProviderJsonLine::Record(bytes) = line else {
            return None;
        };
        if !bytes.iter().all(u8::is_ascii_whitespace) {
            self.pending.push_back(serde_json::from_slice(&bytes).ok()?);
        }
        Some(())
    }

    async fn drain(&mut self) {
        self.pending.clear();
        self.lines.discard();
        while self.stdout.recv().await.is_some() {}
    }
}

#[cfg(all(test, unix))]
mod tests;
