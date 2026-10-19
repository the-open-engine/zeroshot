use crate::execution::process::{ProcessCleanupEvidence, ProcessLaunchEvidence, ProcessSessionOutput};
use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

fn completion(cleanup: ProcessCleanupEvidence, cancelled: bool) -> ProcessSessionOutput {
    ProcessSessionOutput {
        launch_evidence: ProcessLaunchEvidence::MayHaveStarted,
        exit_code: Some(0),
        termination_signal: None,
        core_dumped: false,
        stderr_tail: Vec::new(),
        stderr_tail_truncated: false,
        cancelled,
        timed_out: false,
        cleanup,
        post_launch_error: None,
    }
}

#[test]
fn process_cancellation_wins_over_output_collection_failure() {
    let resolved = resolve_process_completion(
        Err(NodeRunnerError::DurableOutputClosed),
        Ok(completion(ProcessCleanupEvidence::Reaped, true)),
        false,
    );

    assert!(matches!(resolved, Err(NodeRunnerError::Cancelled)));
}

#[test]
fn unknown_process_cleanup_is_fatal_despite_external_cancellation() {
    let resolved = resolve_process_completion(
        Err(NodeRunnerError::DurableOutputClosed),
        Err(ProcessRunnerError::Io("supervisor stopped".to_owned())),
        true,
    );

    assert!(matches!(resolved, Err(NodeRunnerError::CleanupUnconfirmed)));
}

#[test]
fn unconfirmed_cleanup_cannot_become_retryable_provider_output() {
    for cancelled in [false, true] {
        let completed = resolve_process_completion(
            Err(NodeRunnerError::DurableOutputClosed),
            Ok(completion(ProcessCleanupEvidence::TimedOut, cancelled)),
            cancelled,
        );
        let input_failed = resolve_input_failure(
            Err(NodeRunnerError::Cancelled),
            ProcessRunnerError::Io("stdin closed".to_owned()),
            Ok(completion(ProcessCleanupEvidence::TimedOut, cancelled)),
            cancelled,
        );
        assert!(matches!(
            completed,
            Err(NodeRunnerError::CleanupUnconfirmed)
        ));
        assert!(matches!(
            input_failed,
            Err(NodeRunnerError::CleanupUnconfirmed)
        ));
    }
}

#[test]
fn clean_completion_does_not_replace_output_collection_failure() {
    let resolved = resolve_process_completion(
        Err(NodeRunnerError::DurableOutputClosed),
        Ok(completion(ProcessCleanupEvidence::Reaped, false)),
        false,
    );

    assert!(matches!(
        resolved,
        Err(NodeRunnerError::DurableOutputClosed)
    ));
}

#[test]
fn parsing_after_an_emission_failure_retains_later_terminal_usage() {
    let mut decoder = CodexOutputDecoder::new();
    let emissions = decoder.push(
        concat!(
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",",
            "\"text\":\"done\"}}\n",
        )
        .as_bytes(),
    );
    assert_eq!(emissions.len(), 1);

    let suppressed = decoder.push(
        concat!(
            "{\"type\":\"turn.completed\",\"usage\":{",
            "\"input_tokens\":17,\"cached_input_tokens\":4,",
            "\"cache_write_input_tokens\":3,\"output_tokens\":9}}\n",
        )
        .as_bytes(),
    );
    assert_eq!(suppressed.len(), 1);

    let collected = finish_collection(decoder, Some(NodeRunnerError::DurableOutputClosed));
    assert!(matches!(
        collected.delivery_error,
        Some(NodeRunnerError::DurableOutputClosed)
    ));
    assert_eq!(collected.output.final_message().assert_value(), "done");
    let usage = collected.usage.assert_value();
    assert_eq!(usage.input_tokens.get(), 17);
    assert_eq!(usage.output_tokens.get(), 9);
    assert_eq!(usage.cache_read_input_tokens.assert_value().get(), 4);
    assert_eq!(usage.cache_creation_input_tokens.assert_value().get(), 3);
}

#[test]
fn coverage_contract_delivery_and_process_failures_preserve_all_safe_evidence() {
    assert!(matches!(
        retain_delivery_evidence(
            CodexOutput::provider_failure("provider".to_owned()),
            Some(NodeRunnerError::Cancelled),
        ),
        Err(NodeRunnerError::Cancelled)
    ));
    let output = retain_delivery_evidence(
        CodexOutput::provider_failure("provider".to_owned()),
        Some(NodeRunnerError::DurableOutputClosed),
    )
    .assert_value();
    assert!(
        output
            .failure_message()
            .assert_value()
            .contains("provider output delivery failed")
    );

    let resolved = resolve_input_failure(
        Err(NodeRunnerError::DurableOutputClosed),
        ProcessRunnerError::Io("stdin closed".to_owned()),
        Ok(completion(ProcessCleanupEvidence::Reaped, false)),
        false,
    )
    .assert_value();
    let detail = resolved.failure_message().assert_value();
    assert!(detail.contains("provider output collection failed"));
    assert!(detail.contains("provider process input failed"));

    let completion_error = completion_detail(
        &Err(ProcessRunnerError::Launch("not started".to_owned())),
        true,
    )
    .assert_value();
    assert!(completion_error.contains("provider process completion failed"));
    let merged = merge_completion_detail(
        Err(NodeRunnerError::Driver),
        Some("process exited".to_owned()),
    )
    .assert_value();
    let detail = merged.failure_message().assert_value();
    assert!(detail.contains("provider output collection failed"));
    assert!(detail.contains("process exited"));
}

#[test]
fn coverage_contract_output_chunk_boundaries_preserve_utf8_without_exceeding_the_live_limit() {
    assert_eq!(
        crate::native_v2_capsule::provider_process::text::utf8_chunks("short", 8192)[0].len(),
        5
    );
    let text = format!("{}é-tail", "a".repeat(8 * 1024 - 1));
    let end = crate::native_v2_capsule::provider_process::text::utf8_chunks(&text, 8192)[0].len();
    assert_eq!(end, 8 * 1024 - 1);
    assert!(text.is_char_boundary(end));
    assert!(end <= 8 * 1024);
}

#[test]
fn unconfirmed_cleanup_retains_redacted_native_input_and_process_causes() {
    let secret = "sk-cleanup-secret";
    let output = Ok(CodexOutput::provider_failure(format!(
        "native model rejection {secret}"
    )));
    let mut process = completion(ProcessCleanupEvidence::TimedOut, true);
    process.exit_code = Some(17);
    process.stderr_tail = format!("stderr cause {secret}").into_bytes();
    let process = Ok(process);
    let input = ProcessRunnerError::Io(format!("stdin rejected {secret}"));
    let diagnostic =
        cleanup_failure_diagnostic(&output, &process, Some(&input), &[secret.to_owned()]).unwrap();
    assert!(diagnostic.contains("native model rejection [REDACTED]"));
    assert!(diagnostic.contains("stdin rejected [REDACTED]"));
    assert!(diagnostic.contains("stderr cause [REDACTED]"));
    assert!(!diagnostic.contains(secret));
    assert!(matches!(
        resolve_input_failure(output, input, process, true),
        Err(NodeRunnerError::CleanupUnconfirmed)
    ));
}

#[test]
fn successful_answer_text_is_never_previewed_as_a_cleanup_failure() {
    let mut decoder = CodexOutputDecoder::new();
    decoder.push(b"{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"private successful answer\"}}\n");
    decoder.push(b"{\"type\":\"turn.completed\"}\n");
    let output = Ok(decoder.finish());
    let diagnostic = cleanup_failure_diagnostic(
        &output,
        &Ok(completion(ProcessCleanupEvidence::TimedOut, false)),
        None,
        &[],
    )
    .unwrap();
    assert!(diagnostic.contains("cleanup"));
    assert!(!diagnostic.contains("private successful answer"));
}

#[test]
fn cancelled_cleanup_keeps_stderr_without_a_parsed_native_failure() {
    let mut process = completion(ProcessCleanupEvidence::TimedOut, true);
    process.stderr_tail = b"buffered native failure private-token".to_vec();
    let process = Ok(process);
    let output = Err(NodeRunnerError::Cancelled);
    let diagnostic =
        cleanup_failure_diagnostic(&output, &process, None, &["private-token".to_owned()])
            .assert_value();
    assert!(diagnostic.contains("buffered native failure [REDACTED]"));
    assert!(!diagnostic.contains("private-token"));
    assert!(matches!(
        resolve_process_completion(output, process, true),
        Err(NodeRunnerError::CleanupUnconfirmed)
    ));
}
