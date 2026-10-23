use std::time::Duration;

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
