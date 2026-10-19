use super::*;
use crate::execution::process::{ProcessCleanupEvidence, ProcessLaunchEvidence};
use crate::native_v2_capsule::provider_process::text::ProviderText;
use crate::native_v2_runner::DurableNodeEvent;
use openengine_cluster_protocol::FailureDiagnosticKind;
use openengine_cluster_testkit::assertions::AssertValue;

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
            let attempt =
                resolve_process_completion(FixtureTranscript::default(), completion, cancelled);
            assert!(matches!(attempt, Err(NodeRunnerError::CleanupUnconfirmed)));
        }
    }
}

#[derive(Default)]
struct FixtureTranscript {
    failure: Option<&'static str>,
}

impl ProviderTranscript for FixtureTranscript {
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
    fn failure_diagnostic(&self, process_detail: Option<&str>) -> Option<String> {
        ProviderText::new(&["private-token".to_owned()])
            .failure_diagnostic("Fixture", self.failure.into_iter().chain(process_detail))
    }
    fn finish(
        self,
        failure: Option<ProviderProcessFailure>,
    ) -> Result<ProviderAttempt, NodeRunnerError> {
        Ok(ProviderAttempt::process_failure(failure.map_or_else(
            || self.failure.unwrap_or("no answer").to_owned(),
            |failure| failure.detail,
        )))
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

fn cleanup_completion(cancelled: bool) -> ProcessSessionOutput {
    ProcessSessionOutput {
        launch_evidence: ProcessLaunchEvidence::MayHaveStarted,
        exit_code: Some(17),
        termination_signal: None,
        core_dumped: false,
        stderr_tail: b"buffered stderr private-token".to_vec(),
        stderr_tail_truncated: false,
        cancelled,
        timed_out: false,
        cleanup: ProcessCleanupEvidence::TimedOut,
        post_launch_error: None,
    }
}

#[test]
fn cleanup_preview_retains_captured_stderr_even_after_process_cancellation() {
    let transcript = FixtureTranscript {
        failure: Some("native rejection private-token"),
    };
    for cancelled in [false, true] {
        let diagnostic = cleanup_failure_diagnostic(
            &transcript,
            &Ok(cleanup_completion(cancelled)),
            Some("stdin rejected private-token"),
        )
        .assert_value();
        assert!(diagnostic.contains("native rejection [REDACTED]"));
        assert!(diagnostic.contains("stdin rejected [REDACTED]"));
        assert!(diagnostic.contains("buffered stderr [REDACTED]"));
        assert!(!diagnostic.contains("private-token"));
    }
    let diagnostic = cleanup_failure_diagnostic(
        &transcript,
        &Err(ProcessRunnerError::Io(
            "completion lost private-token".to_owned(),
        )),
        None,
    )
    .assert_value();
    assert!(diagnostic.contains("completion lost [REDACTED]"));

    let mut completion = cleanup_completion(false);
    completion.cleanup = ProcessCleanupEvidence::Reaped;
    assert!(cleanup_failure_diagnostic(&transcript, &Ok(completion), None).is_none());
}

#[tokio::test]
async fn input_cleanup_failure_records_original_evidence_after_control_cancellation() {
    let (cancellation, control, mut output) = crate::native_v2_runner::test_support::control();
    cancellation.send(true).assert_value();
    let failure = ProcessInputFailure {
        output: Err(NodeRunnerError::Cancelled),
        input_error: ProcessRunnerError::Io("stdin rejected private-token".to_owned()),
        completion: Ok(cleanup_completion(true)),
    };
    let result = finish_input_failure(FixtureTranscript::default(), failure, &control).await;
    assert!(matches!(result, Err(NodeRunnerError::CleanupUnconfirmed)));
    drop(control);
    assert!(matches!(
        output.recv().await.assert_value(),
        DurableNodeEvent::TokenUsage(None)
    ));
    let diagnostic = recorded_cleanup_failure(&mut output).await;
    assert!(diagnostic.text.contains("stdin rejected [REDACTED]"));

    assert!(output.recv().await.is_err());
}

#[tokio::test]
async fn output_cleanup_failure_records_original_evidence_after_control_cancellation() {
    let (cancellation, control, mut output) = crate::native_v2_runner::test_support::control();
    cancellation.send(true).assert_value();
    let result = finish_output_failure(
        Ok(cleanup_completion(true)),
        FixtureTranscript::default(),
        &control,
        NodeRunnerError::DriverDetail("publication rejected private-token".to_owned()),
    )
    .await;
    assert!(matches!(result, Err(NodeRunnerError::CleanupUnconfirmed)));
    drop(control);
    let diagnostic = recorded_cleanup_failure(&mut output).await;
    assert!(diagnostic.text.contains("publication rejected [REDACTED]"));

    assert!(matches!(
        output.recv().await.assert_value(),
        DurableNodeEvent::TokenUsage(None)
    ));
    assert!(output.recv().await.is_err());
}

#[tokio::test]
async fn confirmed_input_failure_combines_io_and_process_evidence_without_cleanup_diagnostic() {
    let (_cancellation, control, mut output) = crate::native_v2_runner::test_support::control();
    let mut completion = cleanup_completion(false);
    completion.cleanup = ProcessCleanupEvidence::Reaped;
    let failure = ProcessInputFailure {
        output: Err(NodeRunnerError::DurableOutputClosed),
        input_error: ProcessRunnerError::Io("stdin closed".to_owned()),
        completion: Ok(completion),
    };
    let ProviderAttempt::Failed(failure) =
        finish_input_failure(FixtureTranscript::default(), failure, &control)
            .await
            .assert_value()
    else {
        panic!("expected input failure")
    };
    assert!(!failure.retryable);
    assert!(failure.diagnostic.contains("stdin closed"));
    assert!(
        failure
            .diagnostic
            .contains("provider output delivery failed")
    );
    assert!(
        failure
            .diagnostic
            .contains("provider process exited with status 17")
    );
    assert!(failure.diagnostic.contains("buffered stderr"));
    drop(control);
    assert!(matches!(
        output.recv().await.assert_value(),
        DurableNodeEvent::TokenUsage(None)
    ));
    assert!(output.recv().await.is_err());
}

async fn recorded_cleanup_failure(
    output: &mut crate::native_v2_runner::DurableOutput,
) -> LiveOutput {
    let DurableNodeEvent::Output {
        output: diagnostic, ..
    } = output.recv().await.assert_value()
    else {
        panic!("expected failure evidence")
    };
    assert_eq!(diagnostic.diagnostic, Some(FailureDiagnosticKind::Failure));
    assert!(diagnostic.text.contains("buffered stderr [REDACTED]"));
    assert!(!diagnostic.text.contains("private-token"));
    diagnostic
}
