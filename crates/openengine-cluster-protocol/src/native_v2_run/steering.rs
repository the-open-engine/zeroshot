//! Standing human direction, stored only in the run vector record.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::{ExecutionRef, NodeInstructions, RunId, UnixTimestampMillis, VectorId};

pub const RUN_STEER_METHOD: &str = "run/steer";

pub fn validate_steer(text: &str) -> Result<(), String> {
    NodeInstructions::new(text).map_err(|error| error.to_string())?;
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
    {
        return Err(
            "steer must not contain control characters other than tab, newline and carriage return"
                .into(),
        );
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunSteerParams {
    pub run_id: RunId,
    #[schemars(with = "NodeInstructions")]
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunSteer {
    pub id: VectorId,
    pub created_at: UnixTimestampMillis,
    pub text_bytes: usize,
    pub excerpt: String,
    pub received_executions: u64,
    pub executions: Vec<ExecutionRef>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub executions_omitted: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunSteerResult {
    pub run_id: RunId,
    pub steer: RunSteer,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunSteeringStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steers: Vec<RunSteer>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub omitted_steers: u64,
}
fn is_zero(value: &u64) -> bool {
    *value == 0
}
