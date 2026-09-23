use super::*;

#[test]
fn arc_observation_sink_forwards_both_event_kinds_without_loss() {
    let inner = Arc::new(InMemoryObservationSink::default());
    let proxy = inner.clone();
    let operation = OperationObservation {
        module: ObservationModule::Engine,
        operation: ObservationOperation::Execution,
        outcome: ObservationOutcome::Succeeded,
        duration_ms: 7,
    };
    let fault = FaultObservation {
        module: ObservationModule::Provider,
        operation: ObservationOperation::Recovery,
        outcome: ObservationOutcome::Faulted,
        fault_code: FaultCode::Timeout,
        consequence: FaultConsequence::RecoveryBlocked,
        severity: FaultSeverity::Error,
        fault_size_bytes: 11,
        diagnostic_redacted: true,
    };

    ObservationSink::record_operation(&proxy, operation);
    ObservationSink::record_fault(&proxy, fault);

    assert_eq!(
        inner.snapshot(),
        ObservationSnapshot {
            operations_total: 1,
            faults_total: 1,
            diagnostics_redacted_total: 1,
            operation_duration_ms: vec![7],
            fault_size_bytes: vec![11],
            operations: vec![operation],
            faults: vec![fault],
        }
    );
}
