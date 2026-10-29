//! Pure source/destination credential requirements shared by local and contained placement.
use std::collections::BTreeMap;
use openengine_cluster_protocol::{
    ConnectionKey, DeclaredConnections, DeclaredEnvironment, EnvironmentVariableName,
    NodeRuntimeBinding, RunSubmission, SourceForgeIdentity, SourceForgeProviderId,
    SourceRepositoryId,
};
use super::NativeV2AdmissionError;
use crate::source_code_provider::adapters::core;

type Destinations = BTreeMap<ConnectionKey, Vec<SourceForgeIdentity>>;

fn invalid(error: impl std::fmt::Display) -> NativeV2AdmissionError {
    NativeV2AdmissionError::InvalidSourceDelivery(error.to_string())
}

fn source_forge(submission: &RunSubmission) -> Result<SourceForgeIdentity, NativeV2AdmissionError> {
    let source = submission
        .source
        .as_ref()
        .ok_or(NativeV2AdmissionError::DeliverySourceRequired)?;
    if let Some(remote) = &source.remote {
        return remote
            .forge
            .clone()
            .ok_or_else(|| invalid("forge delivery requires a selected source provider"));
    }
    Ok(SourceForgeIdentity {
        provider: SourceForgeProviderId::new(core::GITHUB_PROVIDER_ID).map_err(invalid)?,
        repository: source.repository.clone(),
        base_url: None,
    })
}

fn comment_forge(
    submission: &RunSubmission,
) -> Result<SourceForgeIdentity, NativeV2AdmissionError> {
    let source_forge = submission
        .source
        .as_ref()
        .and_then(|s| s.remote.as_ref())
        .and_then(|r| r.forge.as_ref());
    let input = super::comment::resolve_comment_input_at(
        &submission.graph.root,
        &submission.initial_input,
        source_forge,
    )?
    .ok_or_else(|| invalid("comment delivery destination is missing"))?;
    let destination = input.destination;
    Ok(SourceForgeIdentity {
        provider: SourceForgeProviderId::new(destination.provider.id().as_str())
            .map_err(invalid)?,
        repository: SourceRepositoryId::new(destination.repository).map_err(invalid)?,
        base_url: destination
            .base_url
            .map(openengine_cluster_protocol::SourceForgeBaseUrl::new)
            .transpose()
            .map_err(invalid)?,
    })
}

fn binding_destination(
    submission: &RunSubmission,
    binding: &NodeRuntimeBinding,
) -> Result<Option<SourceForgeIdentity>, NativeV2AdmissionError> {
    match binding {
        NodeRuntimeBinding::Agent { .. } | NodeRuntimeBinding::Decision { .. } => Ok(None),
        NodeRuntimeBinding::CommentDelivery { .. } => comment_forge(submission).map(Some),
        NodeRuntimeBinding::GitDelivery { .. } => {
            if super::delivery_mode(&submission.graph.root)
                == Some(crate::native_v2_delivery::DeliveryMode::Push)
            {
                return Ok(None);
            }
            source_forge(submission).map(Some)
        }
    }
}

/// Resolves each existing delivery connection against its own admitted destination.
/// Model-only bindings are deliberately absent, even when they share a provider account.
pub fn delivery_credential_destinations(
    submission: &RunSubmission,
) -> Result<Destinations, NativeV2AdmissionError> {
    let mut destinations: Destinations = BTreeMap::new();
    for binding in submission.runtime.nodes().values() {
        let Some(destination) = binding_destination(submission, binding)? else {
            continue;
        };
        for (key, _) in binding.declared_connections().iter() {
            let values = destinations.entry(key.clone()).or_default();
            if !values.contains(&destination) {
                values.push(destination.clone());
            }
        }
    }
    Ok(destinations)
}

fn default_connections(
    forge: &SourceForgeIdentity,
) -> Result<DeclaredConnections, NativeV2AdmissionError> {
    let (key, field) = core::forge_connection_requirement(forge.provider.as_str())
        .map_err(invalid)?
        .ok_or_else(|| {
            invalid(
                "source provider has no default delivery credential; author a connection binding",
            )
        })?;
    let environment =
        DeclaredEnvironment::new(vec![EnvironmentVariableName::new(field).map_err(invalid)?])
            .map_err(invalid)?;
    DeclaredConnections::single(key, environment).map_err(invalid)
}

/// Fills only omitted source-delivery requirements after the source is selected.
/// Explicit authored maps always retain their keys and fields.
pub fn materialize_delivery_access(
    submission: &mut RunSubmission,
) -> Result<(), NativeV2AdmissionError> {
    let mut defaults = Vec::new();
    for (name, binding) in submission.runtime.nodes() {
        if !binding.declared_connections().is_empty() {
            continue;
        }
        let Some(forge) = binding_destination(submission, binding)? else {
            continue;
        };
        defaults.push((name.clone(), default_connections(&forge)?));
    }
    for (name, connections) in defaults {
        match submission.runtime.nodes_mut().get_mut(&name) {
            Some(
                NodeRuntimeBinding::GitDelivery {
                    connections: current,
                    ..
                }
                | NodeRuntimeBinding::CommentDelivery {
                    connections: current,
                },
            ) => *current = connections,
            _ => {
                return Err(invalid(
                    "delivery binding changed while materializing access",
                ));
            }
        }
    }
    Ok(())
}
