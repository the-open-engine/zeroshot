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
