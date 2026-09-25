//! Native authoring services shared by standalone and hosted browser workspaces.
//! These operations only produce drafts or validate them; they never store profiles or run graphs.

use openengine_cluster_protocol::{GraphSpec, ProfileRuntimePlan};
use serde_json::{json, Value};

use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission};
use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};

#[path = "profile_ui/catalog.rs"]
mod catalog;
#[path = "profile_ui/data.rs"]
mod data;
#[path = "profile_ui/outcomes.rs"]
mod outcomes;

/// A native workspace problem, independent of an HTTP framework or profile store.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct WorkspaceError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl WorkspaceError {
    fn invalid(message: String) -> Self {
        Self {
            status: 422,
            code: "invalid_profile",
            message,
        }
    }
    fn internal(message: String) -> Self {
        Self {
            status: 500,
            code: "profile_store_error",
            message,
        }
    }
}

/// Native templates, worker contracts and runtime schema. The host adds workspace identity.
pub fn catalog() -> Result<Value, WorkspaceError> {
    Ok(json!({
        "version": 1,
        "templates": catalog::templates()?,
        "workers": catalog::workers()?,
        "runtimeSchema": schemars::schema_for!(ProfileRuntimePlan),
    }))
}

/// Apply a graph outcome operation to a draft without persisting or admitting it.
pub fn author(request: Value) -> Result<Value, WorkspaceError> {
    let request = serde_json::from_value(request).map_err(decode_error)?;
    serde_json::to_value(outcomes::apply(request)?)
        .map_err(|error| WorkspaceError::internal(error.to_string()))
}

/// Apply an input/output operation using the native graph's data-flow rules.
pub fn data(request: Value) -> Result<Value, WorkspaceError> {
    let request = serde_json::from_value(request).map_err(decode_error)?;
    serde_json::to_value(data::apply(request)?)
        .map_err(|error| WorkspaceError::internal(error.to_string()))
}

/// Validate a stored profile using the same admission policy as the standalone editor.
pub async fn validate_profile(
    graph: &GraphSpec,
    runtime: &ProfileRuntimePlan,
) -> Result<(), WorkspaceError> {
    NativeV2Admission
        .validate_profile(
            graph,
            &runtime.clone().map_environment(|_| None),
            DeliveryPolicy::Optional,
        )
        .await
        .map_err(|error| WorkspaceError::invalid(error.to_string()))
}

fn decode_error(error: serde_json::Error) -> WorkspaceError {
    WorkspaceError::invalid(format!("Invalid profile JSON: {error}"))
}
