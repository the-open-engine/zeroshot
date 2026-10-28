use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{ConnectionKey, EnvironmentVariableName};

use super::{
    MAX_DECLARED_CONNECTIONS, MAX_DECLARED_ENVIRONMENT_NAMES, NativeV2RunValueError,
    ReasoningEffort, RuntimeLane, SessionScope,
};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct DeclaredEnvironment(BTreeSet<EnvironmentVariableName>);

impl DeclaredEnvironment {
    pub fn new(
        values: impl IntoIterator<Item = EnvironmentVariableName>,
    ) -> Result<Self, NativeV2RunValueError> {
        let values = values.into_iter().collect::<Vec<_>>();
        if values.len() > MAX_DECLARED_ENVIRONMENT_NAMES {
            return Err(NativeV2RunValueError(
                "declared environment contains more than 64 names",
            ));
        }
        let unique = values.iter().cloned().collect::<BTreeSet<_>>();
        if unique.len() != values.len() {
            return Err(NativeV2RunValueError(
                "declared environment contains a duplicate name",
            ));
        }
        Ok(Self(unique))
    }

    #[must_use]
    pub const fn empty() -> Self {
        Self(BTreeSet::new())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub const fn as_set(&self) -> &BTreeSet<EnvironmentVariableName> {
        &self.0
    }

    pub fn iter(&self) -> impl Iterator<Item = &EnvironmentVariableName> {
        self.0.iter()
    }

    #[must_use]
    pub fn contains(&self, name: &EnvironmentVariableName) -> bool {
        self.0.contains(name)
    }
}

impl<'de> Deserialize<'de> for DeclaredEnvironment {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Vec::<EnvironmentVariableName>::deserialize(deserializer)?)
            .map_err(de::Error::custom)
    }
}

impl JsonSchema for DeclaredEnvironment {
    fn schema_name() -> Cow<'static, str> {
        "DeclaredEnvironment".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let item = generator.subschema_for::<EnvironmentVariableName>();
        json_schema!({
            "type": "array",
            "items": item,
            "maxItems": 64,
            "uniqueItems": true
        })
    }
}

/// Secret-free connection requirements declared by one executable node.
///
/// Keys select stored connections. Values define the exact environment fields the node requires
/// from each selected connection. One environment name cannot be supplied by two connection keys
/// in the same node binding.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct DeclaredConnections(BTreeMap<ConnectionKey, DeclaredEnvironment>);

impl DeclaredConnections {
    pub fn new(
        values: impl IntoIterator<Item = (ConnectionKey, DeclaredEnvironment)>,
    ) -> Result<Self, NativeV2RunValueError> {
        let values = values.into_iter().collect::<Vec<_>>();
        if values.len() > MAX_DECLARED_CONNECTIONS {
            return Err(NativeV2RunValueError(
                "node declares more than 64 connections",
            ));
        }
        let unique = values.iter().cloned().collect::<BTreeMap<_, _>>();
        if unique.len() != values.len() {
            return Err(NativeV2RunValueError(
                "node declares a duplicate connection key",
            ));
        }
        if unique.values().any(DeclaredEnvironment::is_empty) {
            return Err(NativeV2RunValueError(
                "declared connection must require at least one environment name",
            ));
        }
        let names = unique
            .values()
            .flat_map(DeclaredEnvironment::iter)
            .collect::<Vec<_>>();
        if names.len() > MAX_DECLARED_ENVIRONMENT_NAMES {
            return Err(NativeV2RunValueError(
                "node connections declare more than 64 environment names",
            ));
        }
        if names.iter().copied().collect::<BTreeSet<_>>().len() != names.len() {
            return Err(NativeV2RunValueError(
                "an environment name cannot be declared by two node connections",
            ));
        }
        Ok(Self(unique))
    }

    pub fn single(
        key: impl Into<String>,
        environment: DeclaredEnvironment,
    ) -> Result<Self, NativeV2RunValueError> {
        Self::new([(ConnectionKey::new(key)?, environment)])
    }

    #[must_use]
    pub const fn empty() -> Self {
        Self(BTreeMap::new())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub const fn as_map(&self) -> &BTreeMap<ConnectionKey, DeclaredEnvironment> {
        &self.0
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ConnectionKey, &DeclaredEnvironment)> {
        self.0.iter()
    }

    pub fn environment_names(&self) -> impl Iterator<Item = &EnvironmentVariableName> {
        self.0.values().flat_map(DeclaredEnvironment::iter)
    }
}

impl<'de> Deserialize<'de> for DeclaredConnections {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(BTreeMap::<ConnectionKey, DeclaredEnvironment>::deserialize(
            deserializer,
        )?)
        .map_err(de::Error::custom)
    }
}

impl JsonSchema for DeclaredConnections {
    fn schema_name() -> Cow<'static, str> {
        "DeclaredConnections".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let value = generator.subschema_for::<DeclaredEnvironment>();
        json_schema!({
            "type": "object",
            "additionalProperties": value,
            "maxProperties": 64
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestFeedback {
    #[default]
    Consider,
    Ignore,
}

impl PullRequestFeedback {
    #[must_use]
    pub const fn is_consider(&self) -> bool {
        matches!(self, Self::Consider)
    }
}

/// Smallest accepted caller-declared model context window.
pub const MIN_MODEL_CONTEXT_WINDOW: u32 = 1;
/// Largest accepted caller-declared model context window.
pub const MAX_MODEL_CONTEXT_WINDOW: u32 = 10_000_000;
/// Smallest accepted caller-declared model output limit.
pub const MIN_MODEL_MAX_TOKENS: u32 = 1;
/// Largest accepted caller-declared model output limit.
pub const MAX_MODEL_MAX_TOKENS: u32 = 10_000_000;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum NodeRuntimeBinding {
    Agent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lane: Option<RuntimeLane>,
        model: super::ModelId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<ReasoningEffort>,
        /// Caller-owned model context window. Absent means the harness provider's own default.
        #[serde(
            default,
            rename = "contextWindow",
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_context_window"
        )]
        #[schemars(range(min = 1, max = 10_000_000))]
        context_window: Option<u32>,
        /// Caller-owned model output limit. Absent means the harness provider's own default.
        #[serde(
            default,
            rename = "maxTokens",
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_max_tokens"
        )]
        #[schemars(range(min = 1, max = 10_000_000))]
        max_tokens: Option<u32>,
        #[serde(
            default,
            rename = "sessionScope",
            skip_serializing_if = "is_execution_scope"
        )]
        session_scope: SessionScope,
        #[serde(default, skip_serializing_if = "DeclaredConnections::is_empty")]
        connections: DeclaredConnections,
    },
    CommentDelivery {
        #[serde(default, skip_serializing_if = "DeclaredConnections::is_empty")]
        connections: DeclaredConnections,
    },
    GitDelivery {
        #[serde(default, skip_serializing_if = "DeclaredConnections::is_empty")]
        connections: DeclaredConnections,
        #[serde(
            default,
            rename = "pullRequestFeedback",
            skip_serializing_if = "PullRequestFeedback::is_consider"
        )]
        pull_request_feedback: PullRequestFeedback,
    },
}

fn is_execution_scope(scope: &SessionScope) -> bool {
    *scope == SessionScope::Execution
}

impl NodeRuntimeBinding {
    #[must_use]
    pub const fn declared_connections(&self) -> &DeclaredConnections {
        match self {
            Self::Agent { connections, .. }
            | Self::GitDelivery { connections, .. }
            | Self::CommentDelivery { connections } => connections,
        }
    }

    /// Caller-declared model context window, or `None` for the provider's own default.
    #[must_use]
    pub const fn context_window(&self) -> Option<u32> {
        match self {
            Self::Agent { context_window, .. } => *context_window,
            Self::GitDelivery { .. } | Self::CommentDelivery { .. } => None,
        }
    }

    /// Caller-declared model output limit, or `None` for the provider's own default.
    #[must_use]
    pub const fn max_tokens(&self) -> Option<u32> {
        match self {
            Self::Agent { max_tokens, .. } => *max_tokens,
            Self::GitDelivery { .. } | Self::CommentDelivery { .. } => None,
        }
    }

    /// Validates values built directly by Rust callers, without a serialization boundary.
    pub fn validate_model_limits(&self) -> Result<(), NativeV2RunValueError> {
        let Self::Agent {
            context_window,
            max_tokens,
            ..
        } = self
        else {
            return Ok(());
        };
        if let Some(value) = context_window {
            validate_context_window(*value).map_err(NativeV2RunValueError)?;
        }
        if let Some(value) = max_tokens {
            validate_max_tokens(*value).map_err(NativeV2RunValueError)?;
        }
        Ok(())
    }
}

fn validate_context_window(value: u32) -> Result<(), &'static str> {
    if (MIN_MODEL_CONTEXT_WINDOW..=MAX_MODEL_CONTEXT_WINDOW).contains(&value) {
        Ok(())
    } else {
        Err("contextWindow must be in 1..=10000000")
    }
}

fn validate_max_tokens(value: u32) -> Result<(), &'static str> {
    if (MIN_MODEL_MAX_TOKENS..=MAX_MODEL_MAX_TOKENS).contains(&value) {
        Ok(())
    } else {
        Err("maxTokens must be in 1..=10000000")
    }
}

fn deserialize_context_window<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_optional_model_limit(
        deserializer,
        "contextWindow",
        MIN_MODEL_CONTEXT_WINDOW,
        MAX_MODEL_CONTEXT_WINDOW,
    )
}

fn deserialize_max_tokens<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_optional_model_limit(
        deserializer,
        "maxTokens",
        MIN_MODEL_MAX_TOKENS,
        MAX_MODEL_MAX_TOKENS,
    )
}

#[derive(Clone, Copy)]
struct ModelLimitVisitor {
    field: &'static str,
    min: u32,
    max: u32,
}

impl ModelLimitVisitor {
    fn checked<E>(self, value: i128) -> Result<Option<u32>, E>
    where
        E: de::Error,
    {
        let min = i128::from(self.min);
        let max = i128::from(self.max);
        if (min..=max).contains(&value) {
            Ok(Some(value as u32))
        } else {
            Err(E::custom(format_args!(
                "{} must be in {}..={}",
                self.field, self.min, self.max
            )))
        }
    }
}

impl<'de> de::Visitor<'de> for ModelLimitVisitor {
    type Value = Option<u32>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} in {}..={}", self.field, self.min, self.max)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(None)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(None)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_u32(self)
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.checked(i128::from(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.checked(i128::from(value))
    }

    fn visit_u128<E>(self, value: u128) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value <= u128::from(u32::MAX) {
            self.checked(value as i128)
        } else {
            self.checked(i128::from(self.max) + 1)
        }
    }

    fn visit_i128<E>(self, value: i128) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.checked(value)
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(E::custom(format_args!(
            "{} must be in {}..={}",
            self.field, self.min, self.max
        )))
    }
}

/// Deserializes an optional caller-declared model limit shared by every runtime shape.
///
/// The field name and bounds are used in the rejection so a uniform runtime configuration
/// reports the same diagnostic as an exact agent binding.
#[doc(hidden)]
pub fn deserialize_optional_model_limit<'de, D>(
    deserializer: D,
    field: &'static str,
    min: u32,
    max: u32,
) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_option(ModelLimitVisitor { field, min, max })
}

#[cfg(test)]
#[path = "runtime/tests.rs"]
mod tests;
