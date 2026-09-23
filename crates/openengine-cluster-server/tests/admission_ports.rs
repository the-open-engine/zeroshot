use openengine_cluster_protocol::{
    Cursor, DiagnosticSeverity, Generation, GraphDiagnostic, GraphDiagnosticCode, Phase, RunId,
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
