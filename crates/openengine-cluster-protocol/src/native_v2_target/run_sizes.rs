//! Target-owned machine identities and descriptions, independent of provider lanes or billing.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{NativeV2RunValueError, RunSize};

pub const RUN_SIZES_KIND: &str = "openengine.run-sizes/v2";
pub const MAX_TARGET_RUN_SIZES: usize = 128;
pub const MAX_RUN_SIZE_DESCRIPTION_BYTES: usize = 1_024;

pub(super) fn deserialize_catalog<'de, D>(
    deserializer: D,
) -> Result<Option<TargetRunSizesDiscovery>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(None);
    };
    if value.get("kind").and_then(serde_json::Value::as_str) != Some(RUN_SIZES_KIND) {
        return Ok(None);
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

/// A target-owned size identifier and its human-readable resource description.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "RunSizeDescriptorWire")]
pub struct RunSizeDescriptor {
    pub id: RunSize,
    pub description: String,
}

impl RunSizeDescriptor {
    pub fn new(id: RunSize, description: impl Into<String>) -> Result<Self, NativeV2RunValueError> {
        let descriptor = Self {
            id,
            description: description.into(),
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    fn validate(&self) -> Result<(), NativeV2RunValueError> {
        if self.description.trim().is_empty()
            || self.description.len() > MAX_RUN_SIZE_DESCRIPTION_BYTES
            || self.description.chars().any(char::is_control)
        {
            return Err(NativeV2RunValueError(
                "run size description must be 1..=1024 non-control bytes",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunSizeDescriptorWire {
    id: RunSize,
    description: String,
}

impl TryFrom<RunSizeDescriptorWire> for RunSizeDescriptor {
    type Error = NativeV2RunValueError;

    fn try_from(wire: RunSizeDescriptorWire) -> Result<Self, Self::Error> {
        Self::new(wire.id, wire.description)
    }
}

/// An empty catalog offers no sizes. Every nonempty catalog names its own default.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "TargetRunSizesDiscoveryWire")]
pub struct TargetRunSizesDiscovery {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<RunSize>,
    pub sizes: Vec<RunSizeDescriptor>,
}

impl TargetRunSizesDiscovery {
    pub fn new(
        default: Option<RunSize>,
        sizes: Vec<RunSizeDescriptor>,
    ) -> Result<Self, NativeV2RunValueError> {
        let catalog = Self {
            kind: RUN_SIZES_KIND.to_owned(),
            default,
            sizes,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    /// Checks descriptors constructed directly by target implementations too.
    pub fn validate(&self) -> Result<(), NativeV2RunValueError> {
        if self.kind != RUN_SIZES_KIND {
            return Err(NativeV2RunValueError("unsupported run size catalog kind"));
        }
        if self.sizes.len() > MAX_TARGET_RUN_SIZES {
            return Err(NativeV2RunValueError(
                "run size catalog must contain at most 128 sizes",
            ));
        }
        let ids = self
            .sizes
            .iter()
            .map(|descriptor| &descriptor.id)
            .collect::<BTreeSet<_>>();
        if ids.len() != self.sizes.len() {
            return Err(NativeV2RunValueError(
                "run size catalog contains duplicate identifiers",
            ));
        }
        if self.sizes.is_empty() != self.default.is_none()
            || self.default.as_ref().is_some_and(|id| !ids.contains(id))
        {
            return Err(NativeV2RunValueError(
                "a nonempty run size catalog must name an offered default",
            ));
        }
        for descriptor in &self.sizes {
            descriptor.validate()?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetRunSizesDiscoveryWire {
    kind: String,
    #[serde(default)]
    default: Option<RunSize>,
    sizes: Vec<RunSizeDescriptor>,
}

impl TryFrom<TargetRunSizesDiscoveryWire> for TargetRunSizesDiscovery {
    type Error = NativeV2RunValueError;

    fn try_from(wire: TargetRunSizesDiscoveryWire) -> Result<Self, Self::Error> {
        let catalog = Self {
            kind: wire.kind,
            default: wire.default,
            sizes: wire.sizes,
        };
        catalog.validate()?;
        Ok(catalog)
    }
}
