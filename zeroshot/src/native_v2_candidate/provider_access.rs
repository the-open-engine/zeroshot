//! Target-aware provider access materialized into the existing connection contract.

use std::collections::{BTreeMap, BTreeSet};

use openengine_cluster_protocol::{
    ConnectionKey, DeclaredConnections, DeclaredEnvironment, EnvironmentVariableName,
    NativeV2RunValueError, NodeRuntimeBinding, RuntimeLane, RuntimePlan,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderAccessPlacement {
    Local,
    Contained,
}

use crate::harnesses::{HarnessRegistry, ProviderAccessContract};

pub(crate) fn materialize_provider_access(
    runtime: &mut RuntimePlan,
    placement: ProviderAccessPlacement,
) -> Result<(), crate::harnesses::HarnessError> {
    let default_lane = runtime.lane();
    for binding in runtime.nodes_mut().values_mut() {
        materialize_binding(binding, default_lane.clone(), placement)?;
    }
    Ok(())
}

fn materialize_binding(
    binding: &mut NodeRuntimeBinding,
    default_lane: RuntimeLane,
    placement: ProviderAccessPlacement,
) -> Result<(), crate::harnesses::HarnessError> {
    let NodeRuntimeBinding::Agent {
        lane, connections, ..
    } = binding
    else {
        return Ok(());
    };
    let effective_lane = lane.clone().unwrap_or(default_lane);
    let registry = HarnessRegistry::local();
    let contract = registry
        .access(&effective_lane)
        .map_err(|error| crate::harnesses::HarnessError::Configuration(error.to_string()))?;
    if supports_authored_access(connections, contract)
        || placement == ProviderAccessPlacement::Local && contract.native_local
    {
        return Ok(());
    }
    *connections = with_connection_fallback(connections, contract)?;
    Ok(())
}

fn supports_authored_access(
    connections: &DeclaredConnections,
    contract: ProviderAccessContract,
) -> bool {
    contract.accepted_field_sets.iter().any(|fields| {
        fields.iter().all(|field| {
            connections
                .environment_names()
                .any(|name| name.as_str() == *field)
        })
    })
}

fn with_connection_fallback(
    connections: &DeclaredConnections,
    contract: ProviderAccessContract,
) -> Result<DeclaredConnections, NativeV2RunValueError> {
    let mut merged = connections
        .iter()
        .map(|(key, fields)| (key.clone(), fields.iter().cloned().collect::<BTreeSet<_>>()))
        .collect::<BTreeMap<_, _>>();
    let existing = connections
        .environment_names()
        .cloned()
        .collect::<BTreeSet<_>>();
    let fallback = merged
        .entry(ConnectionKey::new(contract.connection_key)?)
        .or_default();
    for name in contract.required_fields {
        let name = EnvironmentVariableName::new(*name)?;
        if !existing.contains(&name) {
            fallback.insert(name);
        }
    }
    DeclaredConnections::new(
        merged
            .into_iter()
            .map(|(key, fields)| DeclaredEnvironment::new(fields).map(|fields| (key, fields)))
            .collect::<Result<Vec<_>, _>>()?,
    )
}

#[cfg(test)]
#[path = "provider_access/tests.rs"]
mod tests;
