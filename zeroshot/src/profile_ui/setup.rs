//! Secret-free access discovery using the same contracts as hosted profile storage.

use openengine_cluster_protocol::RuntimePlan;
use serde::Deserialize;
use serde_json::{json, Value};

use super::WorkspaceError;
use crate::native_v2_capsule::gateway;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    runtime: RuntimePlan,
}

pub(super) fn apply(
    value: Value,
    harnesses: &crate::harnesses::HarnessRegistry,
) -> Result<Value, WorkspaceError> {
    let request: Request = serde_json::from_value(value).map_err(invalid)?;
    let mut runtime = request.runtime;
    crate::native_v2_hosting::materialize_contained_provider_access_with_registry(
        &mut runtime,
        harnesses,
    )
    .map_err(invalid)?;
    Ok(json!({"requirements":runtime.connection_requirements(), "runtime":runtime}))
}

pub(super) fn lanes(
    harnesses: &crate::harnesses::HarnessRegistry,
) -> Result<Vec<Value>, WorkspaceError> {
    harnesses
        .lanes()
        .into_iter()
        .map(|lane| {
            let access = harnesses.access(&lane).map_err(invalid)?;
            let mut entry = json!({
                "harness":lane.harness_name(), "provider":lane.provider_name(),
                "connectionKey":access.connection_key, "requiredFields":access.required_fields,
                "acceptedFieldSets":access.accepted_field_sets,
            });
            if let Some(protocols) = access.gateway_protocols {
                entry["fieldOptions"] = json!({
                    (gateway::API): {
                        "description": "Wire protocol spoken by the gateway endpoint.",
                        "allowedValues": protocols,
                    }
                });
            }
            Ok(entry)
        })
        .collect()
}

fn invalid(error: impl std::fmt::Display) -> WorkspaceError {
    WorkspaceError::invalid(error.to_string())
}

#[cfg(test)]
#[path = "setup/tests.rs"]
mod tests;
