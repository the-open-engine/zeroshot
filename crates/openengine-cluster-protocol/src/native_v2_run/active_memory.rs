//! Opt-out memory settings and the built-in run-global postmortem configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de};

use crate::NodeName;
use super::NativeV2RunValueError;

/// Retrieval controls. Repository cards and procedural graph guidance are independent.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct ActiveMemoryConfig {
    pub repository: bool,
    pub graph: bool,
    #[serde(deserialize_with = "deserialize_budget")]
    #[schemars(range(min = 1, max = 16384))]
    pub budget_bytes: u32,
}

impl Default for ActiveMemoryConfig {
    fn default() -> Self {
        Self {
            repository: true,
            graph: true,
            budget_bytes: 2048,
        }
    }
}

impl ActiveMemoryConfig {
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.repository || self.graph
    }

    pub fn validate(&self) -> Result<(), NativeV2RunValueError> {
        if !(1..=16_384).contains(&self.budget_bytes) {
            return Err(NativeV2RunValueError(
                "memory budgetBytes must be in 1..=16384",
            ));
        }
        Ok(())
    }
}

/// Only product-owned global agents can be configured. Additional keys are rejected.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct GlobalAgentsConfig {
    pub postmortem: PostmortemConfig,
}

impl GlobalAgentsConfig {
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

/// The postmortem inherits one real agent node's model, lane and declared connections.
/// Omission selects the first worker in graph order, then the first agent verifier.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct PostmortemConfig {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_node: Option<NodeName>,
    #[serde(deserialize_with = "deserialize_timeout")]
    #[schemars(range(min = 1, max = 600000))]
    pub timeout_ms: u32,
}

impl Default for PostmortemConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            source_node: None,
            timeout_ms: 120_000,
        }
    }
}

impl PostmortemConfig {
    pub fn validate(&self) -> Result<(), NativeV2RunValueError> {
        if !(1..=600_000).contains(&self.timeout_ms) {
            return Err(NativeV2RunValueError(
                "postmortem timeoutMs must be in 1..=600000",
            ));
        }
        Ok(())
    }
}

fn deserialize_budget<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    bounded_u32(
        deserializer,
        16_384,
        "memory budgetBytes must be in 1..=16384",
    )
}

fn deserialize_timeout<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    bounded_u32(
        deserializer,
        600_000,
        "postmortem timeoutMs must be in 1..=600000",
    )
}

fn bounded_u32<'de, D: Deserializer<'de>>(
    deserializer: D,
    maximum: u32,
    message: &str,
) -> Result<u32, D::Error> {
    let value = u32::deserialize(deserializer)?;
    if value == 0 || value > maximum {
        return Err(de::Error::custom(message));
    }
    Ok(value)
}

#[cfg(test)]
#[path = "active_memory/tests.rs"]
mod tests;
