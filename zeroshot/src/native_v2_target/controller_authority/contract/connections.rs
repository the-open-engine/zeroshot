use std::collections::BTreeSet;

use openengine_cluster_protocol::{
    CONNECTIONS_KIND, TargetConnectionsDiscovery, TargetDiscoveryExtensions,
};
use reqwest::Url;

use super::{authority_error, capability_base_url, compile_literal_route};
use crate::native_v2_target::TargetAuthorityError;

#[derive(Clone)]
pub(in super::super) struct ConnectionsDescriptor {
    pub(in super::super) list: Url,
    pub(in super::super) set: Url,
    pub(in super::super) delete: Url,
}

pub(super) fn build_connections_descriptor(
    origin: &Url,
    extensions: &TargetDiscoveryExtensions,
) -> Result<Option<ConnectionsDescriptor>, TargetAuthorityError> {
    let Some(wire) = extensions.connections.as_ref() else {
        return Ok(None);
    };
    validate_capability(wire)?;
    let base_url = capability_base_url(origin, &wire.base_url)?;
    // The CLI does not call the target's run-scoped resolver, but discovery must prove that the
    // advertised callback route is a bounded same-origin literal before accepting the dynamic
    // connection capability.
    compile_literal_route(&base_url, &wire.route_templates.resolve, "connection")?;
    Ok(Some(ConnectionsDescriptor {
        list: compile_literal_route(&base_url, &wire.route_templates.list, "connection")?,
        set: compile_literal_route(&base_url, &wire.route_templates.set, "connection")?,
        delete: compile_literal_route(&base_url, &wire.route_templates.delete, "connection")?,
    }))
}

fn validate_capability(wire: &TargetConnectionsDiscovery) -> Result<(), TargetAuthorityError> {
    let kinds = wire.dynamic_kinds.iter().collect::<BTreeSet<_>>();
    let valid_kinds = kinds.len() == wire.dynamic_kinds.len()
        && wire.dynamic_kinds.iter().all(|kind| {
            !kind.is_empty() && kind.len() <= 128 && !kind.chars().any(char::is_control)
        });
    if wire.kind != CONNECTIONS_KIND || !valid_kinds {
        return Err(authority_error("connection discovery is incompatible"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "connections/tests.rs"]
mod tests;
