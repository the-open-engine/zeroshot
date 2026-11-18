use super::*;

#[test]
fn oversized_submission_message_survives_hosted_target_projection() {
    for input_bytes in [3_000_011, 9_700_011] {
        let source = NativeV2PlatformError::Ledger(RunLedgerError::AdmittedRunTooLarge {
            submission_bytes: input_bytes + 1_000,
            input_bytes,
        });
        let message = source.to_string();
        let projected = project_platform_error(source);
        assert_eq!(
            projected.kind(),
            crate::native_v2_target_authority::TargetAuthorityErrorKind::Invalid
        );
        assert_eq!(projected.message(), message);
    }
}
