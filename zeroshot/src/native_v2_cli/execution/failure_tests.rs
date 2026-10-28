use std::time::Duration;

use openengine_cluster_protocol::EnumLabel;

use super::*;
use crate::native_v2_cli::tests::support::FakeBackend;

#[tokio::test(start_paused = true)]
async fn error_log_read_ends_at_its_deadline_or_ctrl_c() {
    for (detach_after, waited) in [(60, 5), (2, 2)] {
        let backend = FakeBackend::with_pending_logs();
        let detach = tokio::time::sleep(Duration::from_secs(detach_after));
        tokio::pin!(detach);
        let started = tokio::time::Instant::now();

        let failure =
            RunFailure::read(&backend, None, RunId::new("run-local"), detach.as_mut()).await;

        assert_eq!(started.elapsed(), Duration::from_secs(waited));
        assert_eq!(
            failure.to_string(),
            "run finished unsuccessfully\nThe run log could not be read completely.\n\
             Full run log: zeroshot logs run-local"
        );
    }
}

#[test]
fn foreground_failure_uses_structured_evidence_without_attributing_recorded_errors() {
    let context = serde_json::from_value(serde_json::json!({
        "primary": {"message": "Runtime cleanup failed"},
        "recorded": [{
            "message": "GitHub API request failed: base branch not found",
            "execution": {"execution": "v2:execution:1", "node": "deliver"},
            "cursor": "v2:4"
        }],
        "incomplete": true
    }))
    .expect("valid failure context");
    let failure = RunFailure::from_context(
        RunId::new("run-local"),
        Some("platform"),
        (
            openengine_cluster_protocol::EnumLabel::new("runtime_failed").expect("reason"),
            context,
        ),
    );
    assert_eq!(
        failure.to_string(),
        "run finished unsuccessfully: runtime_failed\nFailure:\n  Runtime cleanup failed\n\
         Recorded failures:\n  deliver: GitHub API request failed: base branch not found (log v2:4)\n\
         Additional failure details may be missing; see the full run log.\n\
         Full run log: zeroshot logs run-local --target platform"
    );
}

#[test]
fn foreground_watch_keeps_the_terminal_context_and_ordinary_json_output() {
    let event: crate::native_v2_cli::CliRunWatchEventNotification =
        serde_json::from_value(serde_json::json!({
            "runId": "run-local",
            "title": "Delivery failure",
            "size": "small",
            "subscriptionId": "sub-1",
            "cursor": "v2:4",
            "status": {
                "phase": "finished",
                "terminalResult": {
                    "status": "failed",
                    "reason": "delivery_failed",
                    "context": {
                        "recorded": [{"message": "base branch not found", "cursor": "v2:3"}],
                        "incomplete": false
                    }
                },
                "metadata": {}
            }
        }))
        .expect("valid failed watch event");
    let mut bytes = Vec::new();
    let mut output = failure::FailureOutput::new(&mut bytes);
    output
        .watch_event(event.clone())
        .expect("write watch event");
    let (reason, context) = output.context.expect("captured terminal context");
    assert_eq!(reason.as_str(), "delivery_failed");
    assert!(context.primary.is_none());
    assert_eq!(
        context.recorded[0].message.as_str(),
        "base branch not found"
    );
    let written: serde_json::Value = serde_json::from_slice(&bytes).expect("watch JSON");
    assert_eq!(
        written,
        serde_json::to_value(event).expect("expected watch JSON")
    );
}

fn error_log(cursor: &str, level: &str, message: &str) -> RunLogEventNotification {
    serde_json::from_value(serde_json::json!({
        "subscriptionId": "sub-logs",
        "runId": "run-local",
        "cursor": cursor,
        "timestamp": 1_700_000_000_000_u64,
        "record": {
            "level": level,
            "target": "worker",
            "message": message
        }
    }))
    .expect("valid log event")
}

#[test]
fn failure_report_keeps_only_the_latest_bounded_error_records() {
    let mut failure = RunFailure::read_for_test(RunId::new("run-local"), None);
    for index in 0..7 {
        failure
            .log_event(error_log(
                &format!("v2:{index}"),
                "error",
                &format!("error {index}"),
            ))
            .expect("record error");
        failure
            .log_event(error_log(&format!("v2:info{index}"), "info", "ignored"))
            .expect("ignore non-errors");
    }
    let report = failure.to_string();
    assert!(report.contains("run finished unsuccessfully"));
    assert!(report.contains("Latest 5 of 7 error logs:"));
    assert!(!report.contains("error 1\n"));
    assert!(report.contains("  error 6"));
    assert!(report.contains("Full run log: zeroshot logs run-local"));
}

#[test]
fn failure_report_without_context_names_latest_retained_errors_and_target() {
    let mut failure = RunFailure::read_for_test(RunId::new("run-local"), Some("cloud"));
    for index in 0..7 {
        failure
            .log_event(error_log(
                &format!("v2:{index}"),
                "error",
                &format!("error {index}"),
            ))
            .expect("record error");
    }
    assert_eq!(
        failure.to_string(),
        "run finished unsuccessfully\nLatest 5 of 7 error logs:\n  error 2\n  error 3\n  error 4\n  error 5\n  error 6\nThe run log could not be read completely.\nFull run log: zeroshot logs run-local --target cloud"
    );

    let no_errors = RunFailure::from_context(
        RunId::new("run-local"),
        None,
        (
            EnumLabel::new("delivery_failed").expect("reason"),
            serde_json::from_value(serde_json::json!({"incomplete": false}))
                .expect("empty failure context"),
        ),
    );
    assert!(
        no_errors
            .to_string()
            .contains("run finished unsuccessfully: delivery_failed")
    );
}
