use super::*;
use crate::execution::process::{ProcessCleanupEvidence, ProcessLaunchEvidence};

#[test]
fn unconfirmed_cleanup_never_becomes_a_retryable_provider_attempt() {
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
            let attempt = resolve_process_completion(EmptyTranscript, completion, cancelled);
            assert!(matches!(attempt, Err(NodeRunnerError::CleanupUnconfirmed)));
        }
    }
}

struct EmptyTranscript;
impl ProviderTranscript for EmptyTranscript {
    fn push(&mut self, _: &[u8]) -> Vec<ProviderEmission> {
        Vec::new()
    }
    fn finish_stream(&mut self) -> Vec<ProviderEmission> {
        Vec::new()
    }
    fn token_usage(&self) -> Option<TokenUsageDelta> {
        None
    }
    fn is_success(&self) -> bool {
        false
    }
    fn finish(self, _: Option<ProviderProcessFailure>) -> Result<ProviderAttempt, NodeRunnerError> {
        Ok(ProviderAttempt::process_failure("no answer"))
    }
}

#[test]
fn typed_completion_evidence_distinguishes_stderr_from_process_failure() {
    let mut output = ProcessSessionOutput {
        launch_evidence: ProcessLaunchEvidence::MayHaveStarted,
        exit_code: Some(0),
        termination_signal: None,
        core_dumped: false,
        stderr_tail: b"startup warning".to_vec(),
        stderr_tail_truncated: false,
        cancelled: false,
        timed_out: false,
        cleanup: ProcessCleanupEvidence::NotRequired,
        post_launch_error: None,
    };
    assert!(
        process_failure_context(&output, false, false)
            .unwrap()
            .is_none()
    );
    let warning = process_failure_context(&output, false, true)
        .unwrap()
        .unwrap();
    assert_eq!(warning.kind, ProviderProcessFailureKind::Stderr);
    assert_eq!(warning.detail, "stderr: startup warning");
    output.exit_code = Some(17);
    let failed = process_failure_context(&output, false, true)
        .unwrap()
        .unwrap();
    assert_eq!(failed.kind, ProviderProcessFailureKind::Completion);
    assert_eq!(
        failed.detail,
        "provider process exited with status 17; stderr: startup warning"
    );
}
