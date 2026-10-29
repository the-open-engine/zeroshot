//! Lifecycle tests: the real Pi adapter driving a scripted `pi` executable.
//!
//! Every other test in this module stops at argv and transcript construction, so these are the only
//! ones that prepare the private filesystem, open a provider process, stream the prompt into it,
//! and settle a turn from its output. A shell script stands in for the CLI, so no network, login,
//! or installed `pi` is needed.
//!
//! A child module of `native_v2_pi/tests.rs`, so the shared fixtures stay in one place and the
//! coverage totals keep excluding this file (`AGENTS.md`).

use std::collections::BTreeMap;
use std::time::Duration;

use openengine_cluster_protocol::WorkerOutcome;
use openengine_cluster_testkit::assertions::AssertValue;

use super::*;
use crate::native_v2_runner::test_support::control;
use crate::native_v2_runner::{LiveOutput, NodeDriver, NodeRunnerError};

/// A `pi --mode json` turn: the prompt is consumed and each line is framed as the CLI frames it.
pub(super) fn script(lines: &[&str]) -> String {
    script_exiting(0, lines)
}

fn script_exiting(code: u8, lines: &[&str]) -> String {
    let mut body = String::from("#!/bin/sh\ncat > /dev/null\n");
    for line in lines {
        body.push_str(&format!("printf '%s\\n' '{line}'\n"));
    }
    body.push_str(&format!("exit {code}\n"));
    body
}

const NARRATION: &str = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"working"}}"#;
const FAILED_TURN: &str = r#"{"type":"message_end","message":{"role":"assistant","stopReason":"error","errorMessage":"provider rejected the configured model","content":[]}}"#;
const COMPLETED_TURN: &str = r#"{"type":"message_end","message":{"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"{\"response\":null}"}]}}"#;

/// The anthropic lane reuses a local login, so this needs no declared credential and keeps the
/// neutral schema dialect for the completion case.
fn scripted_invocation() -> DriverInvocation {
    invocation(
        agent_binding("claude-sonnet-4-5", None, SessionScope::Execution, &[]),
        NodeRole::Worker,
        resolved(&[], &[]),
    )
}

/// A local adapter whose Pi executable is the supplied script, over real temporary directories.
pub(super) fn scripted_adapter(directory: &TestDirectory, body: &str) -> PiAdapter {
    let executable = directory.write_executable("pi", body);
    adapter_for_executable(directory, executable.to_str().assert_value())
}

fn adapter_for_executable(directory: &TestDirectory, executable: &str) -> PiAdapter {
    let workspace = directory.child("workspace");
    let runtime_home = directory.child("runtime");
    std::fs::create_dir_all(&workspace).assert_value();
    std::fs::create_dir_all(&runtime_home).assert_value();
    let mut configuration =
        adapter_configuration(PiProvider::Anthropic, executable, BTreeMap::new());
    configuration.workspace = workspace;
    configuration.runtime_home = runtime_home;
    PiAdapter::new_local(configuration).assert_value()
}

/// Runs one turn, draining the durable output as it arrives so the bounded channel cannot block.
pub(super) async fn run_turn(
    adapter: &PiAdapter,
    cancelled_first: bool,
) -> (Result<WorkerOutcome, NodeRunnerError>, Vec<LiveOutput>) {
    run_invocation(adapter, scripted_invocation(), cancelled_first).await
}

/// Runs one turn for a caller-supplied invocation. A cancelled control is signalled before the turn
/// starts, which is how the cancellation paths are reached deterministically.
pub(super) async fn run_invocation(
    adapter: &PiAdapter,
    invocation: DriverInvocation,
    cancelled_first: bool,
) -> (Result<WorkerOutcome, NodeRunnerError>, Vec<LiveOutput>) {
    let (cancel, control, mut durable) = control();
    if cancelled_first {
        cancel.send(true).assert_value();
    }
    let drain = tokio::spawn(async move {
        let mut seen = Vec::new();
        while let Ok(output) = durable.recv_output().await {
            seen.push(output);
        }
        seen
    });
    let outcome = tokio::time::timeout(Duration::from_secs(60), adapter.run(invocation, control))
        .await
        .assert_value_with("a scripted Pi turn must finish");
    drop(cancel);
    let seen = drain.await.assert_value();
    (outcome, seen)
}

#[tokio::test]
async fn a_scripted_turn_reports_the_failure_the_event_stream_carries() {
    // Pi exits zero even when its retries are exhausted, so only the stream carries the reason.
    let directory = TestDirectory::new("pi-scripted-failure");
    let adapter = scripted_adapter(&directory, &script(&[NARRATION, FAILED_TURN]));
    let (outcome, seen) = run_turn(&adapter, false).await;
    let Err(NodeRunnerError::DriverDetail(detail)) = outcome else {
        panic!("expected a provider failure detail, got {outcome:?}");
    };
    assert!(
        detail.contains("provider rejected the configured model"),
        "{detail}"
    );
    assert!(
        seen.iter().any(|output| output.text.contains("working")),
        "the streamed narration must reach the durable log: {seen:?}"
    );
}

#[tokio::test]
async fn a_scripted_turn_completes_from_its_final_assistant_message() {
    let directory = TestDirectory::new("pi-scripted-complete");
    let adapter = scripted_adapter(&directory, &script(&[NARRATION, COMPLETED_TURN]));
    let (outcome, seen) = run_turn(&adapter, false).await;
    assert!(outcome.is_ok(), "{outcome:?}");
    assert!(
        seen.iter().any(|output| output.text.contains("working")),
        "{seen:?}"
    );
}

#[tokio::test]
async fn a_cancelled_turn_never_starts_the_provider() {
    let directory = TestDirectory::new("pi-scripted-cancelled");
    let adapter = scripted_adapter(&directory, &script(&[COMPLETED_TURN]));
    let (outcome, _seen) = run_turn(&adapter, true).await;
    assert!(
        matches!(outcome, Err(NodeRunnerError::Cancelled)),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn an_unlaunchable_provider_reports_a_start_failure() {
    let directory = TestDirectory::new("pi-scripted-missing");
    let adapter = adapter_for_executable(&directory, "/nonexistent/pi-binary");
    let (outcome, _seen) = run_turn(&adapter, false).await;
    let Err(NodeRunnerError::DriverDetail(detail)) = outcome else {
        panic!("expected a launch failure detail, got {outcome:?}");
    };
    assert!(!detail.is_empty(), "{detail}");
}

#[tokio::test]
async fn a_provider_that_exits_nonzero_after_an_answer_still_fails_the_turn() {
    // A process that failed after producing an answer must not be treated as usable.
    let directory = TestDirectory::new("pi-scripted-late-exit");
    let adapter = scripted_adapter(&directory, &script_exiting(9, &[COMPLETED_TURN]));
    let (outcome, _seen) = run_turn(&adapter, false).await;
    let Err(NodeRunnerError::DriverDetail(detail)) = outcome else {
        panic!("expected a process failure, got {outcome:?}");
    };
    assert!(!detail.is_empty(), "{detail}");
}

#[tokio::test]
async fn a_provider_that_exits_nonzero_without_a_terminal_reports_the_exit_status() {
    // No terminal message and no provider error: the exit status is all the turn has, and reporting
    // it is what keeps the node from silently settling on nothing.
    let directory = TestDirectory::new("pi-scripted-no-terminal");
    let adapter = scripted_adapter(&directory, &script_exiting(4, &[]));
    let (outcome, _seen) = run_turn(&adapter, false).await;
    let Err(NodeRunnerError::DriverDetail(detail)) = outcome else {
        panic!("expected a provider failure, got {outcome:?}");
    };
    assert!(detail.contains("exited with status 4"), "{detail}");
}

#[tokio::test]
async fn a_turn_cancelled_while_the_provider_runs_settles_as_cancelled() {
    // The script announces that it is running before it sleeps, so the cancel provably lands on a
    // live process: cancelling before startup would take the other, already-covered path and the
    // test would pass without exercising a running provider or its cleanup.
    let directory = TestDirectory::new("pi-scripted-mid-turn-cancel");
    let marker = directory.child("pi-running");
    let body = format!(
        "#!/bin/sh\ncat > /dev/null &\n: > '{marker}'\nsleep 15\nexit 0\n",
        marker = marker.display()
    );
    let adapter = std::sync::Arc::new(scripted_adapter(&directory, &body));
    let (cancel, control, mut durable) = control();
    let drain = tokio::spawn(async move {
        let mut seen = Vec::new();
        while let Ok(output) = durable.recv_output().await {
            seen.push(output);
        }
        seen
    });
    let run = {
        let adapter = adapter.clone();
        tokio::spawn(async move { adapter.run(scripted_invocation(), control).await })
    };
    let mut running = false;
    for _ in 0..500 {
        if marker.exists() {
            running = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(running, "the scripted provider never started");
    cancel.send(true).assert_value();
    let outcome = tokio::time::timeout(Duration::from_secs(60), run)
        .await
        .assert_value_with("a cancelled Pi turn must still finish")
        .assert_value();
    let _seen = drain.await.assert_value();
    assert!(
        matches!(outcome, Err(NodeRunnerError::Cancelled)),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_lost_output_consumer_fails_the_turn_instead_of_hanging_it() {
    // The durable log is bounded, so a consumer that disappears must surface as a failure that
    // names the closed output, never as a turn that waits forever on a full channel.
    let directory = TestDirectory::new("pi-scripted-lost-consumer");
    let adapter = scripted_adapter(&directory, &script(&[NARRATION, COMPLETED_TURN]));
    let (_cancel, control, durable) = control();
    drop(durable);
    let outcome = tokio::time::timeout(
        Duration::from_secs(60),
        adapter.run(scripted_invocation(), control),
    )
    .await
    .assert_value_with("a turn with no output consumer must still finish");
    assert!(
        matches!(outcome, Err(NodeRunnerError::DurableOutputClosed)),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn records_that_are_not_events_do_not_hide_the_terminal_message() {
    let directory = TestDirectory::new("pi-scripted-malformed");
    let adapter = scripted_adapter(
        &directory,
        &script(&["not json at all", NARRATION, FAILED_TURN]),
    );
    let (outcome, _seen) = run_turn(&adapter, false).await;
    let Err(NodeRunnerError::DriverDetail(detail)) = outcome else {
        panic!("expected the streamed failure, got {outcome:?}");
    };
    assert!(
        detail.contains("provider rejected the configured model"),
        "{detail}"
    );
}

/// A rejected provider envelope for a contract that requires a string response.
const REJECTED_ENVELOPE: &str = r#"{"type":"message_end","message":{"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"not a provider envelope"}]}}"#;
/// The same contract answered correctly on the correction turn.
const STRING_ENVELOPE: &str = r#"{"type":"message_end","message":{"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"{\"response\":\"corrected\"}"}]}}"#;
const FOREIGN_SESSION: &str = r#"{"type":"session","id":"pi-not-the-derived-session"}"#;

/// An adapter whose private provider home cannot be created, so filesystem preparation fails before
/// any process is launched.
fn adapter_with_unpreparable_home(directory: &TestDirectory) -> PiAdapter {
    let executable = directory.write_executable("pi", &script(&[COMPLETED_TURN]));
    let workspace = directory.child("workspace");
    std::fs::create_dir_all(&workspace).assert_value();
    let runtime_home = directory.child("runtime-is-a-file");
    std::fs::write(&runtime_home, b"not a directory").assert_value();
    let mut configuration = adapter_configuration(
        PiProvider::Anthropic,
        executable.to_str().assert_value(),
        BTreeMap::new(),
    );
    configuration.workspace = workspace;
    configuration.runtime_home = runtime_home;
    PiAdapter::new_local(configuration).assert_value()
}

#[tokio::test]
async fn a_provider_that_never_reads_its_stdin_reports_the_input_failure() {
    // A native error must not hide an incomplete prompt or authorize continuation.
    for native_failure in ["", FAILED_TURN] {
        let directory = TestDirectory::new("pi-scripted-input-failure");
        let marker = directory.child("starts");
        let body = format!(
            "#!/bin/sh\nprintf 'started\n' >> '{marker}'\nprintf '%s\n' '{native_failure}'\nexec 0<&-\nsleep 30\n",
            marker = marker.display()
        );
        let adapter = scripted_adapter(&directory, &body);
        let mut turn = scripted_invocation();
        turn.node.input = serde_json::Value::String("x".repeat(8 * 1024 * 1024));
        let (outcome, seen) = run_invocation(&adapter, turn, false).await;
        let Err(NodeRunnerError::DriverDetail(detail)) = outcome else {
            panic!("expected an input failure detail, got {outcome:?}");
        };
        assert!(detail.contains("provider process input failed"), "{detail}");
        if !native_failure.is_empty() {
            assert!(
                detail.contains("provider rejected the configured model"),
                "{detail}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(marker)
                .assert_value()
                .lines()
                .count(),
            1
        );
        assert!(
            !seen
                .iter()
                .any(|output| output.text.contains("continuing once"))
        );
    }
}

#[tokio::test]
async fn a_completed_turn_reporting_a_foreign_session_fails_the_turn() {
    // Pi creates the exact session ID Zeroshot derived. A different one would silently split the
    // logical node session, so the mismatch is a failure even though the answer parsed.
    let directory = TestDirectory::new("pi-scripted-foreign-session");
    let adapter = scripted_adapter(&directory, &script(&[FOREIGN_SESSION, COMPLETED_TURN]));
    let (outcome, _seen) = run_turn(&adapter, false).await;
    let Err(NodeRunnerError::DriverDetail(detail)) = outcome else {
        panic!("expected a session mismatch detail, got {outcome:?}");
    };
    assert!(detail.contains("different session"), "{detail}");
}

#[tokio::test]
async fn a_failed_turn_reporting_a_foreign_session_fails_before_the_failure_settles() {
    // The same identity check runs on the failure path, so a provider failure cannot hide a
    // session split.
    let directory = TestDirectory::new("pi-scripted-foreign-session-failed");
    let adapter = scripted_adapter(&directory, &script(&[FOREIGN_SESSION, FAILED_TURN]));
    let (outcome, _seen) = run_turn(&adapter, false).await;
    let Err(NodeRunnerError::DriverDetail(detail)) = outcome else {
        panic!("expected a session mismatch detail, got {outcome:?}");
    };
    assert!(detail.contains("different session"), "{detail}");
}

#[tokio::test]
async fn a_rejected_response_is_corrected_in_the_same_session() {
    // Pi has no response-schema flag, so a mechanically rejected answer is corrected by another
    // turn in the same session until it parses. The correction must be observable.
    let directory = TestDirectory::new("pi-scripted-correction");
    let body = format!(
        "#!/bin/sh\ncat > /dev/null\nif [ -e first ]; then\n  printf '%s\\n' '{later}'\n  exit 0\nfi\n: > first\nprintf '%s\\n' '{first}'\nexit 0\n",
        first = REJECTED_ENVELOPE,
        later = STRING_ENVELOPE,
    );
    let adapter = scripted_adapter(&directory, &body);
    let mut turn = scripted_invocation();
    turn.response = NodeResponseContract::Worker {
        output: PayloadType::String,
    };
    let (outcome, seen) = run_invocation(&adapter, turn, false).await;
    assert!(outcome.is_ok(), "{outcome:?}");
    assert!(
        seen.iter()
            .any(|output| output.text.contains("requesting correction")),
        "the rejected answer must be reported as a correction: {seen:?}"
    );
    assert!(
        directory.child("workspace").join("first").exists(),
        "a second provider turn must carry the correction prompt"
    );
}

#[tokio::test]
async fn a_filesystem_that_cannot_be_prepared_fails_before_launch() {
    let directory = TestDirectory::new("pi-scripted-prepare-failure");
    let adapter = adapter_with_unpreparable_home(&directory);
    let (outcome, _seen) = run_turn(&adapter, false).await;
    let Err(NodeRunnerError::DriverDetail(detail)) = outcome else {
        panic!("expected a preparation failure detail, got {outcome:?}");
    };
    assert!(detail.contains("private home"), "{detail}");
}

#[tokio::test]
async fn a_cancelled_turn_whose_filesystem_cannot_be_prepared_settles_as_cancelled() {
    // Cancellation wins over a preparation failure, so an interrupted run is never reported as a
    // provider error.
    let directory = TestDirectory::new("pi-scripted-prepare-cancelled");
    let adapter = adapter_with_unpreparable_home(&directory);
    let (outcome, _seen) = run_turn(&adapter, true).await;
    assert!(
        matches!(outcome, Err(NodeRunnerError::Cancelled)),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_run_without_an_agent_binding_fails_before_dispatch() {
    // Session identity is derived from the agent binding, so a delivery binding must fail before
    // any provider process starts.
    let directory = TestDirectory::new("pi-run-delivery-binding");
    let adapter = scripted_adapter(&directory, &script(&[COMPLETED_TURN]));
    let turn = invocation(delivery_binding(), NodeRole::Worker, resolved(&[], &[]));
    let (outcome, _seen) = run_invocation(&adapter, turn, false).await;
    assert!(
        matches!(outcome, Err(NodeRunnerError::Driver)),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_gateway_run_requires_its_authored_wire_protocol() {
    // The wire protocol is authored, never inferred from the opaque model identifier, so a gateway
    // lane that omits it must fail before any provider process starts.
    let directory = TestDirectory::new("pi-gateway-no-protocol");
    let executable = directory.write_executable("pi", &script(&[COMPLETED_TURN]));
    let workspace = directory.child("workspace");
    let runtime_home = directory.child("runtime");
    std::fs::create_dir_all(&workspace).assert_value();
    std::fs::create_dir_all(&runtime_home).assert_value();
    let mut configuration = adapter_configuration(
        PiProvider::Gateway,
        executable.to_str().assert_value(),
        BTreeMap::new(),
    );
    configuration.workspace = workspace;
    configuration.runtime_home = runtime_home;
    let adapter = PiAdapter::new_local(configuration).assert_value();
    let fields = ["GATEWAY_BASE_URL", "GATEWAY_API_KEY"];
    let turn = invocation(
        agent_binding(
            "provider-owned-model",
            None,
            SessionScope::Execution,
            &fields,
        ),
        NodeRole::Worker,
        resolved(
            &fields,
            &[
                ("GATEWAY_BASE_URL", "https://gateway.example/v1"),
                ("GATEWAY_API_KEY", "declared"),
            ],
        ),
    );
    let (outcome, _seen) = run_invocation(&adapter, turn, false).await;
    assert!(
        matches!(outcome, Err(NodeRunnerError::DriverDetail(_))),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_run_without_authored_instructions_fails_before_dispatch() {
    let directory = TestDirectory::new("pi-run-no-instructions");
    let adapter = scripted_adapter(&directory, &script(&[COMPLETED_TURN]));
    let mut turn = scripted_invocation();
    turn.node.instructions = None;
    let (outcome, _seen) = run_invocation(&adapter, turn, false).await;
    assert!(
        matches!(outcome, Err(NodeRunnerError::InvalidRole)),
        "{outcome:?}"
    );
}

#[test]
fn cleanup_preview_keeps_only_redacted_failure_evidence() {
    use crate::native_v2_capsule::provider_process::turn::ProviderTranscript;
    let mut transcript = crate::native_v2_pi::PiTranscript::new(vec!["private-key".to_owned()]);
    let record = serde_json::json!({"type":"message_end", "message":{"role":"assistant",
        "stopReason":"error", "errorMessage":"native rejection private-key"}})
    .to_string()
        + "\n";
    transcript.push(record.as_bytes());
    let diagnostic = transcript
        .failure_diagnostic(Some("stdin failed private-key"))
        .unwrap();
    assert_eq!(
        diagnostic,
        "Pi provider failure: native rejection [REDACTED]; stdin failed [REDACTED]"
    );
    let mut transcript = crate::native_v2_pi::PiTranscript::new(Vec::new());
    transcript.push(COMPLETED_TURN.as_bytes());
    transcript.finish_stream();
    assert!(transcript.failure_diagnostic(None).is_none());
}

#[tokio::test]
async fn exhausted_corrections_publish_only_the_last_validation_error_as_failure() {
    let directory = TestDirectory::new("pi-exhausted-correction");
    let adapter = scripted_adapter(&directory, &script(&[REJECTED_ENVELOPE]));
    let (outcome, seen) = run_turn(&adapter, false).await;
    assert_eq!(outcome.unwrap(), WorkerOutcome::malformed());
    let failures = seen
        .iter()
        .filter(|output| {
            output.diagnostic == Some(openengine_cluster_protocol::FailureDiagnosticKind::Failure)
        })
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 1);
    assert!(failures[0].text.contains("final output rejected:"));
}
