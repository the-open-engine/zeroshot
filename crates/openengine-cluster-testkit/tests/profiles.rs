use std::collections::BTreeMap;

use openengine_cluster_protocol::{
    ArtifactId, ArtifactLineage, ArtifactProducer, ArtifactRef, ByteLength, EnumLabel, FieldName,
    Generation, MediaType, NodeName, PositiveInteger, RedactionClass, RunId, Sha256Digest, TypeId,
    WorkerDescriptor, WorkerErrorCode, WorkerFailureReason, WorkerOutcome, WorkerProtocolBinding,
    WorkerRef,
};
use openengine_cluster_testkit::worker_profiles::{
    normalize_mock_a2a_1_0, normalize_mock_acp_v1, MockA2a1_0Result, MockAcpV1Result,
    MockPolicyDecision,
};
use serde_json::json;

#[test]
fn profiles_pin_only_acp_v1_and_a2a_1_0() {
    assert_eq!(WorkerProtocolBinding::acp_v1().version, "1");
    assert_eq!(WorkerProtocolBinding::a2a_1_0().version, "1.0");
}

fn descriptor(protocol: &str) -> WorkerDescriptor {
    let binding = match protocol {
        "acp" => WorkerProtocolBinding::acp_v1(),
        "a2a" => WorkerProtocolBinding::a2a_1_0(),
        _ => unreachable!(),
    };
    serde_json::from_value(json!({
        "worker": format!("mock.{protocol}@1"),
        "graphProfiles": ["openengine.graph.full/v1"], "binding": binding,
        "contract": { "input": { "kind": "string" }, "output": { "kind": "string" },
            "verifier": null, "errors": ["crash", "refusal", "malformed", "timeout"] },
        "capabilityPolicy": { "autonomy": "strict", "permissionPolicy": "policy.mock@1" },
        "artifactProfile": { "allowedTypeIds": ["openengine.result@1"],
            "allowedMediaTypes": ["application/json"], "minimumRedaction": "internal" },
        "credentialRequirements": []
    }))
    .unwrap()
}

fn verifier_descriptor(protocol: &str) -> WorkerDescriptor {
    let mut value = serde_json::to_value(descriptor(protocol)).unwrap();
    value["contract"]["verifier"] = json!({
        "signals": { "verdict": ["accepted", "rejected"] },
        "diagnostic": {
            "kind": "record",
            "fields": {
                "message": { "type": { "kind": "string" }, "required": true }
            }
        }
    });
    serde_json::from_value(value).unwrap()
}

fn signals(values: &[(&str, &str)]) -> BTreeMap<FieldName, EnumLabel> {
    values
        .iter()
        .map(|(field, label)| {
            (
                FieldName::new(*field).unwrap(),
                EnumLabel::new(*label).unwrap(),
            )
        })
        .collect()
}

fn receipt() -> ArtifactRef {
    ArtifactRef {
        artifact_id: ArtifactId::new("result").unwrap(),
        sha256: Sha256Digest::new("a".repeat(64)).unwrap(),
        byte_length: ByteLength::new(2).unwrap(),
        media_type: MediaType::new("application/json").unwrap(),
        type_id: TypeId::new("openengine.result@1").unwrap(),
        producer: ArtifactProducer {
            node: NodeName::new("worker").unwrap(),
            worker: WorkerRef::new("mock.acp@1").unwrap(),
        },
        lineage: ArtifactLineage {
            generation: Generation::new(1).unwrap(),
            run_id: RunId::new("run-1"),
            attempt: PositiveInteger::new(1).unwrap(),
        },
        redaction: RedactionClass::Internal,
    }
}

fn assert_error(outcome: WorkerOutcome, code: WorkerErrorCode, reason: WorkerFailureReason) {
    assert!(
        matches!(outcome, WorkerOutcome::Error { code: actual_code, reason: actual_reason }
        if actual_code == code && actual_reason == reason)
    );
}

#[test]
fn acp_v1_normal_error_permission_input_and_malformed_are_closed() {
    let descriptor = descriptor("acp");
    assert!(matches!(
        normalize_mock_acp_v1(
            &descriptor,
            MockAcpV1Result::Completed {
                output: json!("ok"),
                artifacts: vec![receipt()]
            }
        ),
        WorkerOutcome::Verified { .. }
    ));
    assert_error(
        normalize_mock_acp_v1(
            &descriptor,
            MockAcpV1Result::DeclaredError(WorkerErrorCode::Crash),
        ),
        WorkerErrorCode::Crash,
        WorkerFailureReason::DeclaredFailure,
    );
    assert_error(
        normalize_mock_acp_v1(
            &descriptor,
            MockAcpV1Result::PermissionRequest {
                decision: MockPolicyDecision::Deny,
                allowed_output: json!("discarded"),
                artifacts: vec![],
            },
        ),
        WorkerErrorCode::Refusal,
        WorkerFailureReason::PolicyDenied,
    );
    assert!(matches!(
        normalize_mock_acp_v1(
            &descriptor,
            MockAcpV1Result::PermissionRequest {
                decision: MockPolicyDecision::Allow,
                allowed_output: json!("allowed"),
                artifacts: vec![]
            }
        ),
        WorkerOutcome::Verified { .. }
    ));
    assert_error(
        normalize_mock_acp_v1(&descriptor, MockAcpV1Result::InputRequest),
        WorkerErrorCode::Refusal,
        WorkerFailureReason::InteractiveInputRequired,
    );
    assert_error(
        normalize_mock_acp_v1(
            &descriptor,
            MockAcpV1Result::Malformed(json!({"raw": true})),
        ),
        WorkerErrorCode::Malformed,
        WorkerFailureReason::MalformedResult,
    );

    let mut under_redacted_receipt = receipt();
    under_redacted_receipt.redaction = RedactionClass::Public;
    assert_error(
        normalize_mock_acp_v1(
            &descriptor,
            MockAcpV1Result::Completed {
                output: json!("ok"),
                artifacts: vec![under_redacted_receipt],
            },
        ),
        WorkerErrorCode::Malformed,
        WorkerFailureReason::MalformedResult,
    );
}

#[test]
fn a2a_1_0_states_fail_closed_without_callbacks() {
    let descriptor = descriptor("a2a");
    assert!(matches!(
        normalize_mock_a2a_1_0(
            &descriptor,
            MockA2a1_0Result::Completed {
                output: json!("ok"),
                artifacts: vec![]
            }
        ),
        WorkerOutcome::Verified { .. }
    ));
    for (state, reason) in [
        (
            MockA2a1_0Result::InputRequired,
            WorkerFailureReason::InteractiveInputRequired,
        ),
        (
            MockA2a1_0Result::AuthRequired,
            WorkerFailureReason::AuthenticationRequired,
        ),
    ] {
        let outcome = normalize_mock_a2a_1_0(&descriptor, state);
        assert_error(outcome.clone(), WorkerErrorCode::Refusal, reason);
        let value = serde_json::to_value(outcome).unwrap();
        assert!(value.get("callback").is_none() && value.get("wait").is_none());
    }
    assert_error(
        normalize_mock_a2a_1_0(
            &descriptor,
            MockA2a1_0Result::MalformedArtifact(json!({ "signedUrl": "secret" })),
        ),
        WorkerErrorCode::Malformed,
        WorkerFailureReason::MalformedResult,
    );
    assert_error(
        normalize_mock_a2a_1_0(
            &descriptor,
            MockA2a1_0Result::Failed(WorkerErrorCode::Crash),
        ),
        WorkerErrorCode::Crash,
        WorkerFailureReason::DeclaredFailure,
    );
}

#[test]
fn malformed_output_and_artifact_metadata_are_ephemeral() {
    let descriptor = descriptor("acp");
    assert_error(
        normalize_mock_acp_v1(
            &descriptor,
            MockAcpV1Result::Completed {
                output: json!(42),
                artifacts: vec![],
            },
        ),
        WorkerErrorCode::Malformed,
        WorkerFailureReason::MalformedResult,
    );
    let mut bad_receipt = receipt();
    bad_receipt.media_type = MediaType::new("text/plain").unwrap();
    assert_error(
        normalize_mock_acp_v1(
            &descriptor,
            MockAcpV1Result::Completed {
                output: json!("ok"),
                artifacts: vec![bad_receipt],
            },
        ),
        WorkerErrorCode::Malformed,
        WorkerFailureReason::MalformedResult,
    );
}

#[test]
fn verifier_completions_validate_output_signals_diagnostic_and_artifacts() {
    let acp = verifier_descriptor("acp");
    let valid_signals = signals(&[("verdict", "accepted")]);
    assert!(matches!(
        normalize_mock_acp_v1(
            &acp,
            MockAcpV1Result::VerifierCompleted {
                output: json!("ok"),
                signals: valid_signals.clone(),
                diagnostic: json!({ "message": "accepted" }),
                artifacts: vec![receipt()],
            }
        ),
        WorkerOutcome::Verifier { .. }
    ));

    assert_error(
        normalize_mock_acp_v1(
            &acp,
            MockAcpV1Result::Completed {
                output: json!("missing verifier fields"),
                artifacts: vec![],
            },
        ),
        WorkerErrorCode::Malformed,
        WorkerFailureReason::MalformedResult,
    );

    for (output, actual_signals, diagnostic) in [
        (json!(42), valid_signals.clone(), json!({ "message": "ok" })),
        (
            json!("ok"),
            signals(&[("verdict", "unknown")]),
            json!({ "message": "ok" }),
        ),
        (
            json!("ok"),
            signals(&[("unexpected", "accepted")]),
            json!({ "message": "ok" }),
        ),
        (json!("ok"), valid_signals.clone(), json!({ "message": 42 })),
    ] {
        assert_error(
            normalize_mock_acp_v1(
                &acp,
                MockAcpV1Result::VerifierCompleted {
                    output,
                    signals: actual_signals,
                    diagnostic,
                    artifacts: vec![],
                },
            ),
            WorkerErrorCode::Malformed,
            WorkerFailureReason::MalformedResult,
        );
    }

    let a2a = verifier_descriptor("a2a");
    assert!(matches!(
        normalize_mock_a2a_1_0(
            &a2a,
            MockA2a1_0Result::VerifierCompleted {
                output: json!("ok"),
                signals: valid_signals,
                diagnostic: json!({ "message": "accepted" }),
                artifacts: vec![],
            }
        ),
        WorkerOutcome::Verifier { .. }
    ));
}

#[test]
fn integral_json_numbers_match_integer_contracts_recursively() {
    let mut acp = serde_json::to_value(verifier_descriptor("acp")).unwrap();
    acp["contract"]["output"] = json!({
        "kind": "record",
        "fields": {
            "values": {
                "type": { "kind": "array", "items": { "kind": "integer" } },
                "required": true
            }
        }
    });
    acp["contract"]["verifier"]["diagnostic"] = json!({
        "kind": "record",
        "fields": {
            "details": {
                "type": {
                    "kind": "record",
                    "fields": {
                        "count": { "type": { "kind": "integer" }, "required": true }
                    }
                },
                "required": true
            }
        }
    });
    let acp: WorkerDescriptor = serde_json::from_value(acp).unwrap();
    assert!(matches!(
        normalize_mock_acp_v1(
            &acp,
            MockAcpV1Result::VerifierCompleted {
                output: json!({ "values": [42.0, -3.0] }),
                signals: signals(&[("verdict", "accepted")]),
                diagnostic: json!({ "details": { "count": 7.0 } }),
                artifacts: vec![],
            }
        ),
        WorkerOutcome::Verifier { .. }
    ));

    assert_error(
        normalize_mock_acp_v1(
            &acp,
            MockAcpV1Result::VerifierCompleted {
                output: json!({ "values": [42.5] }),
                signals: signals(&[("verdict", "accepted")]),
                diagnostic: json!({ "details": { "count": 7.0 } }),
                artifacts: vec![],
            },
        ),
        WorkerErrorCode::Malformed,
        WorkerFailureReason::MalformedResult,
    );
}
