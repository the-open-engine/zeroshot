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

#[test]
fn output_preserves_records_after_private_snapshot_eviction() {
    let (output, mut receiver) = OperatorDiagnosticOutput::channel();
    let store = OperatorDiagnosticStore::new(Some(output.clone()));
    let requested = run_id("requested");
    let other = run_id("other");
    for run in [requested.clone(), other, requested.clone()] {
        store.record(diagnostic(run, "private diagnostic".to_owned()));
    }

    let records: Vec<_> = (0..3)
        .map(|_| receiver.try_recv().expect("all diagnostics collected"))
        .collect();
    assert_eq!(records[0].id, "1");
    assert_eq!(records[1].run_id, run_id("other"));
    assert_eq!(records[2].id, "3");
    assert_eq!(
        store.snapshot(&requested).diagnostics,
        vec![records[2].clone()]
    );
    assert!(!format!("{output:?}").contains("private diagnostic"));
}

#[test]
fn output_normalizes_controls_and_bounds_utf8_without_losing_truncation_flags() {
    let (output, mut receiver) = OperatorDiagnosticOutput::channel();
    let store = OperatorDiagnosticStore::new(Some(output));
    let requested = run_id("normalized");
    let prefix = "x".repeat(MAX_OPERATOR_DIAGNOSTIC_TEXT_BYTES - 1);
    let mut record = diagnostic(requested.clone(), format!("{prefix}é tail"));
    record.stderr = "\0\u{1b}\r\n\t".to_owned();
    record.stderr_truncated = true;
    store.record(record);

    let record = receiver.try_recv().expect("bounded diagnostic");
    assert_eq!(record.stdout, prefix);
    assert!(record.stdout_truncated);
    assert_eq!(record.stderr, "\u{fffd}\u{fffd}\r\n\t");
    assert!(record.stderr_truncated);
    assert_eq!(store.snapshot(&requested).diagnostics, vec![record]);
}

#[test]
fn slow_collector_reports_exact_loss_and_drains_after_producers_close() {
    use tokio::sync::broadcast::error::TryRecvError;

    let (output, mut receiver) = OperatorDiagnosticOutput::channel();
    let store = OperatorDiagnosticStore::new(Some(output.clone()));
    let requested = run_id("overflow");
    let lost = 7;
    for _ in 0..output::DIAGNOSTIC_OUTPUT_CAPACITY + lost {
        store.record(diagnostic(requested.clone(), "failure".to_owned()));
    }
    drop(output);
    drop(store);

    assert_eq!(receiver.try_recv(), Err(TryRecvError::Lagged(lost as u64)));
    for index in lost + 1..=output::DIAGNOSTIC_OUTPUT_CAPACITY + lost {
        assert_eq!(
            receiver.try_recv().expect("retained record").id,
            index.to_string()
        );
    }
    assert_eq!(receiver.try_recv(), Err(TryRecvError::Closed));
}

#[test]
fn stopped_collector_does_not_disrupt_recording_or_private_snapshots() {
    let (output, receiver) = OperatorDiagnosticOutput::channel();
    let store = OperatorDiagnosticStore::new(Some(output));
    drop(receiver);
    let requested = run_id("collector-stopped");
    for _ in 0..3 {
        store.record(diagnostic(requested.clone(), "failure".to_owned()));
    }

    let records = store.snapshot(&requested).diagnostics;
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].id, "2");
    assert_eq!(records[1].id, "3");
}

#[test]
fn concurrent_producers_publish_in_store_order_without_duplicate_records() {
    use std::sync::Arc;

    let (output, mut receiver) = OperatorDiagnosticOutput::channel();
    let store = Arc::new(OperatorDiagnosticStore::new(Some(output)));
    std::thread::scope(|scope| {
        for lane in 0..4 {
            let store = store.clone();
            scope.spawn(move || {
                for _ in 0..16 {
                    store.record(diagnostic(
                        run_id(&format!("run-{lane}")),
                        "failure".to_owned(),
                    ));
                }
            });
        }
    });
    drop(store);

    for index in 1..=64 {
        assert_eq!(
            receiver.try_recv().expect("ordered diagnostic").id,
            index.to_string()
        );
    }
    assert_eq!(
        receiver.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Closed)
    );
}
