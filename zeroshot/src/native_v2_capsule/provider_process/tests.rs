use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

#[test]
fn verifier_workspace_follows_process_isolation() {
    assert_eq!(
        ProviderProcessRunners::local().verifier_workspace(),
        VerifierWorkspace::Shared
    );
    let pool = HostedProcessPool::new(10_002, 10_002, 20_000, 20_000).assert_value();
    assert_eq!(
        ProviderProcessRunners::hosted(pool).verifier_workspace(),
        VerifierWorkspace::Isolated
    );
}

#[test]
fn boundary_contract_runner_selection_and_agent_workspace_access_follow_the_execution_boundary() {
    assert_eq!(
        agent_workspace_access(NodeRole::Worker),
        Ok(WorkspaceAccessMode::ReadWrite)
    );
    assert_eq!(
        agent_workspace_access(NodeRole::Verifier),
        Ok(WorkspaceAccessMode::ReadWrite)
    );
    assert_eq!(
        agent_workspace_access(NodeRole::GitDelivery),
        Err(NodeRunnerError::Driver)
    );

    let local = ProviderProcessRunners::local();
    assert!(!local.is_hosted());
    let pool = HostedProcessPool::new(10_002, 10_002, 20_000, 20_000).assert_value();
    assert!(ProviderProcessRunners::hosted(pool).is_hosted());

    let root = tempfile::tempdir().assert_value();
    let (_, home) = local
        .turn_process(root.path(), HostedProcessScope::WriterExecution(7))
        .assert_value();
    assert!(home.is_dir());
}

#[tokio::test]
async fn boundary_contract_session_home_is_stable_and_closed_sessions_refuse_new_retention() {
    let root = tempfile::tempdir().assert_value();
    let first_path = root.path().join("first");
    let second_path = root.path().join("second");
    std::fs::create_dir(&first_path).assert_value();
    std::fs::create_dir(&second_path).assert_value();
    let session = ProviderSessionCore::new();
    let first = session.retain_home(first_path.clone()).await.assert_value();
    let same = session.retain_home(first_path).await.assert_value();
    assert!(Arc::ptr_eq(&first, &same));
    assert!(matches!(
        session.retain_home(second_path.clone()).await,
        Err(ProcessRunnerError::InvalidCommand(detail)) if detail.contains("home changed")
    ));
    session.close().await;
    assert!(matches!(
        session.retain_home(second_path).await,
        Err(ProcessRunnerError::InvalidCommand(detail)) if detail.contains("session is closed")
    ));
}

#[tokio::test]
async fn closed_session_failure_maps_to_selected_runner_error() {
    let session = ProviderSessionCore::new();
    session.close().await;

    assert_eq!(
        session.ensure_live(ClosedSessionFailure::Driver),
        Err(NodeRunnerError::Driver)
    );
    assert_eq!(
        session.ensure_live(ClosedSessionFailure::SessionLost),
        Err(NodeRunnerError::SessionLost)
    );
}

fn process_output() -> ProcessSessionOutput {
    ProcessSessionOutput {
        launch_evidence: crate::execution::process::ProcessLaunchEvidence::MayHaveStarted,
        exit_code: Some(0),
        termination_signal: None,
        core_dumped: false,
        stderr_tail: Vec::new(),
        stderr_tail_truncated: false,
        cancelled: false,
        timed_out: false,
        cleanup: crate::execution::process::ProcessCleanupEvidence::NotRequired,
        post_launch_error: None,
    }
}

#[test]
fn process_failure_preserves_cleanup_exit_and_stderr_detail() {
    let mut output = process_output();
    output.exit_code = Some(17);
    output.stderr_tail = b"provider detail".to_vec();
    output.cleanup = crate::execution::process::ProcessCleanupEvidence::TimedOut;
    output.post_launch_error = Some("stdout pump failed".to_owned());

    let detail = process_failure_detail(&output, false, false)
        .assert_value()
        .assert_value();
    assert!(detail.contains("process cleanup did not prove the process tree empty"));
    assert!(detail.contains("stdout pump failed"));
    assert!(detail.contains("provider process exited with status 17"));
    assert!(detail.contains("stderr: provider detail"));
}

#[test]
fn process_failure_gives_cancellation_precedence() {
    let output = process_output();
    assert_eq!(
        process_failure_detail(&output, true, false),
        Err(NodeRunnerError::Cancelled)
    );
}

#[test]
fn clean_process_has_no_failure_detail() {
    assert_eq!(
        process_failure_detail(&process_output(), false, false),
        Ok(None)
    );
}

#[test]
fn stderr_alone_is_evidence_only_after_provider_output_failure() {
    let mut output = process_output();
    output.stderr_tail = b"provider warning".to_vec();

    assert_eq!(process_failure_detail(&output, false, false), Ok(None));
    assert_eq!(
        process_failure_detail(&output, false, true),
        Ok(Some("stderr: provider warning".to_owned()))
    );
}

#[test]
fn process_failure_distinguishes_timeout_from_missing_exit_status() {
    let mut output = process_output();
    output.exit_code = None;
    output.timed_out = true;
    assert_eq!(
        process_failure_detail(&output, false, false),
        Ok(Some("provider process timed out".to_owned()))
    );

    output.timed_out = false;
    assert_eq!(
        process_failure_detail(&output, false, false),
        Ok(Some("provider process exited without a status".to_owned()))
    );
}

#[test]
fn process_failure_distinguishes_unix_signal_and_core_dump_from_missing_status() {
    let mut output = process_output();
    output.exit_code = None;
    output.termination_signal = Some(11);
    output.core_dumped = true;

    assert_eq!(
        process_failure_detail(&output, false, false),
        Ok(Some(
            "provider process terminated by signal 11 (core dumped)".to_owned()
        ))
    );
}

#[test]
fn boundary_contract_process_failure_filters_empty_diagnostics_and_covers_non_core_signal_and_owned_cancellation()
 {
    let mut output = process_output();
    output.cancelled = true;
    assert_eq!(
        process_failure_detail(&output, false, false),
        Err(NodeRunnerError::Cancelled)
    );

    output.cancelled = false;
    output.exit_code = None;
    output.termination_signal = Some(9);
    assert_eq!(
        process_failure_detail(&output, false, false),
        Ok(Some("provider process terminated by signal 9".to_owned()))
    );

    output.termination_signal = None;
    output.exit_code = Some(0);
    output.post_launch_error = Some("  ".to_owned());
    output.stderr_tail = b" \n\t".to_vec();
    assert_eq!(process_failure_detail(&output, false, true), Ok(None));
}

#[test]
fn diagnostic_retains_stderr_from_an_otherwise_clean_process() {
    let mut output = process_output();
    output.stderr_tail = b"clean-exit detail".to_vec();

    let diagnostic = provider_failure_diagnostic("Claude", None, Some(&output), &[]);
    assert_eq!(
        diagnostic,
        "Claude provider failure: stderr: clean-exit detail"
    );
}

#[test]
fn diagnostic_combines_parser_process_and_redacted_stderr_detail() {
    let mut output = process_output();
    output.exit_code = Some(2);
    output.stderr_tail = b"token=secret".to_vec();

    let diagnostic = provider_failure_diagnostic(
        "Claude",
        Some("JSONL stream ended without a result"),
        Some(&output),
        &["secret".to_owned()],
    );
    assert!(diagnostic.contains("JSONL stream ended without a result"));
    assert!(diagnostic.contains("provider process exited with status 2"));
    assert!(diagnostic.contains("stderr: token=[REDACTED]"));
    assert!(!diagnostic.contains("secret"));
}

#[test]
fn provider_diagnostic_is_redacted_sanitized_and_bounded() {
    let detail = format!(
        "token=secret\u{0} {}",
        "x".repeat(MAX_PROVIDER_DIAGNOSTIC_BYTES)
    );
    let diagnostic =
        provider_failure_diagnostic("Codex", Some(&detail), None, &["secret".to_owned()]);

    assert!(diagnostic.starts_with("Codex provider failure: token=[REDACTED]\u{fffd} "));
    assert!(!diagnostic.contains("secret"));
    assert!(!diagnostic.contains('\0'));
    assert_eq!(diagnostic.len(), MAX_PROVIDER_DIAGNOSTIC_BYTES);
    assert!(diagnostic.contains(" ... [middle truncated] ... "));
    assert!(diagnostic.ends_with('x'));
}

#[test]
fn provider_retry_extends_dynamic_redactions_in_longest_first_order() {
    let mut retry =
        ProviderFailureRetry::new("Codex", "prompt".to_owned(), vec!["secret".to_owned()]);
    retry.extend_redactions(&["secret-tail".to_owned(), "secret".to_owned(), String::new()]);

    assert_eq!(retry.redactions, ["secret-tail", "secret"]);
    assert_eq!(
        provider_failure_diagnostic(
            "Codex",
            Some("dynamic=secret-tail base=secret"),
            None,
            &retry.redactions,
        ),
        "Codex provider failure: dynamic=[REDACTED] base=[REDACTED]"
    );
}

#[test]
fn bounded_diagnostic_keeps_utf8_parser_context_and_process_root_cause() {
    let mut output = process_output();
    output.exit_code = Some(23);
    output.stderr_tail = b"fatal root cause at stderr tail".to_vec();
    let parser_detail = format!("parser context: {}", "界".repeat(4_000));

    let diagnostic =
        provider_failure_diagnostic("Claude", Some(&parser_detail), Some(&output), &[]);

    assert!(diagnostic.starts_with("Claude provider failure: parser context: "));
    assert!(diagnostic.contains(" ... [middle truncated] ... "));
    assert!(diagnostic.contains("provider process exited with status 23"));
    assert!(diagnostic.ends_with("stderr: fatal root cause at stderr tail"));
    assert!(diagnostic.len() <= MAX_PROVIDER_DIAGNOSTIC_BYTES);
}

#[test]
fn stderr_tail_cutoff_redacts_leading_ascii_secret_suffix() {
    let secret = "declared-secret-value";
    let split = "declared-".len();
    let diagnostic = cutoff_secret_diagnostic(secret, split);

    assert_eq!(
        diagnostic,
        "Claude provider failure: stderr (truncated tail): [REDACTED]: actionable root cause"
    );
}

#[test]
fn stderr_tail_cutoff_redacts_secret_split_inside_multibyte_character() {
    let secret = "declared-🔐-suffix";
    let split = secret.find('🔐').assert_value() + 1;
    let diagnostic = cutoff_secret_diagnostic(secret, split);

    assert_eq!(
        diagnostic,
        "Claude provider failure: stderr (truncated tail): [REDACTED]: actionable root cause"
    );
}

fn cutoff_secret_diagnostic(secret: &str, split: usize) -> String {
    let mut output = process_output();
    output.stderr_tail_truncated = true;
    output.stderr_tail = secret.as_bytes().get(split..).assert_value().to_vec();
    output
        .stderr_tail
        .extend_from_slice(b": actionable root cause");
    provider_failure_diagnostic("Claude", None, Some(&output), &[secret.to_owned()])
}

#[test]
fn driver_context_upgrades_only_payload_free_failures() {
    assert_eq!(
        with_driver_detail(NodeRunnerError::Driver, "invalid provider configuration"),
        NodeRunnerError::DriverDetail("invalid provider configuration".to_owned())
    );
    assert_eq!(
        with_driver_detail(NodeRunnerError::Cancelled, "unused"),
        NodeRunnerError::Cancelled
    );
    assert_eq!(
        with_driver_detail(
            NodeRunnerError::DriverDetail("specific".to_owned()),
            "coarse"
        ),
        NodeRunnerError::DriverDetail("specific".to_owned())
    );
}

#[test]
fn cleanup_confirmation_is_required_even_for_cancelled_processes() {
    for cancelled in [false, true] {
        let mut output = process_output();
        output.cancelled = cancelled;
        assert!(require_process_cleanup(&Ok(output.clone())).is_ok());
        output.cleanup = crate::execution::process::ProcessCleanupEvidence::TimedOut;
        assert!(matches!(
            require_process_cleanup(&Ok(output)),
            Err(NodeRunnerError::CleanupUnconfirmed)
        ));
    }
    assert!(matches!(
        require_process_cleanup(&Err(ProcessRunnerError::Io("lost completion".to_owned()))),
        Err(NodeRunnerError::CleanupUnconfirmed)
    ));
}

#[test]
fn boundary_contract_every_reasoning_effort_has_a_stable_provider_token() {
    assert_eq!(effort_token(ReasoningEffort::Low), "low");
    assert_eq!(effort_token(ReasoningEffort::Medium), "medium");
    assert_eq!(effort_token(ReasoningEffort::High), "high");
    assert_eq!(effort_token(ReasoningEffort::Xhigh), "xhigh");
    assert_eq!(effort_token(ReasoningEffort::Max), "max");
}
