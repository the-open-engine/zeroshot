use openengine_cluster_protocol::{
    ConnectionScope, GraphSpec, RunProfile, RunProfileListRequest, RunProfileScope,
    RunProfileSelector, RuntimePlan,
};

use super::{materialize_graph, materialize_runtime, validate_graph_profile};
use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission};
use crate::native_v2_cli::{
    CliProfileListResult, LocalRunProfileStore, NativeV2CliBackend, NativeV2CliError,
    ProfileQualifier, ProfileReference, RunCommand, RunGraph, RunRuntime, RunSelection,
};

pub(super) struct ResolvedRunProfile {
    pub(super) graph: GraphSpec,
    pub(super) runtime: RuntimePlan,
    pub(super) remote_selector: Option<RunProfileSelector>,
}

struct StoredProfileLookup<'a, B> {
    target: Option<&'a str>,
    backend: &'a B,
}

pub(super) async fn resolve_run_profile<B>(
    run: &RunCommand,
    backend: &B,
) -> Result<ResolvedRunProfile, NativeV2CliError>
where
    B: NativeV2CliBackend,
{
    match &run.selection {
        RunSelection::Inline { graph, runtime } => {
            let (graph, runtime) =
                materialize_profile(graph, runtime, run.target.as_deref()).await?;
            Ok(ResolvedRunProfile {
                graph,
                runtime,
                remote_selector: None,
            })
        }
        RunSelection::Profile(reference) => {
            resolve_stored_profile(run.target.as_deref(), reference.as_ref(), backend).await
        }
    }
}

async fn resolve_stored_profile<B>(
    target: Option<&str>,
    reference: Option<&ProfileReference>,
    backend: &B,
) -> Result<ResolvedRunProfile, NativeV2CliError>
where
    B: NativeV2CliBackend,
{
    let local = LocalRunProfileStore::production()?;
    resolve_stored_profile_with_store(target, reference, backend, &local).await
}

async fn resolve_stored_profile_with_store<B: NativeV2CliBackend>(
    target: Option<&str>,
    reference: Option<&ProfileReference>,
    backend: &B,
    local: &LocalRunProfileStore,
) -> Result<ResolvedRunProfile, NativeV2CliError> {
    let lookup = StoredProfileLookup { target, backend };
    if reference
        .is_none_or(|reference| matches!(reference.qualifier, None | Some(ProfileQualifier::Local)))
        && let Some(profile) = local.expanded_for_run(reference.map(|reference| &reference.name))?
    {
        require_supported_placement(target, &profile.graph)?;
        return Ok(from_profile(profile, None));
    }
    for scope in profile_scopes(reference, target, backend).await? {
        if let Some(profile) = lookup.find(scope, reference).await? {
            return Ok(profile);
        }
    }
    Err(profile_not_found(reference))
}

impl<B: NativeV2CliBackend> StoredProfileLookup<'_, B> {
    async fn find(
        &self,
        scope: RunProfileScope,
        reference: Option<&ProfileReference>,
    ) -> Result<Option<ResolvedRunProfile>, NativeV2CliError> {
        let list = self
            .backend
            .profile_list(self.target, RunProfileListRequest { scope })
            .await?
            .into();
        let Some(selected) = select_profile(list, reference)? else {
            return Ok(None);
        };
        let selector = RunProfileSelector {
            scope: selected.scope,
            name: selected.name,
        };
        let profile = self
            .backend
            .profile_show(self.target, selector.clone())
            .await?;
        if LocalRunProfileStore::has_imports(&profile.graph) {
            self.backend
                .validate_profile_imports(self.target.ok_or_else(|| {
                    NativeV2CliError::Usage("remote profile selectors require --target".into())
                })?)
                .await?;
        }
        Ok(Some(from_profile(profile, Some(selector))))
    }
}

async fn profile_scopes<B: NativeV2CliBackend>(
    reference: Option<&ProfileReference>,
    target: Option<&str>,
    backend: &B,
) -> Result<Vec<RunProfileScope>, NativeV2CliError> {
    let scopes = match reference.and_then(|value| value.qualifier) {
        Some(ProfileQualifier::Local) => vec![],
        Some(ProfileQualifier::User) => vec![RunProfileScope::User],
        Some(ProfileQualifier::Org) => vec![RunProfileScope::Org],
        None => default_scopes(target, backend).await?,
    };
    if !scopes.is_empty() && target.is_none() {
        return Err(NativeV2CliError::Usage(
            "remote profile selectors require --target".to_owned(),
        ));
    }
    Ok(scopes)
}

async fn default_scopes<B: NativeV2CliBackend>(
    target: Option<&str>,
    backend: &B,
) -> Result<Vec<RunProfileScope>, NativeV2CliError> {
    let mut scopes = Vec::new();
    if target.is_some() {
        if backend.default_resource_scope(target).await? == ConnectionScope::User {
            scopes.push(RunProfileScope::User);
        }
        scopes.push(RunProfileScope::Org);
    }
    Ok(scopes)
}

fn select_profile(
    list: CliProfileListResult,
    reference: Option<&ProfileReference>,
) -> Result<Option<openengine_cluster_protocol::RunProfileSummary>, NativeV2CliError> {
    if let Some(problem) = list.problems.into_iter().find(|problem| match reference {
        Some(reference) => problem.name == reference.name,
        None => problem.is_default,
    }) {
        return Err(NativeV2CliError::Local(problem.message));
    }
    Ok(match reference {
        Some(reference) => list
            .profiles
            .into_iter()
            .find(|profile| profile.name == reference.name),
        None => list.profiles.into_iter().find(|profile| profile.is_default),
    })
}

fn profile_not_found(reference: Option<&ProfileReference>) -> NativeV2CliError {
    NativeV2CliError::Usage(match reference {
        Some(reference) => format!("profile {} was not found", reference.name),
        None => "no default run profile is configured".to_owned(),
    })
}

fn from_profile(
    profile: RunProfile,
    remote_selector: Option<RunProfileSelector>,
) -> ResolvedRunProfile {
    ResolvedRunProfile {
        graph: profile.graph,
        runtime: profile.runtime,
        remote_selector,
    }
}

pub(in crate::native_v2_cli::execution) async fn materialize_profile(
    selection: &RunGraph,
    runtime_source: &RunRuntime,
    target: Option<&str>,
) -> Result<(GraphSpec, RuntimePlan), NativeV2CliError> {
    let (mut graph, mut runtime) = load_profile(selection, runtime_source)?;
    require_supported_placement(target, &graph)?;
    if LocalRunProfileStore::has_imports(&graph) {
        (graph, runtime) = LocalRunProfileStore::production()?.expand(graph, runtime, None)?;
    }
    validate_profile(&graph, &runtime).await?;
    Ok((graph, runtime))
}

pub(in crate::native_v2_cli::execution) fn load_profile(
    selection: &RunGraph,
    runtime_source: &RunRuntime,
) -> Result<(GraphSpec, RuntimePlan), NativeV2CliError> {
    let graph = materialize_graph(selection)?;
    validate_graph_profile(&graph)?;
    let runtime = materialize_runtime(selection, &graph, runtime_source)?;
    Ok((graph, runtime))
}

pub(in crate::native_v2_cli::execution) async fn validate_profile(
    graph: &GraphSpec,
    runtime: &RuntimePlan,
) -> Result<(), NativeV2CliError> {
    NativeV2Admission
        .validate_profile(graph, runtime, DeliveryPolicy::Optional)
        .await
        .map_err(NativeV2CliError::InvalidRun)
}

pub(in crate::native_v2_cli::execution) fn require_supported_placement(
    target: Option<&str>,
    graph: &GraphSpec,
) -> Result<(), NativeV2CliError> {
    if target.is_some() && LocalRunProfileStore::has_imports(graph) {
        return Err(NativeV2CliError::Usage(
            "profile imports require a local run".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "profiles/tests.rs"]
mod tests;
