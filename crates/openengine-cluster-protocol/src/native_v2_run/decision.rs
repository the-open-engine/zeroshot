//! Caller-authored System One decision settings; endpoints and keys remain connection values.
use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, de};

use super::NativeV2RunValueError;

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionApi {
    Systemone,
}

/// A finite probability in the inclusive range 0..=1.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct DecisionProbability(f64);

impl Eq for DecisionProbability {}

impl DecisionProbability {
    pub fn new(value: f64) -> Result<Self, NativeV2RunValueError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(NativeV2RunValueError(
                "decision probability must be in 0..=1",
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn get(&self) -> f64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for DecisionProbability {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(f64::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

impl JsonSchema for DecisionProbability {
    fn schema_name() -> Cow<'static, str> {
        "DecisionProbability".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({"type": "number", "minimum": 0, "maximum": 1})
    }
}

#[cfg(test)]
mod tests;
