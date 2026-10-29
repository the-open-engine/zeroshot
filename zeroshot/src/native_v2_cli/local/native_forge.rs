//! Private native forge credentials, scoped to the admitted operation and its exact instance.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use openengine_cluster_protocol::{
    ConnectionKey, EnvironmentVariableName, GitRemoteUrl, NodeRuntimeBinding, ResolvedSource,
    RunConnectionValues, RunSubmission, SourceForgeBaseUrl, SourceForgeIdentity,
    StaticConnectionValues,
};

use super::connections::LocalConnectionStore;
use super::{NativeV2CliError, local_message};

#[path = "native_forge/process.rs"]
mod process;
use process::{environment_token, stored_token};

const GITHUB: &str = "source.github";
const GITLAB: &str = "source.gitlab";

pub(crate) fn needs_forge(capabilities: &[crate::source_code_provider::SourceCapability]) -> bool {
    capabilities.iter().any(|capability| {
        !matches!(
            capability,
            crate::source_code_provider::SourceCapability::Read
                | crate::source_code_provider::SourceCapability::Push
        )
    })
}

#[derive(Clone, Copy)]
pub(crate) struct NativeContext<'a> {
    pub environment: &'a BTreeMap<String, String>,
    pub directory: &'a Path,
}

pub(crate) fn capture_native_environment(
    invoking_directory: &Path,
) -> Result<BTreeMap<String, String>, NativeV2CliError> {
    crate::native_v2_local::capture_local_native_environment(invoking_directory)
        .map_err(super::local_error)
}

/// Resolve an explicitly selected local instance before the backend validates source identity.
pub(crate) fn select_local_source(
    run: &crate::native_v2_cli::RunCommand,
    request: &mut crate::native_v2_cli::PreparedRunRequest,
) -> Result<(), NativeV2CliError> {
    let Some(selection) = run.source_forge.as_ref().filter(|_| run.target.is_none()) else {
        return Ok(());
    };
    if request.source.is_none() {
        let invoking =
            std::env::current_dir().map_err(|_| local_message("invoking folder is unavailable"))?;
        let selected = run.cwd.as_deref().unwrap_or(&invoking);
        let selected = if selected.is_absolute() {
            selected.to_owned()
        } else {
            invoking.join(selected)
        };
        let (_, source) =
            crate::native_v2_local::local_resolved_source(&selected, Path::new("git"))
                .map_err(super::local_error)?;
        request.source = source.map(|resolved| crate::native_v2_cli::NamedRunSource {
            resolved,
            dirty: false,
        });
    }
    let source = request.source.as_mut().ok_or_else(|| {
        local_message("explicit source instances require an attached Git checkout")
    })?;
    let remote = source
        .resolved
        .remote
        .as_mut()
        .ok_or_else(|| local_message("explicit source instances require a Git remote"))?;
    let forge = crate::source_code_provider::adapters::core::forge_identity_for(
        &remote.url,
        selection.provider.as_str(),
        &selection.base_url,
    )
    .map_err(|error| local_message(error.to_string()))?;
    source.resolved.repository = forge.repository.clone();
    remote.forge = Some(forge);
    remote.connection = run.source_connection.clone();
    Ok(())
}

pub(crate) async fn enrich_source(
    source: &mut ResolvedSource,
    context: NativeContext<'_>,
) -> Result<(), NativeV2CliError> {
    let Some(remote) = source
        .remote
        .as_mut()
        .filter(|remote| remote.forge.is_none())
    else {
        return Ok(());
    };
    let forge = identify_remote(&remote.url, context).await?;
    source.repository = forge.repository.clone();
    remote.forge = Some(forge);
    Ok(())
}

pub(crate) async fn identify_remote(
    remote: &GitRemoteUrl,
    context: NativeContext<'_>,
) -> Result<SourceForgeIdentity, NativeV2CliError> {
    let base = remote_base(remote)?;
    let hostname = instance_hostname(base.as_str())?;
    let (github, gitlab) = tokio::join!(
        stored_token(GITHUB, &hostname, context),
        stored_token(GITLAB, &hostname, context),
    );
    let provider = match (github?, gitlab?) {
        (Some(_), None) => GITHUB,
        (None, Some(_)) => GITLAB,
        (Some(_), Some(_)) => {
            return Err(local_message(
                "both gh and glab have credentials for this remote; select an explicit forge identity",
            ));
        }
        (None, None) => {
            return Err(local_message(format!(
                "cannot identify the selected Git server; sign in to {hostname} with gh or glab",
            )));
        }
    };
    crate::source_code_provider::adapters::core::forge_identity_for(remote, provider, &base)
        .map_err(|error| local_message(error.to_string()))
}

fn remote_base(remote: &GitRemoteUrl) -> Result<SourceForgeBaseUrl, NativeV2CliError> {
    let value = remote.as_str();
    let parsed = if value.contains("://") {
        url::Url::parse(value)
    } else {
        let delimiter = value
            .find("]:")
            .map(|index| index + 1)
            .or_else(|| value.find(':'))
            .ok_or_else(|| local_message("Git remote has no hostname"))?;
        let (authority, path) = value.split_at(delimiter);
        url::Url::parse(&format!("ssh://{authority}/{}", &path[1..]))
    }
    .map_err(|_| local_message("Git remote has no valid hostname"))?;
    let host = parsed
        .host()
        .ok_or_else(|| local_message("Git remote has no hostname"))?;
    let authority = match (parsed.scheme(), parsed.port()) {
        ("https", Some(port)) => format!("{host}:{port}"),
        _ => host.to_string(),
    };
    SourceForgeBaseUrl::new(format!("https://{authority}"))
        .map_err(|error| local_message(error.to_string()))
}

fn instance_hostname(base: &str) -> Result<String, NativeV2CliError> {
    let parsed =
        url::Url::parse(base).map_err(|_| local_message("forge instance has no valid hostname"))?;
    let host = parsed
        .host()
        .ok_or_else(|| local_message("forge instance has no hostname"))?;
    Ok(parsed
        .port()
        .map_or_else(|| host.to_string(), |port| format!("{host}:{port}")))
}

fn credential_field(forge: &SourceForgeIdentity) -> Option<&'static str> {
    match forge.provider.as_str() {
        GITHUB => Some("GH_TOKEN"),
        GITLAB => Some("GITLAB_TOKEN"),
        _ => None,
    }
}

fn base_url(forge: &SourceForgeIdentity) -> Result<SourceForgeBaseUrl, NativeV2CliError> {
    crate::source_code_provider::adapters::core::forge_base_url(forge)
        .map_err(|error| local_message(error.to_string()))
}

pub(super) fn materialize_local_access(
    submission: &mut RunSubmission,
    context: NativeContext<'_>,
    trusted_github_token: bool,
) -> Result<(), NativeV2CliError> {
    let retain_empty = if trusted_github_token
        || context
            .environment
            .contains_key(crate::native_v2_delivery::GITHUB_TOKEN_FILE_ENV)
    {
        submission
            .runtime
            .nodes()
            .iter()
            .filter_map(|(name, binding)| {
                if !binding.declared_connections().is_empty() {
                    return None;
                }
                let forge = match binding {
                    NodeRuntimeBinding::GitDelivery { .. } => source_forge(submission),
                    NodeRuntimeBinding::CommentDelivery { .. } => {
                        comment_forge(submission).ok().flatten()
                    }
                    NodeRuntimeBinding::Agent { .. } | NodeRuntimeBinding::Decision { .. } => None,
                }?;
                (forge.provider.as_str() == GITHUB
                    && base_url(&forge).is_ok_and(|base| base.as_str() == "https://github.com"))
                .then(|| (name.clone(), binding.clone()))
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    crate::native_v2_admission::materialize_delivery_access(submission)
        .map_err(NativeV2CliError::InvalidRun)?;
    for (name, binding) in retain_empty {
        submission.runtime.nodes_mut().insert(name, binding);
    }
    Ok(())
}

pub(super) async fn fill_missing(
    submission: &RunSubmission,
    connections: &mut RunConnectionValues,
    store: &LocalConnectionStore,
    context: NativeContext<'_>,
) -> Result<(), NativeV2CliError> {
    let missing = store.missing_submission_connections(submission, connections)?;
    let selections = missing_forge_connections(submission, &missing)?;
    for (key, forge) in selections {
        let token = token_for(&forge, context).await?;
        connections.insert(key, native_values(&forge, token)?);
    }
    Ok(())
}

fn native_values(
    forge: &SourceForgeIdentity,
    token: String,
) -> Result<StaticConnectionValues, NativeV2CliError> {
    let field = credential_field(forge)
        .ok_or_else(|| local_message("selected forge has no native credential field"))?;
    let field =
        EnvironmentVariableName::new(field).map_err(|error| local_message(error.to_string()))?;
    StaticConnectionValues::new(BTreeMap::from([(field, token)]))
        .map_err(|error| local_message(error.to_string()))
}

pub(super) async fn token_for(
    forge: &SourceForgeIdentity,
    context: NativeContext<'_>,
) -> Result<String, NativeV2CliError> {
    let host = instance_hostname(base_url(forge)?.as_str())?;
    if let Some(token) = environment_token(forge.provider.as_str(), &host, context.environment)? {
        return Ok(token);
    }
    stored_token(forge.provider.as_str(), &host, context)
        .await?
        .ok_or_else(|| local_message(format!(
            "forge login for {host} is unavailable; sign in with {} or supply its delivery connection",
            if forge.provider.as_str() == GITLAB { "glab" } else { "gh" },
        )))
}

fn missing_forge_connections(
    submission: &RunSubmission,
    missing: &BTreeSet<ConnectionKey>,
) -> Result<BTreeMap<ConnectionKey, SourceForgeIdentity>, NativeV2CliError> {
    let mut selected = BTreeMap::new();
    let requirements: openengine_cluster_protocol::RunConnectionRequirements = submission
        .connection_requirements()
        .into_iter()
        .map(|(key, names)| (key, names.into_iter().collect()))
        .collect();
    for binding in submission.runtime.nodes().values() {
        let forge = binding_forge(submission, binding)?;
        let Some(forge) = forge else { continue };
        let Some(field) = credential_field(&forge) else {
            continue;
        };
        for (key, names) in binding.declared_connections().iter() {
            if !missing.contains(key) || !native_requirement(&requirements, key, names, field) {
                continue;
            }
            insert_selection(&mut selected, key.clone(), forge.clone())?;
        }
    }
    Ok(selected)
}

fn binding_forge(
    submission: &RunSubmission,
    binding: &NodeRuntimeBinding,
) -> Result<Option<SourceForgeIdentity>, NativeV2CliError> {
    match binding {
        NodeRuntimeBinding::GitDelivery { .. } => Ok(source_forge(submission)),
        NodeRuntimeBinding::CommentDelivery { .. } => comment_forge(submission),
        NodeRuntimeBinding::Agent { .. } | NodeRuntimeBinding::Decision { .. } => Ok(None),
    }
}

fn native_requirement(
    requirements: &openengine_cluster_protocol::RunConnectionRequirements,
    key: &ConnectionKey,
    names: &openengine_cluster_protocol::DeclaredEnvironment,
    field: &str,
) -> bool {
    requirements
        .get(key)
        .is_some_and(|required| required.len() == 1)
        && names.iter().any(|name| name.as_str() == field)
}

fn insert_selection(
    selected: &mut BTreeMap<ConnectionKey, SourceForgeIdentity>,
    key: ConnectionKey,
    forge: SourceForgeIdentity,
) -> Result<(), NativeV2CliError> {
    if let Some(previous) = selected.get(&key)
        && (previous.provider != forge.provider || base_url(previous)? != base_url(&forge)?)
    {
        return Err(local_message(format!(
            "connection {key} targets different forge instances; select separate connections",
        )));
    }
    selected.insert(key, forge);
    Ok(())
}

fn source_forge(submission: &RunSubmission) -> Option<SourceForgeIdentity> {
    let source = submission.source.as_ref()?;
    match &source.remote {
        Some(remote) => remote.forge.clone(),
        None => Some(SourceForgeIdentity {
            provider: openengine_cluster_protocol::SourceForgeProviderId::new(GITHUB).ok()?,
            repository: source.repository.clone(),
            base_url: None,
        }),
    }
}

fn comment_forge(
    submission: &RunSubmission,
) -> Result<Option<SourceForgeIdentity>, NativeV2CliError> {
    let source = source_forge(submission);
    let Some(input) = crate::native_v2_admission::resolve_comment_input_at(
        &submission.graph.root,
        &submission.initial_input,
        source.as_ref(),
    )
    .map_err(|error| local_message(error.to_string()))?
    else {
        return Ok(None);
    };
    let destination = input.destination;
    Ok(Some(SourceForgeIdentity {
        provider: openengine_cluster_protocol::SourceForgeProviderId::new(
            destination.provider.id().as_str(),
        )
        .map_err(|error| local_message(error.to_string()))?,
        repository: openengine_cluster_protocol::SourceRepositoryId::new(destination.repository)
            .map_err(|error| local_message(error.to_string()))?,
        base_url: destination
            .base_url
            .map(SourceForgeBaseUrl::new)
            .transpose()
            .map_err(|error| local_message(error.to_string()))?,
    }))
}

#[cfg(test)]
#[path = "native_forge/tests.rs"]
mod tests;
