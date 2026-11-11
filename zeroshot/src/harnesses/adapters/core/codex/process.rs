use std::sync::Arc;
use std::time::Duration;

use tokio::time::{Instant, timeout_at};

use crate::execution::process::{
    ProcessRunnerError, ProcessSessionCommand, ProcessSessionOutput, ProcessStdout,
};
use crate::native_v2_capsule::provider_process::{
    ProcessExchange, ProcessInputFailure, ProviderExecutionFiles, ProviderProcess,
    open_provider_process, process_failure_evidence, send_process_input,
    provider_failure_diagnostic, report_cleanup_failure, require_process_cleanup,
    text::ProviderText,
};
use crate::native_v2_contract::TokenUsageDelta;
use crate::native_v2_runner::{DriverControl, LiveOutput, LiveOutputStream, NodeRunnerError};

use super::output::{CodexOutput, CodexOutputDecoder};
use super::session::CodexSession;

const COMPLETED_TURN_GRACE: Duration = Duration::from_millis(250);

pub(super) enum ProcessOpen {
    Ready(ProviderProcess),
    ProviderFailure(String),
}

pub(super) struct ProcessTurnContext<'a> {
    pub(super) control: &'a DriverControl,
    pub(super) session: &'a CodexSession,
    pub(super) resumed: bool,
}

struct CollectedOutput {
    output: CodexOutput,
    delivery_error: Option<NodeRunnerError>,
    usage: Option<TokenUsageDelta>,
    completion_deadline: Option<Instant>,
}

pub(super) async fn open_process(
    files: Arc<ProviderExecutionFiles>,
    command: ProcessSessionCommand,
    control: &DriverControl,
) -> Result<ProcessOpen, NodeRunnerError> {
    let process = match open_provider_process(files, command, control).await? {
        Ok(process) => process,
        Err(error) => {
            return Ok(ProcessOpen::ProviderFailure(format!(
                "provider process could not start: {error}"
            )));
        }
    };
    Ok(ProcessOpen::Ready(process))
}

async fn collect_output(
    mut stdout: ProcessStdout,
    control: &DriverControl,
    redactions: &[String],
) -> CollectedOutput {
    let mut decoder = CodexOutputDecoder::new();
    let mut delivery_error = None;
    let mut completion_deadline = None;
    loop {
        let chunk = match completion_deadline {
            Some(deadline) => timeout_at(deadline, stdout.recv()).await.unwrap_or(None),
            None => stdout.recv().await,
        };
        let Some(chunk) = chunk else { break };
        for emission in decoder.push(chunk.as_slice()) {
            if delivery_error.is_some() {
                continue;
            }
            let (stream, message) = emission.log();
            if let Err(error) = emit_text(control, stream, message, redactions).await {
                delivery_error = Some(error);
            }
        }
        if completion_deadline.is_none() && decoder.is_completed() {
            completion_deadline = Some(Instant::now() + COMPLETED_TURN_GRACE);
        }
    }
    let mut collected = finish_collection(decoder, delivery_error);
    if completion_deadline.is_some() {
        collected.completion_deadline = completion_deadline;
    }
    collected
}

pub(super) async fn exchange_turn(
    process: &mut ProviderProcess,
    prompt: &str,
    context: &ProcessTurnContext<'_>,
    redactions: &[String],
) -> Result<CodexOutput, NodeRunnerError> {
    let stdout = process.detach_stdout();
    let collected = collect_output(stdout, context.control, redactions);
    let exchange = exchange_codex_io(process, prompt.as_bytes(), collected).await;
    let (collected, completion, input_error) = match exchange {
        ProcessExchange::Complete(collected) => {
            let completion = finish_process(process, &collected).await;
            (collected, completion, None)
        }
        ProcessExchange::InputFailure(ProcessInputFailure {
            output,
            input_error,
            completion,
        }) => (output, completion, Some(input_error)),
    };
    let usage = collected.usage;
    let output = retain_delivery_evidence(collected.output, collected.delivery_error);
    if let Some(diagnostic) =
        cleanup_failure_diagnostic(&output, &completion, input_error.as_ref(), redactions)
    {
        report_cleanup_failure(context.control, diagnostic).await;
    }
    let resolved = match input_error {
        Some(error) => {
            resolve_input_failure(output, error, completion, context.control.is_cancelled())
        }
        None => resolve_process_completion(output, completion, context.control.is_cancelled()),
    };
    let normalized_usage = context.session.usage_delta(usage, context.resumed).await;
    let recorded = context.control.record_token_usage(normalized_usage).await;
    if matches!(
        &resolved,
        Err(NodeRunnerError::Cancelled | NodeRunnerError::CleanupUnconfirmed)
    ) {
        return resolved;
    }
    recorded?;
    context.session.commit_usage(usage, context.resumed).await;
    resolved
}

/// Unlike EOF, a completed Codex turn can arrive while stdin is still being written. Bound that
/// remaining write as well as process teardown, while retaining explicit input failures.
async fn exchange_codex_io(
    process: &mut ProviderProcess,
    bytes: &[u8],
    output: impl std::future::Future<Output = CollectedOutput>,
) -> ProcessExchange<CollectedOutput> {
    let mut input = Box::pin(send_process_input(process, bytes));
    let mut output = Box::pin(output);
    let (collected, input_result) = tokio::select! {
        // Preserve an already observable input failure even when output is also ready.
        biased;
        result = &mut input => {
            drop(input);
            match result {
                Ok(()) => return ProcessExchange::Complete(output.await),
                Err(input_error) => {
                    let (completion, output) = tokio::join!(process.release(), output);
                    return ProcessExchange::InputFailure(ProcessInputFailure {
                        output, input_error, completion,
                    });
                }
            }
        }
        collected = &mut output => {
            let result = match collected.completion_deadline {
                Some(deadline) => timeout_at(deadline, &mut input).await.unwrap_or_else(|_| {
                    Err(ProcessRunnerError::Io(
                        "provider process input did not finish after the completed turn".to_owned(),
                    ))
                }),
                None => (&mut input).await,
            };
            (collected, result)
        }
    };
    // Release the stdin borrow before asking the bounded process supervisor to stop it.
    drop(input);
    drop(output);
    match input_result {
        Ok(()) => ProcessExchange::Complete(collected),
        Err(input_error) => ProcessExchange::InputFailure(ProcessInputFailure {
            output: collected,
            input_error,
            completion: process.release().await,
        }),
    }
}

fn cleanup_failure_diagnostic(
    output: &Result<CodexOutput, NodeRunnerError>,
    completion: &Result<ProcessSessionOutput, ProcessRunnerError>,
    input_error: Option<&ProcessRunnerError>,
    redactions: &[String],
) -> Option<String> {
    if require_process_cleanup(completion).is_ok() {
        if input_error.is_none()
            && output.as_ref().is_ok_and(CodexOutput::is_completed)
            && !completion.as_ref().is_ok_and(|process| process.cancelled)
        {
            let detail = completion_detail(completion, false)?;
            return Some(provider_failure_diagnostic(
                "Codex",
                Some(&format!(
                    "process teardown failed after the completed turn: {detail}"
                )),
                None,
                redactions,
            ));
        }
        return None;
    }
    let mut details = match output {
        Ok(output) => output.failure_message().map(str::to_owned),
        Err(NodeRunnerError::DriverDetail(detail)) => Some(detail.clone()),
        _ => None,
    }
    .into_iter()
    .collect::<Vec<_>>();
    if let Some(error) = input_error {
        details.push(format!("provider process input failed: {error}"));
    }
    let process = completion_detail(completion, !details.is_empty());
    details.extend(process);
    (!details.is_empty())
        .then(|| provider_failure_diagnostic("Codex", Some(&details.join("; ")), None, redactions))
}

fn finish_collection(
    decoder: CodexOutputDecoder,
    error: Option<NodeRunnerError>,
) -> CollectedOutput {
    let output = decoder.finish();
    let usage = output.token_usage();
    let completion_deadline = output
        .is_completed()
        .then(|| Instant::now() + COMPLETED_TURN_GRACE);
    CollectedOutput {
        output,
        delivery_error: error,
        usage,
        completion_deadline,
    }
}

fn retain_delivery_evidence(
    mut output: CodexOutput,
    error: Option<NodeRunnerError>,
) -> Result<CodexOutput, NodeRunnerError> {
    match error {
        Some(NodeRunnerError::Cancelled) => Err(NodeRunnerError::Cancelled),
        Some(error) => {
            output.merge_failure_detail(format!("provider output delivery failed: {error}"));
            Ok(output)
        }
        None => Ok(output),
    }
}

async fn finish_process(
    process: &mut ProviderProcess,
    collected: &CollectedOutput,
) -> Result<ProcessSessionOutput, ProcessRunnerError> {
    if collected.delivery_error.is_some() {
        return process.release().await;
    }
    if let Some(deadline) = collected.completion_deadline {
        match timeout_at(deadline, process.wait()).await {
            Ok(completion) => completion,
            Err(_) => process.release().await,
        }
    } else {
        process.wait().await
    }
}

pub(super) fn resolve_process_completion(
    output: Result<CodexOutput, NodeRunnerError>,
    completion: Result<ProcessSessionOutput, ProcessRunnerError>,
    cancelled: bool,
) -> Result<CodexOutput, NodeRunnerError> {
    check_process_completion(&output, &completion, cancelled)?;
    if output.as_ref().is_ok_and(CodexOutput::is_completed) {
        return output;
    }
    let provider_output_failed = output
        .as_ref()
        .map_or(true, |output| output.failure_message().is_some());
    let process_detail = completion_detail(&completion, provider_output_failed);
    merge_completion_detail(output, process_detail)
}

pub(super) fn resolve_input_failure(
    output: Result<CodexOutput, NodeRunnerError>,
    input_error: ProcessRunnerError,
    completion: Result<ProcessSessionOutput, ProcessRunnerError>,
    cancelled: bool,
) -> Result<CodexOutput, NodeRunnerError> {
    check_process_completion(&output, &completion, cancelled)?;
    let mut output = output.unwrap_or_else(|error| {
        CodexOutput::provider_failure(format!("provider output collection failed: {error}"))
    });
    output.merge_failure_detail(format!("provider process input failed: {input_error}"));
    if let Some(detail) = completion_detail(&completion, true) {
        output.merge_failure_detail(detail);
    }
    Ok(output)
}

fn check_process_completion(
    output: &Result<CodexOutput, NodeRunnerError>,
    completion: &Result<ProcessSessionOutput, ProcessRunnerError>,
    externally_cancelled: bool,
) -> Result<(), NodeRunnerError> {
    require_process_cleanup(completion)?;
    if externally_cancelled
        || matches!(output, Err(NodeRunnerError::Cancelled))
        || matches!(completion, Ok(output) if output.cancelled)
    {
        Err(NodeRunnerError::Cancelled)
    } else {
        Ok(())
    }
}

fn completion_detail(
    completion: &Result<ProcessSessionOutput, ProcessRunnerError>,
    provider_output_failed: bool,
) -> Option<String> {
    match completion {
        Err(error) => Some(format!("provider process completion failed: {error}")),
        Ok(completion) => process_failure_evidence(completion, provider_output_failed)
            .map(|failure| failure.detail),
    }
}

fn merge_completion_detail(
    output: Result<CodexOutput, NodeRunnerError>,
    process_detail: Option<String>,
) -> Result<CodexOutput, NodeRunnerError> {
    match (output, process_detail) {
        (Ok(mut output), Some(detail)) => {
            output.merge_failure_detail(detail);
            Ok(output)
        }
        (Ok(output), None) => Ok(output),
        (Err(error), Some(detail)) => {
            let mut output = CodexOutput::provider_failure(format!(
                "provider output collection failed: {error}"
            ));
            output.merge_failure_detail(detail);
            Ok(output)
        }
        (Err(error), None) => Err(error),
    }
}

async fn emit_text(
    control: &DriverControl,
    stream: LiveOutputStream,
    text: &str,
    redactions: &[String],
) -> Result<(), NodeRunnerError> {
    let mut emissions = Vec::new();
    ProviderText::new(redactions).emit(&mut emissions, stream, text);
    for output in emissions {
        control.emit(output).await?;
    }
    if text.is_empty() {
        control.emit(LiveOutput::new(stream, "")?).await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "process/tests.rs"]
mod tests;
