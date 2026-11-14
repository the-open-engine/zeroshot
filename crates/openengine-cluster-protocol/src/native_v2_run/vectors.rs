//! Ordered durable run vectors. List entries contain bounded fields only.
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use crate::{NodeName, RunId, UnixTimestampMillis};

/// Opaque record identity, retained unchanged across successors.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VectorId(String);
impl JsonSchema for VectorId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "VectorId".into()
    }
    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string", "minLength": 1, "maxLength": 64,
            "pattern": "^[A-Za-z0-9_.:-]+$"
        })
    }
}
impl VectorId {
    pub fn new(value: impl Into<String>) -> Result<Self, super::NativeV2RunValueError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
        {
            return Err(super::NativeV2RunValueError(
                "vector ID must contain 1 through 64 ASCII identifier characters",
            ));
        }
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for VectorId {
    type Error = super::NativeV2RunValueError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<VectorId> for String {
    fn from(value: VectorId) -> Self {
        value.0
    }
}
impl std::fmt::Display for VectorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunVector {
    pub id: VectorId,
    #[serde(deserialize_with = "deserialize_record_run_id")]
    #[schemars(with = "VectorId")]
    pub run_id: RunId,
    pub created_at: UnixTimestampMillis,
    #[serde(flatten)]
    pub content: RunVectorContent,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum RunVectorContent {
    SuspendRequest {},
    Suspension {
        reason: VectorSuspensionReason,
        node: NodeName,
    },
    Continuation {
        #[serde(rename = "resumedFrom", deserialize_with = "deserialize_record_run_id")]
        #[schemars(with = "VectorId")]
        resumed_from: RunId,
    },
    Wait {
        node: NodeName,
        #[serde(rename = "dueAt")]
        due_at: UnixTimestampMillis,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunVectorsParams {
    pub run_id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<VectorId>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::checkpoints::deserialize_page_limit"
    )]
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<u32>,
}
impl RunVectorsParams {
    pub fn validate(&self) -> Result<(), super::NativeV2RunValueError> {
        super::checkpoints::validate_page_limit(self.limit.unwrap_or(50))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunVectorsResult {
    pub run_id: RunId,
    #[schemars(length(max = 100))]
    pub vectors: Vec<RunVector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_after: Option<VectorId>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunVectorParams {
    pub run_id: RunId,
    pub id: VectorId,
}

/// Full vector content. Kept separate from page response bounds.
pub type RunVectorResult = RunVector;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VectorSuspensionReason {
    Command,
    Wait,
}

/// Independent bound for a full entry response, including one ledger event and framing.
pub const MAX_VECTOR_RESPONSE_BYTES: usize = 1024 * 1024 + 1024;

fn deserialize_record_run_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<RunId, D::Error> {
    let value = String::deserialize(deserializer)?;
    VectorId::new(value.clone()).map_err(serde::de::Error::custom)?;
    Ok(RunId::new(value))
}

impl RunVector {
    pub fn validate(&self) -> Result<(), super::NativeV2RunValueError> {
        VectorId::new(self.run_id.as_str())?;
        if let RunVectorContent::Continuation { resumed_from } = &self.content {
            VectorId::new(resumed_from.as_str())?;
        }
        Ok(())
    }
}
