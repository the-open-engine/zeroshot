use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use super::*;

fn canonical_fault() -> EngineFault {
    EngineFault::from_sources(vec![SafeSourceFrame::new(
        FaultModule::Provider,
        FaultContext::Execution,
        EvidenceClass::Timeout,
    )])
    .assert_value()
}

#[test]
fn coverage_contract_module_evidence_accessors_preserve_only_typed_safe_state() {
    let diagnostic = RawDiagnostic::new(RedactionMarker::Credential, "secret").assert_value();
    let evidence = ModuleEvidence::new(
        FaultModule::Credential,
        FaultContext::Configuration,
        EvidenceClass::AuthenticationRequired,
    )
    .with_diagnostic(diagnostic);

    assert_eq!(evidence.module(), FaultModule::Credential);
    assert_eq!(evidence.context(), FaultContext::Configuration);
    assert_eq!(evidence.class(), EvidenceClass::AuthenticationRequired);
    assert_eq!(evidence.diagnostic(), Some(&diagnostic));
}

#[test]
fn coverage_contract_fault_decoder_checks_summary_bound_before_semantics() {
    let mut encoded = serde_json::to_value(canonical_fault()).assert_value();
    *encoded.get_mut("summary").assert_value() =
        Value::String("s".repeat(MAX_FAULT_SUMMARY_BYTES + 1));
    assert_eq!(
        EngineFault::decode_json(&serde_json::to_vec(&encoded).assert_value()),
        Err(FaultError::SummaryTooLong)
    );

    let sources = vec![
        SafeSourceFrame::new(
            FaultModule::Engine,
            FaultContext::Recovery,
            EvidenceClass::InvariantViolation,
        );
        MAX_FAULT_SOURCES + 1
    ];
    assert_eq!(
        EngineFault::from_sources(sources),
        Err(FaultError::TooManySources)
    );
    assert_eq!(
        EngineFault::decode_json(&serde_json::to_vec(&json!({})).assert_value()),
        Err(FaultError::InvalidEncoding)
    );
}

#[test]
fn coverage_contract_fault_errors_have_stable_specific_operator_messages() {
    let cases = [
        (FaultError::SummaryTooLong, "summary exceeds"),
        (FaultError::DiagnosticTooLong, "diagnostic exceeds"),
        (FaultError::TooManySources, "source chain exceeds"),
        (FaultError::MissingPrimarySource, "canonical primary frame"),
        (
            FaultError::EncodedFaultTooLong,
            "encoded engine fault exceeds",
        ),
        (FaultError::InvalidSafeSummary, "engine-owned summary"),
        (
            FaultError::InvalidFaultSemantics,
            "canonical primary source",
        ),
        (FaultError::InvalidEncoding, "encoding is invalid"),
        (FaultError::EncodingFailed, "encoding failed"),
    ];
    for (error, expected) in cases {
        let message = error.to_string();
        assert!(message.contains(expected), "unexpected message: {message}");
    }
}
