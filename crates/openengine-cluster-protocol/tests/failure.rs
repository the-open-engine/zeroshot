use openengine_cluster_protocol::{
    BoundedLogMessage, FailureContext, FailureDiagnostic, TerminalResult, MAX_RECORDED_FAILURES,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

fn diagnostic(message: &str) -> FailureDiagnostic {
    FailureDiagnostic {
        message: BoundedLogMessage::new(message).assert_value(),
        execution: None,
        cursor: None,
    }
}

#[test]
fn failed_terminal_reads_old_data_and_round_trips_public_safe_context() {
    let old: TerminalResult =
        serde_json::from_value(json!({"status":"failed","reason":"runtime_failed"})).assert_value();
    assert!(matches!(old, TerminalResult::Failed { context: None, .. }));
    let context = FailureContext {
        primary: Some(diagnostic("workspace snapshot failed")),
        ..FailureContext::default()
    };
    let failed = TerminalResult::Failed {
        reason: "runtime_failed".parse().assert_value(),
        context: Some(Box::new(context)),
    };
    assert_eq!(
        serde_json::from_value::<TerminalResult>(serde_json::to_value(&failed).assert_value())
            .assert_value(),
        failed
    );
    assert!(
        serde_json::from_value::<FailureDiagnostic>(json!({"message":"unsafe\u{001b}control"}))
            .is_err()
    );
}

#[test]
fn failure_context_bounds_both_wire_directions_and_reports_omitted_evidence() {
    let mut context = FailureContext::default();
    for index in 0..MAX_RECORDED_FAILURES + 2 {
        context.record(diagnostic(&format!("failure {index}")));
    }
    assert!(context.incomplete);
    assert_eq!(context.recorded.len(), MAX_RECORDED_FAILURES);
    assert_eq!(context.recorded[0].message.as_str(), "failure 2");
    let mut wire = serde_json::to_value(&context).assert_value();
    wire["recorded"]
        .as_array_mut()
        .assert_value()
        .push(json!({"message":"one too many"}));
    assert!(serde_json::from_value::<FailureContext>(wire).is_err());
    context.recorded.push(diagnostic("one too many"));
    assert!(serde_json::to_value(context).is_err());
}
