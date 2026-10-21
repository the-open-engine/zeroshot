use std::io::Write;

use openengine_cluster_protocol::{
    ConnectionScope, RunProfileDefaultRequest, RunProfileListRequest, RunProfileScope,
    RunProfileSelector, RunProfileSetRequest,
};

use super::super::{
    CliOutcome, NativeV2CliBackend, NativeV2CliCommand, NativeV2CliError, ProfileSetCommand,
};
use super::submission::materialize_profile;
use super::write_json;
use crate::native_v2_candidate::{ProviderAccessPlacement, materialize_provider_access};

pub(super) async fn execute_profile<B, W>(
    command: NativeV2CliCommand,
    backend: &B,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    W: Write,
{
    let result = match command {
        NativeV2CliCommand::ProfileList(route) => list(route, backend, output).await,
        NativeV2CliCommand::ProfileShow { route, name } => show(route, name, backend, output).await,
        NativeV2CliCommand::ProfileSet(command) => set(command, backend, output).await,
        NativeV2CliCommand::ProfileRemove { route, name } => {
            remove(route, name, backend, output).await
        }
        NativeV2CliCommand::ProfileDefault { route, name } => {
            set_default(route, name, backend, output).await
        }
        _ => {
            return Err(NativeV2CliError::Usage(
                "expected a profile operation".to_owned(),
            ));
        }
    };
    result.map(|()| CliOutcome::Completed)
}

async fn list<B: NativeV2CliBackend>(
    route: super::super::ProfileRoute,
    backend: &B,
    output: &mut impl Write,
) -> Result<(), NativeV2CliError> {
    let scope = resolve_scope(&route, backend).await?;
    let result = backend
        .profile_list(route.target.as_deref(), RunProfileListRequest { scope })
        .await?;
    write_json(output, &result)
}

async fn show<B: NativeV2CliBackend>(
    route: super::super::ProfileRoute,
    name: openengine_cluster_protocol::RunProfileName,
    backend: &B,
    output: &mut impl Write,
) -> Result<(), NativeV2CliError> {
    let target = route.target.as_deref();
    let request = selector(resolve_scope(&route, backend).await?, name);
    let result = backend.profile_show(target, request).await?;
    write_json(output, &result)
}

async fn set<B: NativeV2CliBackend>(
    command: ProfileSetCommand,
    backend: &B,
    output: &mut impl Write,
) -> Result<(), NativeV2CliError> {
    let ProfileSetCommand {
        route,
        name,
        graph,
        runtime,
        set_default,
    } = command;
    let (graph, mut runtime) = materialize_profile(&graph, &runtime).await?;
    materialize_stored_provider_access(&mut runtime, route.target.is_some())?;
    let scope = resolve_scope(&route, backend).await?;
    let result = backend
        .profile_set(
            route.target.as_deref(),
            RunProfileSetRequest {
                name,
                scope,
                graph,
                runtime,
                set_default,
            },
        )
        .await?;
    write_json(output, &result)
}

fn materialize_stored_provider_access(
    runtime: &mut openengine_cluster_protocol::RuntimePlan,
    contained: bool,
) -> Result<(), NativeV2CliError> {
    if contained {
        materialize_provider_access(runtime, ProviderAccessPlacement::Contained)
            .map_err(|error| NativeV2CliError::Usage(error.to_string()))?;
    }
    Ok(())
}

async fn remove<B: NativeV2CliBackend>(
    route: super::super::ProfileRoute,
    name: openengine_cluster_protocol::RunProfileName,
    backend: &B,
    output: &mut impl Write,
) -> Result<(), NativeV2CliError> {
    let scope = resolve_scope(&route, backend).await?;
    let result = backend
        .profile_delete(route.target.as_deref(), selector(scope, name))
        .await?;
    write_json(output, &result)
}

fn selector(
    scope: RunProfileScope,
    name: openengine_cluster_protocol::RunProfileName,
) -> RunProfileSelector {
    RunProfileSelector { scope, name }
}

async fn set_default<B: NativeV2CliBackend>(
    route: super::super::ProfileRoute,
    name: Option<openengine_cluster_protocol::RunProfileName>,
    backend: &B,
    output: &mut impl Write,
) -> Result<(), NativeV2CliError> {
    let scope = resolve_scope(&route, backend).await?;
    let result = backend
        .profile_default(
            route.target.as_deref(),
            RunProfileDefaultRequest { scope, name },
        )
        .await?;
    write_json(output, &result)
}

async fn resolve_scope<B: NativeV2CliBackend>(
    route: &super::super::ProfileRoute,
    backend: &B,
) -> Result<RunProfileScope, NativeV2CliError> {
    match route.scope {
        Some(scope) => Ok(scope),
        None => backend
            .default_resource_scope(route.target.as_deref())
            .await
            .map(|scope| match scope {
                ConnectionScope::User => RunProfileScope::User,
                ConnectionScope::Org => RunProfileScope::Org,
            }),
    }
}

#[cfg(test)]
#[path = "profiles/tests.rs"]
mod tests;
