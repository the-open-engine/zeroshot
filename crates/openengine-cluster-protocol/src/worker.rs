//! Stable worker descriptors and byte-free normalized worker outcomes.
//!
//! These types deliberately describe resolution contracts only. They contain no command,
//! endpoint, transport, credential value, callback, or execution configuration.

use std::borrow::Cow;
use std::collections::BTreeMap;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize};

use crate::value::deserialize_validated_wire;

use crate::{
    FieldName, GraphProfile, MediaType, NonEmptyEnumSet, PayloadType, RedactionClass, TypeId,
    WorkerErrorCode, WorkerRef,
};

mod error;
mod outcome;
pub use error::*;
pub use outcome::*;

pub const MAX_WORKER_PROTOCOL_LENGTH: usize = 64;
pub const MAX_WORKER_BINDING_VERSION_LENGTH: usize = 64;
pub const MAX_WORKER_PROFILE_LENGTH: usize = 256;
pub const BUILTIN_PROTOCOL: &str = "builtin";
pub const BUILTIN_VERSION: &str = "1";
pub const BUILTIN_PROFILE: &str = "openengine.worker.builtin/v1";
pub const RUNTIME_WORKER_ERRORS: [WorkerErrorCode; 4] = [
    WorkerErrorCode::Timeout,
    WorkerErrorCode::Crash,
    WorkerErrorCode::Malformed,
    WorkerErrorCode::Refusal,
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerProtocolBinding {
    pub protocol: String,
    pub version: String,
    pub profile: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WorkerProtocolBindingWire {
    protocol: String,
    version: String,
    profile: String,
}

impl WorkerProtocolBinding {
    pub fn new(
        protocol: impl Into<String>,
        version: impl Into<String>,
        profile: impl Into<String>,
    ) -> Result<Self, WorkerContractError> {
        let binding = Self {
            protocol: protocol.into(),
            version: version.into(),
            profile: profile.into(),
        };
        binding.validate()?;
        Ok(binding)
    }

    pub fn builtin_v1() -> Self {
        Self {
            protocol: BUILTIN_PROTOCOL.to_owned(),
            version: BUILTIN_VERSION.to_owned(),
            profile: BUILTIN_PROFILE.to_owned(),
        }
    }

    pub fn validate(&self) -> Result<(), WorkerContractError> {
        validate_binding_component(
            &self.protocol,
            MAX_WORKER_PROTOCOL_LENGTH,
            false,
            "protocol",
        )?;
        validate_binding_component(
            &self.version,
            MAX_WORKER_BINDING_VERSION_LENGTH,
            false,
            "version",
        )?;
        validate_binding_component(&self.profile, MAX_WORKER_PROFILE_LENGTH, true, "profile")?;

        let uses_reserved_builtin = self.protocol == BUILTIN_PROTOCOL
            || self.profile.starts_with("openengine.worker.builtin/");
        if uses_reserved_builtin
            && (
                self.protocol.as_str(),
                self.version.as_str(),
                self.profile.as_str(),
            ) != (BUILTIN_PROTOCOL, BUILTIN_VERSION, BUILTIN_PROFILE)
        {
            return Err(WorkerContractError::UnsupportedProtocolBinding);
        }
        Ok(())
    }
}

fn validate_binding_component(
    value: &str,
    maximum: usize,
    allow_slash: bool,
    kind: &'static str,
) -> Result<(), WorkerContractError> {
    let valid_edge = |byte: u8| byte.is_ascii_alphanumeric();
    let valid_body = |byte: u8| {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'.' | b'_' | b'-')
            || (allow_slash && byte == b'/')
    };
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || bytes.len() > maximum
        || !bytes.first().copied().is_some_and(valid_edge)
        || !bytes.last().copied().is_some_and(valid_edge)
        || !bytes.iter().copied().all(valid_body)
    {
        Err(WorkerContractError::InvalidProtocolBindingComponent(kind))
    } else {
        Ok(())
    }
}

impl<'de> Deserialize<'de> for WorkerProtocolBinding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_validated_wire(deserializer, |wire: WorkerProtocolBindingWire| {
            Self::new(wire.protocol, wire.version, wire.profile)
        })
    }
}

impl JsonSchema for WorkerProtocolBinding {
    fn schema_name() -> Cow<'static, str> {
        "WorkerProtocolBinding".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "additionalProperties": false,
            "required": ["protocol", "version", "profile"],
            "properties": {
                "protocol": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": MAX_WORKER_PROTOCOL_LENGTH,
                    "pattern": "^[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?$"
                },
                "version": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": MAX_WORKER_BINDING_VERSION_LENGTH,
                    "pattern": "^[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?$"
                },
                "profile": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": MAX_WORKER_PROFILE_LENGTH,
                    "pattern": "^[A-Za-z0-9](?:[A-Za-z0-9._/-]*[A-Za-z0-9])?$"
                }
            },
            "allOf": [{
                "if": {
                    "anyOf": [
                        { "required": ["protocol"], "properties": { "protocol": { "const": BUILTIN_PROTOCOL } } },
                        {
                            "required": ["profile"],
                            "properties": {
                                "profile": { "pattern": "^openengine\\.worker\\.builtin/" }
                            }
                        }
                    ]
                },
                "then": {
                    "properties": {
                        "protocol": { "const": BUILTIN_PROTOCOL },
                        "version": { "const": BUILTIN_VERSION },
                        "profile": { "const": BUILTIN_PROFILE }
                    }
                }
            }]
        })
    }
}

/// Opaque registry identity. The handle can select secret material but can never contain it.
#[derive(
    Clone, Debug, Deserialize, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(transparent)]
#[schemars(transparent)]
pub struct CredentialHandle(crate::PolicyRef);

impl CredentialHandle {
    pub fn new(value: impl Into<String>) -> Result<Self, WorkerContractError> {
        crate::PolicyRef::new(value)
            .map(Self)
            .map_err(|_| WorkerContractError::InvalidOpaqueHandle)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyPolicy {
    #[default]
    Strict,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CapabilityPolicy {
    pub autonomy: AutonomyPolicy,
    pub permission_policy: crate::PolicyRef,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct VerifierContract {
    #[schemars(
        schema_with = "crate::value::identifier_keyed_map_schema::<FieldName, NonEmptyEnumSet>"
    )]
    pub signals: BTreeMap<FieldName, NonEmptyEnumSet>,
    pub diagnostic: PayloadType,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerContract {
    pub input: PayloadType,
    pub output: PayloadType,
    pub verifier: Option<VerifierContract>,
    #[schemars(schema_with = "closed_worker_errors_schema")]
    pub errors: Vec<WorkerErrorCode>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ArtifactResultProfile {
    #[schemars(schema_with = "nonempty_unique_array_schema::<TypeId>")]
    pub allowed_type_ids: Vec<TypeId>,
    #[schemars(schema_with = "nonempty_unique_array_schema::<MediaType>")]
    pub allowed_media_types: Vec<MediaType>,
    pub minimum_redaction: RedactionClass,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    deny_unknown_fields,
    rename_all = "camelCase",
    try_from = "WorkerDescriptorWire"
)]
pub struct WorkerDescriptor {
    pub worker: WorkerRef,
    pub graph_profiles: Vec<GraphProfile>,
    pub binding: WorkerProtocolBinding,
    pub contract: WorkerContract,
    pub capability_policy: CapabilityPolicy,
    pub artifact_profile: ArtifactResultProfile,
    pub credential_requirements: Vec<CredentialHandle>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WorkerDescriptorWire {
    worker: WorkerRef,
    #[schemars(schema_with = "nonempty_unique_array_schema::<GraphProfile>")]
    graph_profiles: Vec<GraphProfile>,
    binding: WorkerProtocolBinding,
    contract: WorkerContract,
    capability_policy: CapabilityPolicy,
    artifact_profile: ArtifactResultProfile,
    #[schemars(schema_with = "unique_array_schema::<CredentialHandle>")]
    credential_requirements: Vec<CredentialHandle>,
}

impl WorkerDescriptor {
    pub fn validate(&self) -> Result<(), WorkerContractError> {
        self.binding.validate()?;
        self.validate_collections()?;
        self.validate_builtin_binding()
    }

    fn validate_collections(&self) -> Result<(), WorkerContractError> {
        require_unique_nonempty(&self.graph_profiles, "graph profiles")?;
        require_unique_nonempty(&self.contract.errors, "worker errors")?;
        if self.contract.errors.len() != RUNTIME_WORKER_ERRORS.len()
            || !RUNTIME_WORKER_ERRORS
                .iter()
                .all(|code| self.contract.errors.contains(code))
        {
            return Err(WorkerContractError::IncompleteRuntimeErrors);
        }
        require_unique_nonempty(&self.artifact_profile.allowed_type_ids, "artifact type IDs")?;
        require_unique_nonempty(
            &self.artifact_profile.allowed_media_types,
            "artifact media types",
        )?;
        require_unique(&self.credential_requirements, "credential handles")
    }

    fn validate_builtin_binding(&self) -> Result<(), WorkerContractError> {
        if self.binding.protocol == BUILTIN_PROTOCOL && !self.credential_requirements.is_empty() {
            Err(WorkerContractError::InvalidBuiltinBinding)
        } else {
            Ok(())
        }
    }
}

impl TryFrom<WorkerDescriptorWire> for WorkerDescriptor {
    type Error = WorkerContractError;

    fn try_from(wire: WorkerDescriptorWire) -> Result<Self, Self::Error> {
        let descriptor = Self {
            worker: wire.worker,
            graph_profiles: wire.graph_profiles,
            binding: wire.binding,
            contract: wire.contract,
            capability_policy: wire.capability_policy,
            artifact_profile: wire.artifact_profile,
            credential_requirements: wire.credential_requirements,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }
}

impl JsonSchema for WorkerDescriptor {
    fn schema_name() -> Cow<'static, str> {
        "WorkerDescriptor".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let base = generator.subschema_for::<WorkerDescriptorWire>();
        json_schema!({
            "allOf": [
                base,
                {
                    "if": {
                        "required": ["binding"],
                        "properties": {
                            "binding": {
                                "required": ["protocol"],
                                "properties": { "protocol": { "const": BUILTIN_PROTOCOL } }
                            }
                        }
                    },
                    "then": {
                        "required": ["credentialRequirements"],
                        "properties": { "credentialRequirements": { "maxItems": 0 } }
                    }
                }
            ]
        })
    }
}

fn require_unique_nonempty<T>(values: &[T], kind: &'static str) -> Result<(), WorkerContractError>
where
    T: PartialEq,
{
    if values.is_empty() {
        return Err(WorkerContractError::Empty(kind));
    }
    require_unique(values, kind)
}

fn require_unique<T>(values: &[T], kind: &'static str) -> Result<(), WorkerContractError>
where
    T: PartialEq,
{
    if values
        .iter()
        .enumerate()
        .all(|(index, value)| !values.iter().take(index).any(|prior| prior == value))
    {
        Ok(())
    } else {
        Err(WorkerContractError::Duplicate(kind))
    }
}

fn nonempty_unique_array_schema<T>(generator: &mut SchemaGenerator) -> Schema
where
    T: JsonSchema,
{
    json_schema!({
        "type": "array",
        "minItems": 1,
        "uniqueItems": true,
        "items": generator.subschema_for::<T>()
    })
}

fn unique_array_schema<T>(generator: &mut SchemaGenerator) -> Schema
where
    T: JsonSchema,
{
    json_schema!({
        "type": "array",
        "uniqueItems": true,
        "items": generator.subschema_for::<T>()
    })
}

fn closed_worker_errors_schema(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "array",
        "minItems": RUNTIME_WORKER_ERRORS.len(),
        "maxItems": RUNTIME_WORKER_ERRORS.len(),
        "uniqueItems": true,
        "items": {
            "enum": ["timeout", "crash", "malformed", "refusal"]
        }
    })
}
