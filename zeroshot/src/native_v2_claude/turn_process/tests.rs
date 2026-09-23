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
