//! Host-supported machine size identities, independent of provider lanes or billing.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{NativeV2RunValueError, RunSize};

pub const RUN_SIZES_KIND: &str = "openengine.run-sizes/v1";

/// An empty list explicitly disables every size; an absent extension keeps legacy choices.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetRunSizesDiscovery {
    pub kind: String,
    #[serde(deserialize_with = "deserialize_sizes")]
    pub sizes: Vec<RunSize>,
}

impl TargetRunSizesDiscovery {
    pub fn new(sizes: Vec<RunSize>) -> Result<Self, NativeV2RunValueError> {
        validate_sizes(&sizes)?;
        Ok(Self {
            kind: RUN_SIZES_KIND.to_owned(),
            sizes,
        })
    }

    /// Also checks descriptors constructed directly by Rust hosts.
    pub fn validate(&self) -> Result<(), NativeV2RunValueError> {
        validate_sizes(&self.sizes)
    }
}

fn validate_sizes(sizes: &[RunSize]) -> Result<(), NativeV2RunValueError> {
    if sizes.iter().collect::<BTreeSet<_>>().len() != sizes.len() {
        return Err(NativeV2RunValueError(
            "run size catalog contains duplicate sizes",
        ));
    }
    Ok(())
}

fn deserialize_sizes<'de, D>(deserializer: D) -> Result<Vec<RunSize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let sizes = Vec::<RunSize>::deserialize(deserializer)?;
    validate_sizes(&sizes).map_err(serde::de::Error::custom)?;
    Ok(sizes)
}
