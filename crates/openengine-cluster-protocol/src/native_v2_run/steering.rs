//! Human direction and graph-local node messages share the run vector record.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::{ExecutionRef, NodeInstructions, NodeName, RunId, UnixTimestampMillis, VectorId};

pub const RUN_STEER_METHOD: &str = "run/steer";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SteeringMode {
    Once,
    #[default]
    Standing,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SteerOrigin {
    pub node: NodeName,
    #[schemars(range(min = 1))]
    pub node_instance: u64,
    pub execution: ExecutionRef,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NetworkSteer {
    #[schemars(length(min = 1))]
    pub networks: Vec<NodeName>,
    pub origin: SteerOrigin,
}

impl NetworkSteer {
    pub fn validate(&self) -> Result<(), super::NativeV2RunValueError> {
        if self.networks.is_empty() || self.origin.node_instance == 0 {
            return Err(super::NativeV2RunValueError(
                "invalid network steer metadata",
            ));
        }
        Ok(())
    }
}

/// Private callback publication input. The host supplies and authorizes the origin separately.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NetworkSteerParams {
    pub run_id: RunId,
    pub text: String,
    pub mode: SteeringMode,
    pub networks: Vec<NodeName>,
}

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
    #[serde(default)]
    pub mode: SteeringMode,
    #[schemars(with = "NodeInstructions")]
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunSteer {
    pub id: VectorId,
    #[serde(default)]
    pub mode: SteeringMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkSteer>,
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
