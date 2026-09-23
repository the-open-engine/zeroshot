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
fn coverage_contract_rpc_responses_validate_identity_error_precedence_and_result_presence() {
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
        .contains("refused")
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
        Some(json!({"ok": true}))
    );
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
    validate_detach(&json!({"success": true})).assert_value();
    assert!(validate_detach(&json!({"success": false})).is_err());

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
