use super::*;

fn run_id(value: &str) -> RunId {
    RunId::new(value)
}

fn diagnostic(run_id: RunId, text: String) -> NewOperatorDiagnostic {
    NewOperatorDiagnostic {
        run_id,
        code: "git_push_failed",
        operation: "delivery.git_push",
        exit_status: Some(128),
        stdout: text.clone(),
        stderr: text,
        stdout_truncated: false,
        stderr_truncated: false,
    }
}

#[test]
fn snapshots_are_run_scoped_and_the_global_queue_is_bounded() {
    let store = OperatorDiagnosticStore::default();
    let requested = run_id("018f5e78-7f95-7c22-8d98-3f15af20c991");
    let other = run_id("018f5e78-7f95-7c22-8d98-3f15af20c992");
    store.record(diagnostic(requested.clone(), "evicted".to_owned()));
    store.record(diagnostic(other, "other".to_owned()));
    store.record(diagnostic(requested.clone(), "retained".to_owned()));

    let snapshot = store.snapshot(&requested);

    assert_eq!(snapshot.diagnostics.len(), 1);
    assert_eq!(snapshot.diagnostics[0].id, "3");
    assert_eq!(snapshot.diagnostics[0].stdout, "retained");
    let debug = format!("{store:?}");
    assert!(!debug.contains("retained"));
}

#[test]
fn worst_case_snapshot_stays_below_the_transport_response_limit() {
    let store = OperatorDiagnosticStore::default();
    let requested = run_id("018f5e78-7f95-7c22-8d98-3f15af20c991");
    let escaped = "\n".repeat(MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES);
    store.record(diagnostic(requested.clone(), escaped.clone()));
    store.record(diagnostic(requested.clone(), escaped));

    let encoded = serde_json::to_vec(&store.snapshot(&requested)).expect("serialize snapshot");

    assert!(encoded.len() < 64 * 1024, "{} bytes", encoded.len());
}
