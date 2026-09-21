//! Discoverable, secret-free node and atomic-group entry checkpoints.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{Generation, NodeName, PositiveInteger, RunId, UnixTimestampMillis};

use super::NativeV2RunValueError;

pub const DEFAULT_CHECKPOINT_PAGE_SIZE: u32 = 50;
pub const MAX_CHECKPOINT_PAGE_SIZE: u32 = 100;

/// Opaque run-scoped checkpoint identity. Clients must not interpret its spelling.
#[derive(
    Clone, Debug, Deserialize, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(transparent)]
pub struct CheckpointId(crate::value::BoundedString256);

impl CheckpointId {
    pub fn new(value: impl Into<String>) -> Result<Self, NativeV2RunValueError> {
        crate::value::BoundedString256::new(value)
            .map(Self)
            .map_err(NativeV2RunValueError)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for CheckpointId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A new successor either restarts the graph on the latest workspace or continues from an entry
/// checkpoint with its matching predecessor outputs. Provider sessions are never resumed.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum RunResumeFrom {
    Restart {},
    Checkpoint {
        #[serde(rename = "checkpointId")]
        checkpoint_id: CheckpointId,
    },
}

/// A durable entry point immediately before the named node or outer concurrent group starts.
/// Parallel and mapped groups are atomic: their children and all map waves share one entry point.
/// Workspace bytes, storage locations, execution seeds, and credentials remain target-private.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunCheckpoint {
    pub checkpoint_id: CheckpointId,
    /// Positive JavaScript-safe sequence, increasing within this run's checkpoint inventory.
    pub sequence: PositiveInteger,
    pub node: NodeName,
    #[serde(deserialize_with = "deserialize_occurrence_indices")]
    #[schemars(with = "Vec<Generation>")]
    pub map_indices: Vec<u64>,
    #[serde(deserialize_with = "deserialize_occurrence_indices")]
    #[schemars(with = "Vec<Generation>")]
    pub loop_iterations: Vec<u64>,
    pub created_at: UnixTimestampMillis,
}

/// Lists retained entry checkpoints in increasing sequence order. `after` is exclusive and must
/// identify a checkpoint in this run. Omitted `limit` means 50; accepted limits are 1 through 100.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunCheckpointsParams {
    pub run_id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<CheckpointId>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_page_limit"
    )]
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<u32>,
}

impl RunCheckpointsParams {
    #[must_use]
    pub fn page_limit(&self) -> u32 {
        self.limit.unwrap_or(DEFAULT_CHECKPOINT_PAGE_SIZE)
    }

    /// Also validates values built directly by Rust callers, without a serialization boundary.
    pub fn validate(&self) -> Result<(), NativeV2RunValueError> {
        validate_page_limit(self.page_limit())
    }
}

/// One bounded page. `nextAfter` is the last returned checkpoint identity only when more remain.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunCheckpointsResult {
    pub run_id: RunId,
    #[serde(deserialize_with = "deserialize_checkpoints")]
    #[schemars(length(max = 100))]
    pub checkpoints: Vec<RunCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_after: Option<CheckpointId>,
}

fn validate_page_limit(value: u32) -> Result<(), NativeV2RunValueError> {
    if (1..=MAX_CHECKPOINT_PAGE_SIZE).contains(&value) {
        Ok(())
    } else {
        Err(NativeV2RunValueError(
            "checkpoint limit must be 1 through 100",
        ))
    }
}

fn deserialize_page_limit<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<PositiveInteger>::deserialize(deserializer)?
        .map(|value| {
            let value = u32::try_from(value.get()).map_err(serde::de::Error::custom)?;
            validate_page_limit(value).map_err(serde::de::Error::custom)?;
            Ok(value)
        })
        .transpose()
}

fn deserialize_occurrence_indices<'de, D>(deserializer: D) -> Result<Vec<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    Vec::<Generation>::deserialize(deserializer)
        .map(|values| values.into_iter().map(Generation::get).collect())
}

fn deserialize_checkpoints<'de, D>(deserializer: D) -> Result<Vec<RunCheckpoint>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<RunCheckpoint>::deserialize(deserializer)?;
    if values.len() > MAX_CHECKPOINT_PAGE_SIZE as usize {
        return Err(serde::de::Error::custom(
            "checkpoint page exceeds 100 entries",
        ));
    }
    Ok(values)
}
