//! Shipped CLI entrypoint.

mod native_v2_target;

use std::io::Write;
use std::path::PathBuf;

use thiserror::Error;
use zeroshot_engine::native_v2_cli::local::{LOCAL_CONTROLLER_MODE, LocalCliBackend};
use zeroshot_engine::native_v2_cli::oecp::NamedTargetCliBackend;
use zeroshot_engine::native_v2_cli::{
    execute_native_v2_cli, parse_native_v2_args, try_execute_native_v2_preflight, serve_local_acp,
    try_execute_native_v2_static, AcpServeError, CtrlCDetachSignal, NativeV2CliCommand,
    NativeV2CliDiagnostic, NativeV2CliError, ERROR_FORMAT_ENV, JSON_ERROR_FORMAT,
};
use zeroshot_engine::native_v2_portable_controller::{PortableControllerError, run_controller_process};

use native_v2_target::{
    default_target_registry_path, serve_direct_target, FileTargetRegistry, NativeV2TargetConnector,
    TargetConnectorError, TargetHttpControlAuthority, TargetOecpWebSocketDialer, TargetServeError,
};
#[cfg(feature = "ui")]
use native_v2_target::{TargetRegistry, TargetRunHistoryTransport};

#[derive(Debug, Error)]
enum ProcessError {
    #[error(transparent)]
    Acp(#[from] AcpServeError),
    #[error(transparent)]
    Target(#[from] TargetConnectorError),
    #[error(transparent)]
    Cli(#[from] NativeV2CliError),
    #[error(transparent)]
    Portable(#[from] PortableControllerError),
    #[error(transparent)]
    Serve(#[from] TargetServeError),
    #[error("could not write CLI output: {0}")]
    Output(#[from] std::io::Error),
}

async fn run() -> Result<(), ProcessError> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if run_private_controller(&arguments).await? {
        return Ok(());
    }
    dispatch(parse_native_v2_args(arguments)?).await
}

async fn dispatch(command: NativeV2CliCommand) -> Result<(), ProcessError> {
    match command {
        NativeV2CliCommand::Acp { profile } => {
            serve_local_acp(profile).await?;
            Ok(())
        }
        NativeV2CliCommand::Ui { listen, target } => {
            serve_ui(listen, target).await?;
            Ok(())
        }
        NativeV2CliCommand::TargetServe(config) => {
            serve_direct_target(config).await?;
            Ok(())
        }
        command => run_public_command(command).await,
    }
}

async fn run_private_controller(arguments: &[std::ffi::OsString]) -> Result<bool, ProcessError> {
    let Some(bootstrap) = private_controller_bootstrap(arguments)? else {
        return Ok(false);
    };
    run_controller_process(&bootstrap).await?;
    Ok(true)
}

async fn run_public_command(command: NativeV2CliCommand) -> Result<(), ProcessError> {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    if try_execute_native_v2_static(&command, &mut output)?.is_some() {
        return Ok(());
    }
    if try_execute_native_v2_preflight(&command, &mut output)
        .await?
        .is_some()
    {
        return Ok(());
    }

    let mut detach = CtrlCDetachSignal;
    if is_local_command(&command) {
        return run_local_command(command, &mut detach, &mut output).await;
    }
    run_named_target_command(command, &mut detach, &mut output).await
}

async fn run_named_target_command(
    command: NativeV2CliCommand,
    detach: &mut CtrlCDetachSignal,
    output: &mut impl Write,
) -> Result<(), ProcessError> {
    let registry = FileTargetRegistry::new(default_target_registry_path()?);
    let connector = NativeV2TargetConnector::new(
        registry,
        TargetHttpControlAuthority::production().map_err(TargetConnectorError::Authority)?,
        TargetOecpWebSocketDialer,
    );
    let backend = NamedTargetCliBackend::new(connector);
    execute_native_v2_cli(command, &backend, detach, output).await?;
    Ok(())
}

async fn run_local_command(
    command: NativeV2CliCommand,
    detach: &mut CtrlCDetachSignal,
    output: &mut impl Write,
) -> Result<(), ProcessError> {
    let backend = LocalCliBackend::production()?;
    execute_native_v2_cli(command, &backend, detach, output).await?;
    Ok(())
}

fn is_local_command(command: &NativeV2CliCommand) -> bool {
    match command {
        NativeV2CliCommand::Run(run) => run.target.is_none(),
        NativeV2CliCommand::List { target } => target.is_none(),
        command if run_operation_is_local(command) => true,
        command if connection_operation_is_local(command) => true,
        command if profile_operation_is_local(command) => true,
        _ => false,
    }
}

fn run_operation_is_local(command: &NativeV2CliCommand) -> bool {
    match command {
        NativeV2CliCommand::Status(run)
        | NativeV2CliCommand::ForceStop(run)
        | NativeV2CliCommand::Resume(run)
        | NativeV2CliCommand::DiscardWorkspace(run) => run.target.is_none(),
        NativeV2CliCommand::Watch(command) => command.run.target.is_none(),
        NativeV2CliCommand::Logs(command) => command.run.target.is_none(),
        NativeV2CliCommand::Attach { run, .. } => run.target.is_none(),
        _ => false,
    }
}

fn connection_operation_is_local(command: &NativeV2CliCommand) -> bool {
    match command {
        NativeV2CliCommand::ConnectionList(route)
        | NativeV2CliCommand::ConnectionDelete { route, .. } => route.target.is_none(),
        NativeV2CliCommand::ConnectionSet(command) => command.route.target.is_none(),
        _ => false,
    }
}

fn profile_operation_is_local(command: &NativeV2CliCommand) -> bool {
    match command {
        NativeV2CliCommand::ProfileList(route)
        | NativeV2CliCommand::ProfileShow { route, .. }
        | NativeV2CliCommand::ProfileRemove { route, .. }
        | NativeV2CliCommand::ProfileDefault { route, .. } => route.target.is_none(),
        NativeV2CliCommand::ProfileSet(command) => command.route.target.is_none(),
        _ => false,
    }
}

fn private_controller_bootstrap(
    arguments: &[std::ffi::OsString],
) -> Result<Option<PathBuf>, NativeV2CliError> {
    if arguments
        .first()
        .is_none_or(|value| value != LOCAL_CONTROLLER_MODE)
    {
        return Ok(None);
    }
    if arguments.len() != 3 || arguments.get(1).is_none_or(|value| value != "--bootstrap") {
        return Err(NativeV2CliError::Usage(
            "private controller bootstrap arguments are malformed".to_owned(),
        ));
    }
    Ok(arguments.get(2).cloned().map(PathBuf::from))
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        write_process_error(&error);
        std::process::exit(1);
    }
}

fn write_process_error(error: &ProcessError) {
    if std::env::var(ERROR_FORMAT_ENV).as_deref() == Ok(JSON_ERROR_FORMAT) {
        let diagnostic = match error {
            ProcessError::Cli(error) => error.diagnostic(),
            error => NativeV2CliDiagnostic::target(error.to_string()),
        };
        match serde_json::to_string(&diagnostic) {
            Ok(encoded) => eprintln!("{encoded}"),
            Err(_) => eprintln!("zeroshot: {error}"),
        }
    } else {
        eprintln!("zeroshot: {error}");
    }
}

async fn serve_ui(
    listen: std::net::SocketAddr,
    target: Option<String>,
) -> Result<(), ProcessError> {
    #[cfg(feature = "ui")]
    {
        let target = target.map(resolve_ui_target).transpose()?;
        zeroshot_engine::profile_ui::serve_with_target(listen, target).await?;
        Ok(())
    }
    #[cfg(not(feature = "ui"))]
    {
        let _ = (listen, target);
        let message = concat!(
            "Build the editor with npm --prefix ui ci && npm --prefix ui run build, ",
            "then cargo build -p zeroshot --features ui"
        );
        Err(NativeV2CliError::Local(message.to_owned()).into())
    }
}

#[cfg(feature = "ui")]
fn resolve_ui_target(
    name: String,
) -> Result<zeroshot_engine::profile_ui::RunHistoryTarget, ProcessError> {
    let registry = FileTargetRegistry::new(default_target_registry_path()?);
    let target = registry.get(&name)?;
    let authority =
        TargetHttpControlAuthority::production().map_err(TargetConnectorError::Authority)?;
    let transport = TargetRunHistoryTransport::new(authority, target)
        .map_err(TargetConnectorError::Authority)?;
    Ok(zeroshot_engine::profile_ui::RunHistoryTarget::remote(
        transport,
    ))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use openengine_cluster_testkit::assertions::AssertValue;

    use super::*;

    #[test]
    fn target_serve_is_part_of_the_public_command_schema() {
        let arguments = [
            "target",
            "serve",
            "--listen",
            "127.0.0.1:8080",
            "--public-origin",
            "http://127.0.0.1:8080",
            "--storage",
            "/tmp/zeroshot-target",
        ]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
        let command = parse_native_v2_args(arguments).assert_value();
        assert!(matches!(command, NativeV2CliCommand::TargetServe(_)));
    }
}
