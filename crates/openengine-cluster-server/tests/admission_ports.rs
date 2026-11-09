use openengine_cluster_protocol::{
    Cursor, DiagnosticSeverity, Generation, GraphDiagnostic, GraphDiagnosticCode, Phase, RunId,
    TargetHttpProblem, MAX_TARGET_HTTP_PROBLEM_MESSAGE_BYTES, TARGET_RUN_REJECTED_CODE,
};
use openengine_cluster_server::admission::{
    CancellationObserver, CancellationSignal, ControlSnapshot, VerificationError,
};
use openengine_cluster_server::lifecycle::LifecycleSnapshot;

#[path = "support/assert_value.rs"]
mod assert_value;
use assert_value::AssertValue;

#[test]
fn cancellation_observers_share_state_without_mutation_authority() {
    let signal = CancellationSignal::default();
    let observer = signal.observer();
    assert!(!observer.is_cancelled());
    assert_eq!(
        format!("{observer:?}"),
        "CancellationObserver { cancelled: false }"
    );

    signal.cancel();
    assert!(signal.is_cancelled());
    assert!(observer.is_cancelled());
    assert_eq!(
        format!("{signal:?}"),
        "CancellationSignal { cancelled: true }"
    );
    assert_ne!(signal, CancellationSignal::default());
    assert_ne!(observer, CancellationObserver::default());
}

#[test]
fn control_status_uses_lifecycle_cursor_only_when_one_exists() {
    let control = ControlSnapshot {
        generation: Some(Generation::new(7).assert_value()),
        run_id: Some(RunId::new("run-7")),
        phase: Phase::Running,
        cursor: Some(Cursor::new("control:7")),
        ..ControlSnapshot::default()
    };

    let status = control.status();
    assert_eq!(status.observed_generation, control.generation);
    assert_eq!(status.current_run_id, control.run_id);
    assert_eq!(status.at_cursor, control.cursor);

    let inherited = control.status_with_lifecycle(&LifecycleSnapshot::default());
    assert_eq!(inherited.at_cursor, control.cursor);
    let durable = control.status_with_lifecycle(&LifecycleSnapshot {
        latest_cursor: Some(Cursor::new("lifecycle:8")),
        ..LifecycleSnapshot::default()
    });
    assert_eq!(durable.at_cursor, Some(Cursor::new("lifecycle:8")));
}

#[test]
fn verification_rejection_reports_the_first_diagnostic_or_safe_fallback() {
    let diagnostic = GraphDiagnostic {
        severity: DiagnosticSeverity::Error,
        code: GraphDiagnosticCode::InvalidGraphShape,
        message: "root node is missing".to_owned(),
        path: Vec::new(),
        related_nodes: Vec::new(),
    };
    assert_eq!(
        VerificationError::Rejected {
            diagnostics: vec![diagnostic],
        }
        .to_string(),
        "graph verification rejected the graph: root node is missing"
    );
    assert_eq!(
        VerificationError::Rejected {
            diagnostics: Vec::new(),
        }
        .to_string(),
        "graph verification rejected the graph"
    );
    assert_eq!(
        VerificationError::Internal("verifier unavailable".to_owned()).to_string(),
        "graph verifier failed internally: verifier unavailable"
    );
}

#[test]
fn verification_rejection_names_the_first_path_and_counts_the_rest_within_the_bound() {
    let diagnostic = GraphDiagnostic {
        severity: DiagnosticSeverity::Error,
        code: GraphDiagnosticCode::CeilingExceeded,
        message: "control assignment space exceeds 65536".to_owned(),
        path: serde_json::from_value(serde_json::json!([
            {"kind":"field","name":"root"},
            {"kind":"node","name":"run"},
            {"kind":"field","name":"children"},
            {"kind":"index","index":1},
            {"kind":"node","name":"check"},
            {"kind":"field","name":"branches"}
        ]))
        .assert_value(),
        related_nodes: Vec::new(),
    };
    let rejected = |diagnostic: &GraphDiagnostic, count| {
        VerificationError::Rejected {
            diagnostics: vec![diagnostic.clone(); count],
        }
        .to_string()
    };
    let first = concat!(
        "graph verification rejected the graph at root/`run`/children[1]/`check`/branches: ",
        "control assignment space exceeds 65536"
    );
    assert_eq!(rejected(&diagnostic, 1), first);
    assert_eq!(
        rejected(&diagnostic, 2),
        format!("{first} (and 1 more diagnostic)")
    );
    assert_eq!(
        rejected(&diagnostic, 3),
        format!("{first} (and 2 more diagnostics)")
    );

    let near_bound = |free: usize| {
        let cause = "graph verification rejected the graph: ".len();
        let long = GraphDiagnostic {
            message: "m".repeat(MAX_TARGET_HTTP_PROBLEM_MESSAGE_BYTES - cause - free),
            ..diagnostic.clone()
        };
        let text = rejected(&long, 2);
        assert!(
            TargetHttpProblem::new(TARGET_RUN_REJECTED_CODE, text.as_str(), None).is_ok(),
            "{} byte rejection text is not a valid target problem message",
            text.len()
        );
        (long.message, text)
    };
    // Too few bytes for the count, enough for the path's last field.
    let (message, text) = near_bound(20);
    assert_eq!(
        text,
        format!("graph verification rejected the graph at .../branches: {message}")
    );
    // The count fits exactly, leaving no room for the path.
    let (message, text) = near_bound(24);
    assert_eq!(
        text,
        format!("graph verification rejected the graph: {message} (and 1 more diagnostic)")
    );
}
