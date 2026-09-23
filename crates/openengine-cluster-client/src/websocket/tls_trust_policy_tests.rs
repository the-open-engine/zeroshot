use rcgen::{generate_simple_self_signed, CertifiedKey};

use super::*;

fn generated_root() -> CertificateDer<'static> {
    let generated = generate_simple_self_signed(vec!["localhost".to_owned()]);
    assert!(generated.is_ok(), "test root generation must succeed");
    let mut generated = generated.into_iter().collect::<Vec<_>>();
    let CertifiedKey { cert, .. } = generated.swap_remove(0);
    cert.der().clone()
}

fn expect_system_trust_error(
    native_certificates: Vec<CertificateDer<'static>>,
    native_errors: Vec<String>,
    additional_root_certificates: Vec<CertificateDer<'static>>,
) -> WebSocketDialError {
    let result = build_tls_connector_from_native(
        native_certificates,
        native_errors,
        additional_root_certificates,
    );
    assert!(
        result.is_err(),
        "invalid native trust state must fail closed"
    );
    let mut errors = result.err().into_iter().collect::<Vec<_>>();
    errors.swap_remove(0)
}

#[test]
fn empty_system_roots_fail_before_additional_roots_can_rescue() {
    let error = expect_system_trust_error(Vec::new(), Vec::new(), vec![generated_root()]);

    assert!(matches!(error, WebSocketDialError::SystemTrustRoots { .. }));
}

#[test]
fn partial_system_root_load_errors_fail_before_additional_roots_can_rescue() {
    let error = expect_system_trust_error(
        vec![generated_root()],
        vec!["native loader rejected one certificate".to_owned()],
        vec![generated_root()],
    );

    assert!(matches!(
        &error,
        WebSocketDialError::SystemTrustRoots { .. }
    ));
    assert!(
        error
            .to_string()
            .contains("native loader rejected one certificate")
    );
}

#[cfg(feature = "bundled-roots")]
#[test]
fn bundled_roots_do_not_fallback_when_system_roots_are_empty() {
    let error = expect_system_trust_error(Vec::new(), Vec::new(), Vec::new());

    assert!(matches!(error, WebSocketDialError::SystemTrustRoots { .. }));
}
