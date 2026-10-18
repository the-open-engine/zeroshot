//! Shared streamed-turn exchange, cancellation, usage, and confirmed cleanup.

use std::sync::Arc;
use futures_util::FutureExt;

use crate::execution::process::{ProcessRunnerError, ProcessSessionCommand, ProcessSessionOutput};
use crate::native_v2_capsule::provider_process::{
    ProcessExchange, ProcessInputFailure, ProviderExecutionFiles, ProviderProcess,
    ProviderProcessFailure, ProviderProcessFailureKind, exchange_process_io, open_provider_process,
    process_failure_context, process_failure_detail, require_process_cleanup,
};
use crate::native_v2_runner::{DriverControl, LiveOutput, NodeRunnerError};

use crate::execution::process::ProcessStdout;
use crate::native_v2_contract::TokenUsageDelta;
use super::{ProviderAttempt, ProviderEmission};

/// Native event parsing and terminal interpretation remain owned by each harness.
pub(crate) trait ProviderTranscript: Send {
    fn push(&mut self, bytes: &[u8]) -> Vec<ProviderEmission>;
    fn finish_stream(&mut self) -> Vec<ProviderEmission>;
    fn token_usage(&self) -> Option<TokenUsageDelta>;
    fn is_success(&self) -> bool;
    fn finish(
        self,
        process_failure: Option<ProviderProcessFailure>,
    ) -> Result<ProviderAttempt, NodeRunnerError>;
}

pub(crate) enum ProviderProcessStart {
    Ready(ProviderProcess),
    Failed(ProviderAttempt),
}

pub(crate) fn failed_before_start(
    error: ProcessRunnerError,
    control: &DriverControl,
) -> Result<ProviderProcessStart, NodeRunnerError> {
    if control.is_cancelled() {
        return Err(NodeRunnerError::Cancelled);
    }
    Ok(ProviderProcessStart::Failed(
        ProviderAttempt::process_failure(error.to_string()),
    ))
}

pub(crate) async fn open(
    files: Arc<ProviderExecutionFiles>,
    command: ProcessSessionCommand,
    control: &DriverControl,
) -> Result<ProviderProcessStart, NodeRunnerError> {
    let process = match open_provider_process(files, command, control).await? {
        Ok(process) => process,
        Err(error) => return failed_before_start(error, control),
    };
    Ok(ProviderProcessStart::Ready(process))
}

pub(crate) async fn finish_process<T: ProviderTranscript>(
    process: &mut ProviderProcess,
    prompt: &[u8],
    mut transcript: T,
    control: &DriverControl,
) -> Result<ProviderAttempt, NodeRunnerError> {
    let stdout = process.detach_stdout();
    let (failed, failure) = tokio::sync::oneshot::channel();
    // Stdin can remain pending after the collector finishes. Keep a separate observation of its
    // result so failure cleanup can drain it without polling a completed future twice.
    let output = collect_transcript(stdout, &mut transcript, control, failed).shared();
    let exchange = {
        let exchange = exchange_process_io(process, prompt, output.clone());
        tokio::pin!(exchange);
        tokio::select! {
            result = &mut exchange => Some(result),
            _ = async {
                if failure.await.is_err() { std::future::pending::<()>().await; }
            } => None,
        }
    };
    let exchange = match exchange {
        Some(exchange) => exchange,
        None => {
            // Signal cleanup as soon as publication fails, while parsing buffered output and usage.
            let (completion, result) = tokio::join!(process.release(), output);
            let error = result.err().unwrap_or(NodeRunnerError::DurableOutputClosed);
            return finish_output_failure(completion, transcript, control, error).await;
        }
    };
    drop(output);
    match exchange {
        ProcessExchange::Complete(Ok(())) => {
            finish_process_completion(process, transcript, control).await
        }
        ProcessExchange::Complete(Err(error)) => {
            finish_output_failure(process.release().await, transcript, control, error).await
        }
        ProcessExchange::InputFailure(failure) => {
            finish_input_failure(transcript, failure, control).await
        }
    }
}

async fn finish_input_failure<T: ProviderTranscript>(
    transcript: T,
    failure: ProcessInputFailure<Result<(), NodeRunnerError>>,
    control: &DriverControl,
) -> Result<ProviderAttempt, NodeRunnerError> {
    let ProcessInputFailure {
        output,
        input_error,
        completion,
    } = failure;
    let usage = control.record_token_usage(transcript.token_usage()).await;
    require_process_cleanup(&completion)?;
    if input_failure_cancelled(control, &output, &completion) {
        return Err(NodeRunnerError::Cancelled);
    }

    let mut diagnostic = format!("provider process input failed: {input_error}");
    if let Err(error) = output {
        append_detail(
            &mut diagnostic,
            &format!("provider output delivery failed: {error}"),
        );
    }
    append_completion_detail(&mut diagnostic, completion)?;
    usage?;
    transcript.finish(Some(ProviderProcessFailure {
        kind: ProviderProcessFailureKind::IncompleteInput,
        detail: diagnostic,
    }))
}

fn input_failure_cancelled(
    control: &DriverControl,
    output: &Result<(), NodeRunnerError>,
    completion: &Result<ProcessSessionOutput, ProcessRunnerError>,
) -> bool {
    control.is_cancelled()
        || matches!(output, Err(NodeRunnerError::Cancelled))
        || matches!(completion, Ok(output) if output.cancelled)
}

async fn finish_output_failure<T: ProviderTranscript>(
    completion: Result<ProcessSessionOutput, ProcessRunnerError>,
    transcript: T,
    control: &DriverControl,
    error: NodeRunnerError,
) -> Result<ProviderAttempt, NodeRunnerError> {
    let token_usage = transcript.token_usage();
    let attempt = failed_completion(
        completion,
        format!("provider output delivery failed: {error}"),
        control,
    );
    let usage = control.record_token_usage(token_usage).await;
    match attempt {
        Err(NodeRunnerError::Cancelled) => Err(NodeRunnerError::Cancelled),
        Err(error) => Err(error),
        Ok(attempt) => {
            usage?;
            Ok(attempt)
        }
    }
}

async fn finish_process_completion<T: ProviderTranscript>(
    process: &mut ProviderProcess,
    transcript: T,
    control: &DriverControl,
) -> Result<ProviderAttempt, NodeRunnerError> {
    let completion = process.wait().await;
    let usage = control.record_token_usage(transcript.token_usage()).await;
    let attempt = resolve_process_completion(transcript, completion, control.is_cancelled());
    match attempt {
        Err(error) => Err(error),
        Ok(attempt) => {
            usage?;
            Ok(attempt)
        }
    }
}

fn resolve_process_completion<T: ProviderTranscript>(
    transcript: T,
    completion: Result<ProcessSessionOutput, ProcessRunnerError>,
    cancelled: bool,
) -> Result<ProviderAttempt, NodeRunnerError> {
    let output = require_process_cleanup(&completion)?;
    let failure = process_failure_context(output, cancelled, !transcript.is_success())?;
    transcript.finish(failure)
}

fn failed_completion(
    completion: Result<ProcessSessionOutput, ProcessRunnerError>,
    mut diagnostic: String,
    control: &DriverControl,
) -> Result<ProviderAttempt, NodeRunnerError> {
    require_process_cleanup(&completion)?;
    if control.is_cancelled() {
        return Err(NodeRunnerError::Cancelled);
    }
    append_completion_detail(&mut diagnostic, completion)?;
    Ok(ProviderAttempt::process_failure(diagnostic))
}

fn append_completion_detail(
    diagnostic: &mut String,
    completion: Result<ProcessSessionOutput, ProcessRunnerError>,
) -> Result<(), NodeRunnerError> {
    match completion {
        Ok(output) => {
            if let Some(detail) = process_failure_detail(&output, false, true)? {
                append_detail(diagnostic, &detail);
            }
        }
        Err(error) => append_detail(
            diagnostic,
            &format!("provider process cleanup failed: {error}"),
        ),
    }
    Ok(())
}

fn append_detail(diagnostic: &mut String, detail: &str) {
    let detail = detail.trim();
    if !detail.is_empty() && detail != diagnostic.trim() {
        diagnostic.push_str("; ");
        diagnostic.push_str(detail);
    }
}

async fn collect_transcript<T: ProviderTranscript>(
    mut stdout: ProcessStdout,
    transcript: &mut T,
    control: &DriverControl,
    failed: tokio::sync::oneshot::Sender<()>,
) -> Result<(), NodeRunnerError> {
    let mut failed = Some(failed);
    let mut delivery_error = None;
    while let Some(chunk) = stdout.recv().await {
        let emissions = transcript.push(chunk.as_slice());
        if delivery_error.is_none() {
            delivery_error = emit_provider(control, emissions).await.err();
            if delivery_error.is_some() {
                signal_output_failure(&mut failed);
            }
        }
    }
    // The transcript holds back text until a line or block ends, or while it could begin a redacted
    // value, so the remainder can include text received before cancellation. Record it even when
    // cancellation already started.
    let emissions = transcript.finish_stream();
    if matches!(delivery_error, None | Some(NodeRunnerError::Cancelled)) {
        if let Err(error) = emit_provider_final(control, emissions).await {
            delivery_error.get_or_insert(error);
            signal_output_failure(&mut failed);
        }
    }
    delivery_error.map_or(Ok(()), Err)
}

fn signal_output_failure(failed: &mut Option<tokio::sync::oneshot::Sender<()>>) {
    if let Some(failed) = failed.take() {
        let _ = failed.send(());
    }
}

async fn emit_provider(
    control: &DriverControl,
    emissions: Vec<ProviderEmission>,
) -> Result<(), NodeRunnerError> {
    let mut emissions = emissions.into_iter();
    while let Some(emission) = emissions.next() {
        let output = LiveOutput::new(emission.stream, emission.text)?;
        match control.emit(output.clone()).await {
            Ok(()) => {}
            Err(NodeRunnerError::Cancelled) => {
                // This batch can carry text held back before cancellation; record it once.
                control.emit_final(output).await?;
                emit_provider_final(control, emissions.collect()).await?;
                return Err(NodeRunnerError::Cancelled);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

async fn emit_provider_final(
    control: &DriverControl,
    emissions: Vec<ProviderEmission>,
) -> Result<(), NodeRunnerError> {
    for emission in emissions {
        control
            .emit_final(LiveOutput::new(emission.stream, emission.text)?)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
