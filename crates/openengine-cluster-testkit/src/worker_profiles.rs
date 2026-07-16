//! Test-only mock ACP v1 and A2A 1.0 normalization profiles.
//!
//! This is conformance data, not a transport, SDK adapter, discovery client, or runtime.

use std::collections::BTreeMap;

use openengine_cluster_protocol::{
    ArtifactRef, EnumLabel, FieldName, PayloadType, RedactionClass, VerifierContract,
    WorkerDescriptor, WorkerErrorCode, WorkerOutcome, WorkerProtocol, A2A_PROFILE, A2A_VERSION,
    ACP_PROFILE, ACP_VERSION,
};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MockPolicyDecision {
    Allow,
    Deny,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MockAcpV1Result {
    Completed {
        output: Value,
        artifacts: Vec<ArtifactRef>,
    },
    VerifierCompleted {
        output: Value,
        signals: BTreeMap<FieldName, EnumLabel>,
        diagnostic: Value,
        artifacts: Vec<ArtifactRef>,
    },
    DeclaredError(WorkerErrorCode),
    PermissionRequest {
        decision: MockPolicyDecision,
        allowed_output: Value,
        artifacts: Vec<ArtifactRef>,
    },
    InputRequest,
    Malformed(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub enum MockA2a1_0Result {
    Completed {
        output: Value,
        artifacts: Vec<ArtifactRef>,
    },
    VerifierCompleted {
        output: Value,
        signals: BTreeMap<FieldName, EnumLabel>,
        diagnostic: Value,
        artifacts: Vec<ArtifactRef>,
    },
    Failed(WorkerErrorCode),
    InputRequired,
    AuthRequired,
    MalformedArtifact(Value),
}

struct VerifierCompletion {
    output: Value,
    signals: BTreeMap<FieldName, EnumLabel>,
    diagnostic: Value,
    artifacts: Vec<ArtifactRef>,
}

#[must_use]
pub fn normalize_mock_acp_v1(
    descriptor: &WorkerDescriptor,
    result: MockAcpV1Result,
) -> WorkerOutcome {
    if !descriptor_uses_profile(descriptor, WorkerProtocol::Acp, ACP_VERSION, ACP_PROFILE) {
        return WorkerOutcome::malformed();
    }
    match result {
        MockAcpV1Result::Completed { output, artifacts } => {
            normalize_completed(descriptor, output, artifacts)
        }
        MockAcpV1Result::VerifierCompleted {
            output,
            signals,
            diagnostic,
            artifacts,
        } => normalize_verifier_completed(
            descriptor,
            VerifierCompletion {
                output,
                signals,
                diagnostic,
                artifacts,
            },
        ),
        MockAcpV1Result::DeclaredError(code) => normalize_declared_error(descriptor, code),
        MockAcpV1Result::PermissionRequest {
            decision: MockPolicyDecision::Allow,
            allowed_output,
            artifacts,
        } => normalize_completed(descriptor, allowed_output, artifacts),
        MockAcpV1Result::PermissionRequest {
            decision: MockPolicyDecision::Deny,
            ..
        } => WorkerOutcome::policy_refusal(),
        MockAcpV1Result::InputRequest => WorkerOutcome::interactive_refusal(),
        MockAcpV1Result::Malformed(_) => WorkerOutcome::malformed(),
    }
}

#[must_use]
pub fn normalize_mock_a2a_1_0(
    descriptor: &WorkerDescriptor,
    result: MockA2a1_0Result,
) -> WorkerOutcome {
    if !descriptor_uses_profile(descriptor, WorkerProtocol::A2a, A2A_VERSION, A2A_PROFILE) {
        return WorkerOutcome::malformed();
    }
    match result {
        MockA2a1_0Result::Completed { output, artifacts } => {
            normalize_completed(descriptor, output, artifacts)
        }
        MockA2a1_0Result::VerifierCompleted {
            output,
            signals,
            diagnostic,
            artifacts,
        } => normalize_verifier_completed(
            descriptor,
            VerifierCompletion {
                output,
                signals,
                diagnostic,
                artifacts,
            },
        ),
        MockA2a1_0Result::Failed(code) => normalize_declared_error(descriptor, code),
        MockA2a1_0Result::InputRequired => WorkerOutcome::interactive_refusal(),
        MockA2a1_0Result::AuthRequired => WorkerOutcome::authentication_refusal(),
        MockA2a1_0Result::MalformedArtifact(_) => WorkerOutcome::malformed(),
    }
}

fn descriptor_uses_profile(
    descriptor: &WorkerDescriptor,
    protocol: WorkerProtocol,
    version: &str,
    profile: &str,
) -> bool {
    descriptor.validate().is_ok()
        && descriptor.binding.protocol == protocol
        && descriptor.binding.version == version
        && descriptor.binding.profile == profile
}

fn normalize_declared_error(descriptor: &WorkerDescriptor, code: WorkerErrorCode) -> WorkerOutcome {
    if descriptor.contract.errors.contains(&code) {
        WorkerOutcome::declared_failure(code)
    } else {
        WorkerOutcome::malformed()
    }
}

fn normalize_completed(
    descriptor: &WorkerDescriptor,
    output: Value,
    artifacts: Vec<ArtifactRef>,
) -> WorkerOutcome {
    if descriptor.contract.verifier.is_none()
        && payload_matches(&output, &descriptor.contract.output)
        && artifacts
            .iter()
            .all(|artifact| artifact_allowed(descriptor, artifact))
    {
        WorkerOutcome::Verified { output, artifacts }
    } else {
        WorkerOutcome::malformed()
    }
}

fn normalize_verifier_completed(
    descriptor: &WorkerDescriptor,
    completion: VerifierCompletion,
) -> WorkerOutcome {
    let Some(contract) = &descriptor.contract.verifier else {
        return WorkerOutcome::malformed();
    };
    if payload_matches(&completion.output, &descriptor.contract.output)
        && verifier_signals_match(&completion.signals, contract)
        && payload_matches(&completion.diagnostic, &contract.diagnostic)
        && completion
            .artifacts
            .iter()
            .all(|artifact| artifact_allowed(descriptor, artifact))
    {
        WorkerOutcome::Verifier {
            output: completion.output,
            signals: completion.signals,
            diagnostic: completion.diagnostic,
            artifacts: completion.artifacts,
        }
    } else {
        WorkerOutcome::malformed()
    }
}

fn verifier_signals_match(
    signals: &BTreeMap<FieldName, EnumLabel>,
    contract: &VerifierContract,
) -> bool {
    signals.len() == contract.signals.len()
        && contract.signals.iter().all(|(field, allowed_labels)| {
            signals
                .get(field)
                .is_some_and(|label| allowed_labels.values().contains(label))
        })
}

fn artifact_allowed(descriptor: &WorkerDescriptor, artifact: &ArtifactRef) -> bool {
    descriptor
        .artifact_profile
        .allowed_type_ids
        .contains(&artifact.type_id)
        && descriptor
            .artifact_profile
            .allowed_media_types
            .contains(&artifact.media_type)
        && redaction_rank(artifact.redaction)
            >= redaction_rank(descriptor.artifact_profile.minimum_redaction)
}

const fn redaction_rank(redaction: RedactionClass) -> u8 {
    match redaction {
        RedactionClass::Public => 0,
        RedactionClass::Internal => 1,
        RedactionClass::Confidential => 2,
        RedactionClass::Restricted => 3,
    }
}

fn payload_matches(value: &Value, payload_type: &PayloadType) -> bool {
    match payload_type {
        PayloadType::Null => value.is_null(),
        PayloadType::Boolean => value.is_boolean(),
        PayloadType::Integer => {
            value.as_i64().is_some()
                || value.as_u64().is_some()
                || value
                    .as_f64()
                    .is_some_and(|number| number.is_finite() && number.fract() == 0.0)
        }
        PayloadType::Number => value.is_number(),
        PayloadType::String => value.is_string(),
        PayloadType::Array { items } => value
            .as_array()
            .is_some_and(|values| values.iter().all(|value| payload_matches(value, items))),
        PayloadType::Enum { values } => value
            .as_str()
            .is_some_and(|value| values.values().iter().any(|label| label.as_str() == value)),
        PayloadType::Record { fields } => value.as_object().is_some_and(|object| {
            object
                .keys()
                .all(|name| fields.iter().any(|(field, _)| field.as_str() == name))
                && fields
                    .iter()
                    .all(|(name, field)| match object.get(name.as_str()) {
                        Some(value) => payload_matches(value, &field.value_type),
                        None => !field.required,
                    })
        }),
    }
}
