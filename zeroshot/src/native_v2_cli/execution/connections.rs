use std::collections::BTreeMap;
use std::io::{Read, Write};

use openengine_cluster_protocol::{
    ConnectionDeleteRequest, ConnectionListRequest, ConnectionScope, ConnectionSetRequest,
    EnvironmentVariableName, StaticConnectionValues,
};

use crate::native_v2_cli::{
    CliOutcome, ConnectionInput, ConnectionRoute, ConnectionSetCommand, NativeV2CliBackend,
    NativeV2CliCommand, NativeV2CliError,
};

use super::write_json;

pub(super) async fn execute_connection<B, W>(
    command: NativeV2CliCommand,
    backend: &B,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    W: Write,
{
    match command {
        NativeV2CliCommand::ConnectionList(route) => {
            execute_connection_list(route, backend, output).await
        }
        NativeV2CliCommand::ConnectionSet(command) => {
            execute_connection_set(command, backend, output, read_connection_values).await
        }
        NativeV2CliCommand::ConnectionDelete { route, key } => {
            execute_connection_delete(route, key, backend, output).await
        }
        _ => Err(NativeV2CliError::Usage(
            "expected a connection operation".to_owned(),
        )),
    }
}

async fn execute_connection_list<B: NativeV2CliBackend, W: Write>(
    route: ConnectionRoute,
    backend: &B,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError> {
    let scope = resolve_scope(&route, backend).await?;
    let result = backend
        .connection_list(route.target.as_deref(), ConnectionListRequest { scope })
        .await?;
    write_json(output, &result)?;
    Ok(CliOutcome::Completed)
}

async fn execute_connection_delete<B: NativeV2CliBackend, W: Write>(
    route: ConnectionRoute,
    key: openengine_cluster_protocol::ConnectionKey,
    backend: &B,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError> {
    let scope = resolve_scope(&route, backend).await?;
    let result = backend
        .connection_delete(
            route.target.as_deref(),
            ConnectionDeleteRequest { key, scope },
        )
        .await?;
    write_json(output, &result)?;
    Ok(CliOutcome::Completed)
}

async fn execute_connection_set<B, W>(
    command: ConnectionSetCommand,
    backend: &B,
    output: &mut W,
    read_values: fn(ConnectionInput) -> Result<StaticConnectionValues, NativeV2CliError>,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    W: Write,
{
    let ConnectionSetCommand { route, key, input } = command;
    let scope = resolve_scope(&route, backend).await?;
    let values = read_values(input)?;
    store_connection_values(
        &route,
        ConnectionSetRequest { key, scope, values },
        backend,
        output,
    )
    .await
}

async fn resolve_scope<B: NativeV2CliBackend>(
    route: &ConnectionRoute,
    backend: &B,
) -> Result<ConnectionScope, NativeV2CliError> {
    match route.scope {
        Some(scope) => Ok(scope),
        None => {
            backend
                .default_resource_scope(route.target.as_deref())
                .await
        }
    }
}

async fn store_connection_values<B, W>(
    route: &ConnectionRoute,
    request: ConnectionSetRequest,
    backend: &B,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    W: Write,
{
    let result = backend
        .connection_set(route.target.as_deref(), request)
        .await?;
    write_json(output, &result)?;
    Ok(CliOutcome::Completed)
}

fn read_connection_values(
    input: ConnectionInput,
) -> Result<StaticConnectionValues, NativeV2CliError> {
    read_connection_values_with(
        input,
        |field| rpassword::prompt_password(format!("{}: ", field.as_str())),
        || {
            let mut encoded = String::new();
            std::io::stdin().lock().read_to_string(&mut encoded)?;
            Ok(encoded)
        },
    )
}

fn read_connection_values_with<P, R>(
    input: ConnectionInput,
    mut prompt: P,
    read_stdin: R,
) -> Result<StaticConnectionValues, NativeV2CliError>
where
    P: FnMut(&EnvironmentVariableName) -> Result<String, std::io::Error>,
    R: FnOnce() -> Result<String, std::io::Error>,
{
    let values = match input {
        ConnectionInput::Prompt(fields) => fields
            .into_iter()
            .map(|field| {
                let value = prompt(&field)?;
                Ok((field, value))
            })
            .collect::<Result<BTreeMap<_, _>, std::io::Error>>()?,
        ConnectionInput::JsonStdin => {
            let encoded = read_stdin()?;
            serde_json::from_str::<BTreeMap<EnvironmentVariableName, String>>(&encoded).map_err(
                |error| NativeV2CliError::Usage(format!("connection JSON is invalid: {error}")),
            )?
        }
    };
    StaticConnectionValues::new(values).map_err(|error| NativeV2CliError::Usage(error.to_string()))
}

#[cfg(test)]
#[path = "connections/tests.rs"]
mod tests;
