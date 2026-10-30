use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;
use crate::execution::process::{ProcessCleanupEvidence, ProcessLaunchEvidence, ProcessSessionOutput};

#[test]
fn coverage_contract_rpc_request_ids_are_bounded_and_never_reused_after_overflow() {
    let mut next = 0;
    assert_eq!(reserve_request_id(&mut next, 0).assert_value(), 1);
    assert!(
        reserve_request_id(&mut next, 128)
            .assert_error()
            .to_string()
            .contains("pending request limit")
    );
    next = u64::MAX;
    assert!(
        reserve_request_id(&mut next, 0)
            .assert_error()
            .to_string()
            .contains("identifier overflow")
    );
    assert!(
        process_error(ProcessRunnerError::Launch("safe".to_owned()))
            .to_string()
            .contains("Copilot process failed")
    );
}

#[test]
fn grammar_compilation_rate_limit_classification_is_narrow_and_case_insensitive() {
    assert!(is_grammar_compilation_rate_limit(
        "400 Grammar compilation rate limit exceeded. You can try again later."
    ));
    assert!(is_grammar_compilation_rate_limit(
        "GRAMMAR COMPILATION RATE LIMIT EXCEEDED"
    ));
    assert!(!is_grammar_compilation_rate_limit(
        "HTTP 400: Invalid JSON schema"
    ));
    assert!(!is_grammar_compilation_rate_limit(
        "generic rate limit exceeded"
    ));
}

#[test]
fn grammar_compilation_retry_delay_is_exponential_bounded_and_session_staggered() {
    let first = grammar_rate_limit_retry_delay(0, "session-0");
    assert!((Duration::from_secs(1)..Duration::from_secs(2)).contains(&first));
    assert_eq!(first, grammar_rate_limit_retry_delay(0, "session-0"));

    let second = grammar_rate_limit_retry_delay(1, "session-0");
    assert!((Duration::from_secs(2)..Duration::from_secs(3)).contains(&second));
    let third = grammar_rate_limit_retry_delay(2, "session-0");
    assert!((Duration::from_secs(4)..Duration::from_secs(5)).contains(&third));
    let capped = grammar_rate_limit_retry_delay(usize::MAX, "session-0");
    assert!((Duration::from_secs(8)..Duration::from_secs(9)).contains(&capped));

    let staggered = (0..16)
        .map(|index| grammar_rate_limit_retry_delay(0, &format!("session-{index}")))
        .collect::<BTreeSet<_>>();
    assert!(
        staggered.len() > 1,
        "parallel sessions should not share one delay"
    );
}

#[test]
fn coverage_contract_rpc_responses_validate_identity_error_shape_and_result_presence() {
    let mut pending = BTreeSet::from([1, 2, 3]);
    assert!(resolve_response_message(&mut pending, &json!({}), 1).is_err());
    assert!(resolve_response_message(&mut pending, &json!({"id": 9, "result": {}}), 1).is_err());
    assert!(
        resolve_response_message(
            &mut pending,
            &json!({"id": 1, "error": {"message": "refused"}, "result": {}}),
            1,
        )
        .assert_error()
        .to_string()
        .contains("exactly one result or error")
    );
    assert!(resolve_response_message(&mut pending, &json!({"id": 2}), 2).is_err());
    assert_eq!(
        resolve_response_message(&mut pending, &json!({"id": 3, "result": {"ok": true}}), 4)
            .assert_value(),
        None
    );
    assert!(pending.is_empty());

    let mut expected = BTreeSet::from([7]);
    assert_eq!(
        resolve_response_message(&mut expected, &json!({"id": 7, "result": {"ok": true}}), 7,)
            .assert_value(),
        Some(RpcReply::Result(json!({"ok": true})))
    );

    let mut refused = BTreeSet::from([8]);
    assert_eq!(
        resolve_response_message(
            &mut refused,
            &json!({"id": 8, "error": {"code": -32601, "message": "refused"}}),
            8,
        )
        .assert_value(),
        Some(RpcReply::Error("refused".to_owned()))
    );
    for error in [json!({"message": "refused"}), json!({"code": -32601})] {
        assert!(response_payload(&json!({"error": error})).is_err());
    }
}

#[test]
fn coverage_contract_rpc_stderr_diagnostics_mark_truncated_and_complete_tails() {
    for (truncated, marker) in [
        (false, "; stderr: provider failed"),
        (true, "; stderr (truncated tail): provider failed"),
    ] {
        let mut detail = "execution failed".to_owned();
        append_stderr_detail(
            &mut detail,
            &ProcessSessionOutput {
                launch_evidence: ProcessLaunchEvidence::MayHaveStarted,
                exit_code: Some(1),
                termination_signal: None,
                core_dumped: false,
                stderr_tail: b"provider failed".to_vec(),
                stderr_tail_truncated: truncated,
                cancelled: false,
                timed_out: false,
                cleanup: ProcessCleanupEvidence::Reaped,
                post_launch_error: None,
            },
        );
        assert!(detail.contains(marker));
    }
    let mut unchanged = "execution failed".to_owned();
    append_stderr_detail(
        &mut unchanged,
        &ProcessSessionOutput {
            launch_evidence: ProcessLaunchEvidence::MayHaveStarted,
            exit_code: Some(1),
            termination_signal: None,
            core_dumped: false,
            stderr_tail: Vec::new(),
            stderr_tail_truncated: false,
            cancelled: false,
            timed_out: false,
            cleanup: ProcessCleanupEvidence::Reaped,
            post_launch_error: None,
        },
    );
    assert_eq!(unchanged, "execution failed");
}

#[test]
fn coverage_contract_rpc_protocol_session_and_dispatch_decisions_fail_closed() {
    validate_protocol_version(&json!({"protocolVersion": 3})).assert_value();
    for value in [
        json!({}),
        json!({"protocolVersion": 2}),
        json!({"protocolVersion": "3"}),
    ] {
        assert!(validate_protocol_version(&value).is_err());
    }
    validate_session_identity(&json!({"sessionId": "expected"}), "expected").assert_value();
    assert!(validate_session_identity(&json!({}), "expected").is_err());
    assert!(validate_session_identity(&json!({"sessionId": "other"}), "expected").is_err());
    assert!(validate_detach(&json!({"success": true})).assert_value());
    assert!(!validate_detach(&json!({"success": false})).assert_value());
    for result in [json!({}), json!({"success": "true"}), json!(null)] {
        assert!(validate_detach(&result).is_err());
    }

    for (message, expected) in [
        (json!({"method": "session.event"}), DispatchAction::Event),
        (
            json!({"method": "gitHubToken.getToken", "id": 1}),
            DispatchAction::AcquireToken,
        ),
        (
            json!({"method": "future.method", "id": 2}),
            DispatchAction::Reject,
        ),
        (
            json!({"method": "future.notification"}),
            DispatchAction::Ignore,
        ),
    ] {
        assert_eq!(dispatch_action(&message), expected);
    }
    assert!(request_ended("connect").to_string().contains("connect"));
    assert_eq!(
        failure_detail(NodeRunnerError::Driver).assert_value(),
        "execution failed"
    );
    assert_eq!(
        failure_detail(NodeRunnerError::DriverDetail("safe".to_owned())).assert_value(),
        "safe"
    );
    assert!(matches!(
        failure_detail(NodeRunnerError::Cancelled),
        Err(NodeRunnerError::Cancelled)
    ));
}
