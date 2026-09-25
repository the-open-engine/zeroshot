//! Secret-free preparation shared by every node in one run attempt.
use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    DeclaredConnections, EnvironmentId, EnvironmentName, EnvironmentRevision,
    EnvironmentVariableName, NativeV2RunValueError,
};

pub const MAX_RUNTIME_SCRIPT_BYTES: usize = 64 * 1024;
pub const MAX_RUNTIME_VARIABLE_BYTES: usize = 256 * 1024;

/// Setup runs as root before checkout; startup runs as the workspace user after checkout or restore.
/// Both hooks run again on a new attempt. Values here are public environment data, never secrets.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimeEnvironment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<EnvironmentVariableName, String>,
    /// Exact connection fields available to setup and startup, separate from node credentials.
    #[serde(default, skip_serializing_if = "DeclaredConnections::is_empty")]
    pub connections: DeclaredConnections,
}

impl RuntimeEnvironment {
    pub fn validate(&self) -> Result<(), NativeV2RunValueError> {
        for script in [&self.setup, &self.startup].into_iter().flatten() {
            if script.len() > MAX_RUNTIME_SCRIPT_BYTES || script.contains('\0') {
                return Err(NativeV2RunValueError(
                    "environment scripts must be at most 65536 bytes and contain no NUL",
                ));
            }
        }
        if self.variables.len() > 64
            || self
                .variables
                .iter()
                .map(|(name, value)| name.as_str().len() + value.len())
                .sum::<usize>()
                > MAX_RUNTIME_VARIABLE_BYTES
            || self.variables.values().any(|value| value.contains('\0'))
        {
            return Err(NativeV2RunValueError(
                "environment variables exceed 64 names or 256 KiB, or contain NUL",
            ));
        }
        for name in self
            .variables
            .keys()
            .chain(self.connections.environment_names())
        {
            if reserved_variable(name.as_str()) {
                return Err(NativeV2RunValueError(
                    "environment variable name is reserved by the target",
                ));
            }
        }
        if self
            .connections
            .environment_names()
            .any(|name| self.variables.contains_key(name))
        {
            return Err(NativeV2RunValueError(
                "environment variables and connections must have distinct names",
            ));
        }
        Ok(())
    }
}

fn reserved_variable(name: &str) -> bool {
    matches!(
        name,
        "HOME"
            | "PATH"
            | "TMPDIR"
            | "ZEROSHOT_TOOLS"
            | "CODEX_HOME"
            | "CLAUDE_CONFIG_DIR"
            | "COPILOT_HOME"
    )
}

/// Stable reference resolved by the resource owner before a run is accepted.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentReference {
    pub id: EnvironmentId,
}

/// A reusable definition. The hosting store owns scope and authorization.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimeEnvironmentResource {
    pub id: EnvironmentId,
    pub name: EnvironmentName,
    pub definition: RuntimeEnvironment,
    pub revision: EnvironmentRevision,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimeEnvironmentSummary {
    pub id: EnvironmentId,
    pub name: EnvironmentName,
    pub revision: EnvironmentRevision,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimeEnvironmentListResult {
    pub environments: Vec<RuntimeEnvironmentSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimeEnvironmentSaveRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<EnvironmentId>,
    pub name: EnvironmentName,
    pub definition: RuntimeEnvironment,
    #[serde(deserialize_with = "required_environment_revision")]
    pub expected_revision: Option<EnvironmentRevision>,
}

fn required_environment_revision<'de, D: serde::Deserializer<'de>>(
    value: D,
) -> Result<Option<EnvironmentRevision>, D::Error> {
    Option::<EnvironmentRevision>::deserialize(value)
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimeEnvironmentDeleteRequest {
    pub id: EnvironmentId,
    pub expected_revision: EnvironmentRevision,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimeEnvironmentDeleteResult {
    pub deleted: bool,
}

pub const RUNTIME_ENVIRONMENTS_KIND: &str = "zeroshot.runtime-environments/v1";

/// Read-only resource discovery used when previewing a hosted profile's connection requirements.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TargetRuntimeEnvironmentsDiscovery {
    pub kind: String,
    pub base_url: String,
    pub route_templates: TargetRuntimeEnvironmentRoutes,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetRuntimeEnvironmentRoutes {
    /// GET route with exact {scope} and {environment_id} path segments.
    pub show: String,
}

#[cfg(test)]
mod tests;
