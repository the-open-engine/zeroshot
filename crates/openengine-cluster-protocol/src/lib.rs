//! Canonical Open Engine Cluster Protocol wire and domain types.

use std::borrow::Cow;
use std::fmt;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const JSON_RPC_VERSION: &str = "2.0";
pub const PROTOCOL_NAME: &str = "openengine.cluster";
/// The only wire protocol version accepted by Cluster Protocol v1 servers.
pub const PROTOCOL_VERSION: &str = "openengine.cluster/v1";
pub const BASE_PROFILE: &str = "openengine.cluster/base/v1";

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
pub const APPLICATION_ERROR: i64 = -32000;

pub const UNSUPPORTED_PROTOCOL_VERSION: &str = "UNSUPPORTED_PROTOCOL_VERSION";
pub const INTERNAL_ERROR_CODE: &str = "INTERNAL_ERROR";

pub const MAX_SAFE_GENERATION: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Deserialize, Eq, Hash, JsonSchema, PartialEq, Serialize)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Integer(i64),
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonRpcRequest<P> {
    pub jsonrpc: String,
    pub id: RequestId,
    pub method: String,
    pub params: P,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct JsonRpcSuccess<T> {
    pub jsonrpc: String,
    pub id: RequestId,
    pub result: T,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct JsonRpcErrorResponse {
    pub jsonrpc: String,
    pub id: Option<RequestId>,
    pub error: JsonRpcError,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(untagged)]
pub enum JsonRpcResponse<T> {
    Success(JsonRpcSuccess<T>),
    Error(JsonRpcErrorResponse),
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<DomainErrorData>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct DomainErrorData {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl DomainErrorData {
    #[must_use]
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            details: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InitializeParams {
    #[schemars(schema_with = "protocol_version_schema")]
    pub protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    #[schemars(schema_with = "protocol_version_schema")]
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub status: ClusterStatus,
}

impl InitializeResult {
    #[must_use]
    pub fn new(capabilities: ServerCapabilities, status: ClusterStatus) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            capabilities,
            status,
        }
    }

    pub fn validate_protocol_version(&self) -> Result<(), ProtocolVersionMismatch> {
        if self.protocol_version == PROTOCOL_VERSION {
            Ok(())
        } else {
            Err(ProtocolVersionMismatch {
                received: self.protocol_version.clone(),
            })
        }
    }
}

fn protocol_version_schema(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "string",
        "const": PROTOCOL_VERSION
    })
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("protocol version mismatch: expected {PROTOCOL_VERSION}, received {received}")]
pub struct ProtocolVersionMismatch {
    pub received: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GetParams {
    #[serde(default)]
    pub at_cursor: Option<Cursor>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetResult {
    pub spec: Option<Value>,
    pub status: ClusterStatus,
    pub at_cursor: Option<Cursor>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerCapabilities {}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterStatus {
    pub phase: Phase,
    pub observed_generation: Option<Generation>,
    pub current_run_id: Option<RunId>,
    pub at_cursor: Option<Cursor>,
}

impl ClusterStatus {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            phase: Phase::Empty,
            observed_generation: None,
            current_run_id: None,
            at_cursor: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Empty,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Cursor(String);

impl Cursor {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RunId(String);

impl RunId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Generation(u64);

impl Generation {
    pub fn new(value: u64) -> Result<Self, GenerationOutOfRange> {
        if value <= MAX_SAFE_GENERATION {
            Ok(Self(value))
        } else {
            Err(GenerationOutOfRange(value))
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for Generation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct GenerationVisitor;

        impl Visitor<'_> for GenerationVisitor {
            type Value = Generation;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "an integer from 0 through {MAX_SAFE_GENERATION}")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Generation::new(value).map_err(E::custom)
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let value = u64::try_from(value).map_err(E::custom)?;
                self.visit_u64(value)
            }
        }

        deserializer.deserialize_u64(GenerationVisitor)
    }
}

impl JsonSchema for Generation {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "Generation".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "integer",
            "minimum": 0,
            "maximum": MAX_SAFE_GENERATION
        })
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("generation {0} exceeds the JavaScript-safe integer maximum {MAX_SAFE_GENERATION}")]
pub struct GenerationOutOfRange(pub u64);
