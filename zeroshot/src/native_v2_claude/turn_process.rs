use std::sync::Arc;

use crate::execution::process::{ProcessRunnerError, ProcessSessionCommand, ProcessSessionOutput};
use crate::native_v2_capsule::provider_process::{
    ProcessExchange, ProcessInputFailure, ProviderExecutionFiles, ProviderProcess,
    exchange_process_io, open_provider_process, process_failure_detail, require_process_cleanup,
};
use crate::native_v2_runner::{DriverControl, NodeRunnerError};

use super::transcript::ClaudeTranscript;
use super::ClaudeAttempt;

pub(super) enum ClaudeProcessStart {
    Ready(ProviderProcess),
    Failed(ClaudeAttempt),
}

pub(super) fn failed_before_start(
    error: ProcessRunnerError,
    control: &DriverControl,
) -> Result<ClaudeProcessStart, NodeRunnerError> {
    if control.is_cancelled() {
        return Err(NodeRunnerError::Cancelled);
    }
    Ok(ClaudeProcessStart::Failed(ClaudeAttempt::process_failure(
        error.to_string(),
    )))
}

pub(super) async fn open(
    files: Arc<ProviderExecutionFiles>,
    command: ProcessSessionCommand,
    control: &DriverControl,
) -> Result<ClaudeProcessStart, NodeRunnerError> {
    let process = match open_provider_process(files, command, control).await? {
        Ok(process) => process,
        Err(error) => return failed_before_start(error, control),
    };
    Ok(ClaudeProcessStart::Ready(process))
}

pub(super) async fn finish_process(
    process: &mut ProviderProcess,
    prompt: &[u8],
    mut transcript: ClaudeTranscript,
    control: &DriverControl,
) -> Result<ClaudeAttempt, NodeRunnerError> {
    let stdout = process.detach_stdout();
    let output = super::collect_transcript(stdout, &mut transcript, control);
    match exchange_process_io(process, prompt, output).await {
        ProcessExchange::Complete(Ok(())) => {
            finish_process_completion(process, transcript, control).await
        }
        ProcessExchange::Complete(Err(error)) => {
            finish_output_failure(process, transcript, control, error).await
        }
        ProcessExchange::InputFailure(failure) => {
            finish_input_failure(transcript, failure, control).await
        }
    }
}

async fn finish_input_failure(
    transcript: ClaudeTranscript,
    failure: ProcessInputFailure<Result<(), NodeRunnerError>>,
    control: &DriverControl,
) -> Result<ClaudeAttempt, NodeRunnerError> {
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
    transcript.finish(Some(&diagnostic))
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

async fn finish_output_failure(
    process: &mut ProviderProcess,
    transcript: ClaudeTranscript,
    control: &DriverControl,
    error: NodeRunnerError,
) -> Result<ClaudeAttempt, NodeRunnerError> {
    let token_usage = transcript.token_usage();
    let attempt = release_failure(
        process,
        format!("provider output delivery failed: {error}"),
        control,
    )
    .await;
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

async fn finish_process_completion(
    process: &mut ProviderProcess,
    transcript: ClaudeTranscript,
    control: &DriverControl,
) -> Result<ClaudeAttempt, NodeRunnerError> {
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

fn resolve_process_completion(
    transcript: ClaudeTranscript,
    completion: Result<ProcessSessionOutput, ProcessRunnerError>,
    cancelled: bool,
) -> Result<ClaudeAttempt, NodeRunnerError> {
    let output = require_process_cleanup(&completion)?;
    let failure = process_failure_detail(output, cancelled, !transcript.is_success())?;
    transcript.finish(failure.as_deref())
}

pub(super) async fn release_failure(
    process: &mut ProviderProcess,
    mut diagnostic: String,
    control: &DriverControl,
) -> Result<ClaudeAttempt, NodeRunnerError> {
    let completion = process.release().await;
    require_process_cleanup(&completion)?;
    if control.is_cancelled() {
        return Err(NodeRunnerError::Cancelled);
    }
    append_completion_detail(&mut diagnostic, completion)?;
    Ok(ClaudeAttempt::process_failure(diagnostic))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::process::{ProcessCleanupEvidence, ProcessLaunchEvidence};

    #[test]
    fn unconfirmed_cleanup_never_becomes_a_retryable_claude_attempt() {
        for cancelled in [false, true] {
            let output = ProcessSessionOutput {
                launch_evidence: ProcessLaunchEvidence::MayHaveStarted,
                exit_code: Some(0),
                termination_signal: None,
                core_dumped: false,
                stderr_tail: Vec::new(),
                stderr_tail_truncated: false,
                cancelled,
                timed_out: false,
                cleanup: ProcessCleanupEvidence::TimedOut,
                post_launch_error: None,
            };
            for completion in [
                Ok(output),
                Err(ProcessRunnerError::Io("completion lost".to_owned())),
            ] {
                let attempt = resolve_process_completion(
                    ClaudeTranscript::new(Vec::new()),
                    completion,
                    cancelled,
                );
                assert!(matches!(attempt, Err(NodeRunnerError::CleanupUnconfirmed)));
            }
        }
    }
}
