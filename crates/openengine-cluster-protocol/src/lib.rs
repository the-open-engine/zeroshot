//! Canonical wire and domain types for Open Engine Cluster Protocol v1.

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_NAME: &str = "openengine.cluster";
pub const PROTOCOL_VERSION: &str = "openengine.cluster/v1";
pub const PROTOCOL_PROFILE: &str = "openengine.cluster/v1/core";
pub const JSONRPC_VERSION: &str = "2.0";
pub const MAX_SAFE_GENERATION: u64 = 9_007_199_254_740_991;

fn nullable_schema<T: JsonSchema>(generator: &mut SchemaGenerator) -> Schema {
    Option::<T>::json_schema(generator)
}

fn nullable_request_id_schema(generator: &mut SchemaGenerator) -> Schema {
    nullable_schema::<RequestId>(generator)
}

fn nullable_cursor_schema(generator: &mut SchemaGenerator) -> Schema {
    nullable_schema::<Cursor>(generator)
}

fn nullable_generation_schema(generator: &mut SchemaGenerator) -> Schema {
    nullable_schema::<Generation>(generator)
}

fn nullable_run_id_schema(generator: &mut SchemaGenerator) -> Schema {
    nullable_schema::<RunId>(generator)
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Integer(i64),
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcRequest<P = Value> {
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

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct JsonRpcErrorResponse {
    pub jsonrpc: String,
    #[schemars(required, schema_with = "nullable_request_id_schema")]
    pub id: Option<RequestId>,
    pub error: JsonRpcError,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<DomainErrorData>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct DomainErrorData {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl DomainErrorData {
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            details: None,
        }
    }

    pub fn with_details(code: impl Into<String>, details: Value) -> Self {
        Self {
            code: code.into(),
            details: Some(details),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub status: ClusterStatus,
}

impl InitializeResult {
    pub fn empty() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            capabilities: ServerCapabilities {},
            status: ClusterStatus::empty(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GetParams {}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetResult {
    #[schemars(required)]
    pub spec: Option<Value>,
    pub status: ClusterStatus,
    #[schemars(required, schema_with = "nullable_cursor_schema")]
    pub at_cursor: Option<Cursor>,
}

impl GetResult {
    pub fn empty() -> Self {
        Self {
            spec: None,
            status: ClusterStatus::empty(),
            at_cursor: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerCapabilities {}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterStatus {
    pub phase: Phase,
    #[schemars(required, schema_with = "nullable_generation_schema")]
    pub observed_generation: Option<Generation>,
    #[schemars(required, schema_with = "nullable_run_id_schema")]
    pub current_run_id: Option<RunId>,
    #[schemars(required, schema_with = "nullable_cursor_schema")]
    pub at_cursor: Option<Cursor>,
}

impl ClusterStatus {
    pub fn empty() -> Self {
        Self {
            phase: Phase::Empty,
            observed_generation: None,
            current_run_id: None,
            at_cursor: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Empty,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Generation(#[schemars(range(max = 9_007_199_254_740_991_i64))] u64);

impl Generation {
    pub fn new(value: u64) -> Result<Self, GenerationOutOfRange> {
        if value <= MAX_SAFE_GENERATION {
            Ok(Self(value))
        } else {
            Err(GenerationOutOfRange(value))
        }
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for Generation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GenerationOutOfRange(pub u64);

impl std::fmt::Display for GenerationOutOfRange {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "generation {} exceeds JavaScript safe integer maximum {MAX_SAFE_GENERATION}",
            self.0
        )
    }
}

impl std::error::Error for GenerationOutOfRange {}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RunId(String);

impl RunId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Cursor(String);

impl Cursor {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
