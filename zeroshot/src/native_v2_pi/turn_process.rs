//! One Pi turn: open the process, write the prompt, drain the event stream, and settle.

use std::sync::Arc;

use crate::execution::process::{ProcessRunnerError, ProcessSessionCommand, ProcessSessionOutput};
use crate::native_v2_capsule::provider_process::{
    ProcessExchange, ProcessInputFailure, ProviderExecutionFiles, ProviderProcess,
    exchange_process_io, open_provider_process, process_failure_detail, require_process_cleanup,
};
use crate::native_v2_runner::{DriverControl, NodeRunnerError};

use super::transcript::{PiAttempt, PiFailure, PiTranscript};

pub(super) enum PiProcessStart {
    Ready(ProviderProcess),
    Failed(PiAttempt),
}

pub(super) fn failed_before_start(
    error: ProcessRunnerError,
    control: &DriverControl,
) -> Result<PiProcessStart, NodeRunnerError> {
    if control.is_cancelled() {
        return Err(NodeRunnerError::Cancelled);
    }
    Ok(PiProcessStart::Failed(PiAttempt::Failed(PiFailure {
        session_id: None,
        retryable: false,
        diagnostic: error.to_string(),
    })))
}

pub(super) async fn open(
    files: Arc<ProviderExecutionFiles>,
    command: ProcessSessionCommand,
    control: &DriverControl,
) -> Result<PiProcessStart, NodeRunnerError> {
    let process = match open_provider_process(files, command, control).await? {
        Ok(process) => process,
        Err(error) => return failed_before_start(error, control),
    };
    Ok(PiProcessStart::Ready(process))
}

/// Streams the prompt on stdin while draining stdout concurrently, then settles the attempt.
///
/// Pi resolves a piped prompt only at stdin EOF, so closing stdin is what starts the run; the
/// exchange helper does that while keeping both directions bounded.
pub(super) async fn finish_process(
    process: &mut ProviderProcess,
    prompt: &[u8],
    mut transcript: PiTranscript,
    control: &DriverControl,
) -> Result<PiAttempt, NodeRunnerError> {
    let stdout = process.detach_stdout();
    let output = super::collect_transcript(stdout, &mut transcript, control);
    match exchange_process_io(process, prompt, output).await {
        ProcessExchange::Complete(Ok(())) => finish_completion(process, transcript, control).await,
        ProcessExchange::Complete(Err(error)) => {
            finish_output_failure(process, transcript, control, error).await
        }
        ProcessExchange::InputFailure(failure) => {
            finish_input_failure(transcript, failure, control).await
        }
    }
}

async fn finish_input_failure(
    transcript: PiTranscript,
    failure: ProcessInputFailure<Result<(), NodeRunnerError>>,
    control: &DriverControl,
) -> Result<PiAttempt, NodeRunnerError> {
    let ProcessInputFailure {
        output,
        input_error,
        completion,
    } = failure;
    let usage = control.record_token_usage(transcript.token_usage()).await;
    let cancelled = control.is_cancelled()
        || matches!(output, Err(NodeRunnerError::Cancelled))
        || matches!(&completion, Ok(output) if output.cancelled);
    // Cleanup is confirmed before the turn settles, so an unconfirmed release cannot be reduced to
    // an ordinary provider failure.
    require_process_cleanup(&completion)?;
    if cancelled {
        return Err(NodeRunnerError::Cancelled);
    }
    let mut diagnostic = format!("provider process input failed: {input_error}");
    if let Err(error) = output {
        append_detail(
            &mut diagnostic,
            &format!("provider output delivery failed: {error}"),
        );
    }
    append_completion_detail(&mut diagnostic, &completion)?;
    usage?;
    transcript.finish(Some(&diagnostic))
}

async fn finish_output_failure(
    process: &mut ProviderProcess,
    transcript: PiTranscript,
    control: &DriverControl,
    error: NodeRunnerError,
) -> Result<PiAttempt, NodeRunnerError> {
    let token_usage = transcript.token_usage();
    let attempt = release_failure(
        process,
        format!("provider output delivery failed: {error}"),
        control,
    )
    .await;
    let usage = control.record_token_usage(token_usage).await;
    match attempt {
        Err(error) => Err(error),
        Ok(attempt) => {
            usage?;
            Ok(attempt)
        }
    }
}

async fn finish_completion(
    process: &mut ProviderProcess,
    transcript: PiTranscript,
    control: &DriverControl,
) -> Result<PiAttempt, NodeRunnerError> {
    let completion = process.wait().await;
    let usage = control.record_token_usage(transcript.token_usage()).await;
    match resolve_completion(transcript, completion, control.is_cancelled()) {
        Err(error) => Err(error),
        Ok(attempt) => {
            usage?;
            Ok(attempt)
        }
    }
}

fn resolve_completion(
    transcript: PiTranscript,
    completion: Result<ProcessSessionOutput, ProcessRunnerError>,
    cancelled: bool,
) -> Result<PiAttempt, NodeRunnerError> {
    let output = require_process_cleanup(&completion)?;
    // Pi reports a failed or aborted model response in the event stream and still exits zero, so
    // provider-owned success is the transcript's terminal state, not the exit status alone.
    let failure = process_failure_detail(output, cancelled, !transcript.is_success())?;
    transcript.finish(failure.as_deref())
}

async fn release_failure(
    process: &mut ProviderProcess,
    mut diagnostic: String,
    control: &DriverControl,
) -> Result<PiAttempt, NodeRunnerError> {
    let completion = process.release().await;
    require_process_cleanup(&completion)?;
    if control.is_cancelled() {
        return Err(NodeRunnerError::Cancelled);
    }
    append_completion_detail(&mut diagnostic, &completion)?;
    Ok(PiAttempt::Failed(PiFailure {
        session_id: None,
        retryable: false,
        diagnostic,
    }))
}

fn append_completion_detail(
    diagnostic: &mut String,
    completion: &Result<ProcessSessionOutput, ProcessRunnerError>,
) -> Result<(), NodeRunnerError> {
    match completion {
        Ok(output) => {
            if let Some(detail) = process_failure_detail(output, false, true)? {
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
