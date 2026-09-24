use super::*;

#[test]
fn source_checkout_failure_is_a_public_application_error() {
    let error = cloud_backend_error(NativeV2CloudError::Allocation(
        CapsuleAllocationUnavailable::SourceCheckout,
    ));
    assert_eq!(error.code, "source_checkout_unavailable");
    assert!(
        error
            .message
            .contains("repository access, branch, or revision")
    );
    assert!(error.details.is_none());
}

#[test]
fn attach_lookup_failures_are_public_application_errors() {
    for (source, code) in [
        (NativeV2ObservationError::ExecutionNotFound, NOT_FOUND),
        (NativeV2ObservationError::ExecutionNotActive, GONE),
        (NativeV2ObservationError::ExecutionNotLive, GONE),
    ] {
        let error = cloud_backend_error(NativeV2CloudError::Observation(source));
        assert_eq!(error.code, code);
        assert!(error.details.is_none());
    }
}

#[test]
fn backend_errors_preserve_public_refusals_and_hide_internal_failures() {
    let admission = cloud_backend_error(NativeV2CloudError::Admission(
        NativeV2AdmissionError::UnsupportedGraphProfile,
    ));
    assert_eq!(admission.code, GRAPH_INVALID);
    assert!(admission.message.contains("graph profile"));
    assert!(admission.details.is_none());

    let existing_run_id = RunId::new("existing-run");
    let conflict = cloud_backend_error(NativeV2CloudError::Ledger(
        RunLedgerError::SubmissionConflict {
            existing_run_id: existing_run_id.clone(),
        },
    ));
    assert_eq!(conflict.code, IDEMPOTENCY_REUSE);
    assert_eq!(
        conflict.details,
        Some(serde_json::json!({ "runId": existing_run_id }))
    );

    for source in [
        NativeV2CloudError::Observation(NativeV2ObservationError::Ledger(RunLedgerError::Storage)),
        NativeV2CloudError::Observation(NativeV2ObservationError::Ledger(RunLedgerError::Corrupt)),
    ] {
        let error = cloud_backend_error(source);
        assert_eq!(error.code, "SOURCE_UNAVAILABLE");
        assert!(error.message.contains("history is unavailable"));
        assert!(error.details.is_none());
    }

    let internal = cloud_backend_error(NativeV2CloudError::SubmissionIdentity);
    assert_eq!(internal.code, INTERNAL_ERROR_CODE);
    assert_eq!(internal.message, "native-v2 operation failed");
    assert!(internal.details.is_none());
}
