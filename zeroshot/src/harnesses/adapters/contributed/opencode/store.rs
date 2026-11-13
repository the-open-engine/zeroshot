//! Private OpenCode state per node session and the one-time saved-login transfer.
//!
//! Every OpenCode process of a node session runs against private `XDG_DATA_HOME`,
//! `XDG_STATE_HOME` and `XDG_CACHE_HOME` directories inside the session's private home.
//! On local runs the user's saved login for the lane's provider is copied into that
//! private store once per session, unless the node declares its own credential. Nothing
//! is ever written back to the user's store.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::execution::driver::WorkspaceCapability;
use crate::execution::process::{LocalProcessRunner, ProcessSessionCommand};
use crate::execution::WorkspaceAccessMode;
use crate::native_v2_capsule::provider_process::{
    ProviderExecutionFiles, require_process_cleanup, send_process_input,
};
use crate::native_v2_runner::{DriverControl, DriverInvocation, NodeRunnerError};

use super::command;
use super::OpenCodeAdapter;

const MAX_STORE_OUTPUT: usize = 128 * 1024;
const STORE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

const READ_LOGIN_FAILURE: &str = "OpenCode could not read the saved login from the user's store";
const STORE_LOGIN_FAILURE: &str =
    "OpenCode could not store the saved login in the private session store";

/// Private data, state and cache directories for a session's private home.
pub(super) fn private_environment(
    home: &Path,
) -> Result<[(&'static str, String); 3], NodeRunnerError> {
    Ok([
        ("XDG_DATA_HOME", private_path(home, "data")?),
        ("XDG_STATE_HOME", private_path(home, "state")?),
        ("XDG_CACHE_HOME", private_path(home, "cache")?),
    ])
}

fn private_path(home: &Path, name: &str) -> Result<String, NodeRunnerError> {
    home.join("opencode")
        .join(name)
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| {
            NodeRunnerError::DriverDetail(
                "OpenCode private state path is not valid UTF-8".to_owned(),
            )
        })
}

pub(super) struct TransferRequest<'a> {
    pub(super) adapter: &'a OpenCodeAdapter,
    pub(super) invocation: &'a DriverInvocation,
    pub(super) files: &'a ProviderExecutionFiles,
    pub(super) control: &'a DriverControl,
}

/// Copies the user's saved login for the lane's provider into the session's private
/// store, once per session. Returns the secret strings that were imported so the
/// session can redact them.
pub(super) async fn transfer_login(
    request: TransferRequest<'_>,
) -> Result<Vec<String>, NodeRunnerError> {
    let TransferRequest {
        adapter,
        invocation,
        files,
        control,
    } = request;
    if adapter.runners.is_hosted() {
        return Ok(Vec::new());
    }
    let declared = invocation.environment.iter().any(|(name, _)| {
        Some(name.as_str()) == command::credential_field(&adapter.config.provider)
    });
    if declared {
        return Ok(Vec::new());
    }
    let export_environment = store_environment(adapter, files)?;
    let exported = run_store_process(StoreProcessRequest {
        adapter,
        files,
        control,
        environment: &export_environment,
        arguments: &["auth", "export", "--standalone"],
        input: None,
        failure: READ_LOGIN_FAILURE,
        capture: true,
    })
    .await?
    .ok_or_else(|| NodeRunnerError::DriverDetail(READ_LOGIN_FAILURE.to_owned()))?;
    let entries: Vec<Value> = serde_json::from_slice(&exported)
        .map_err(|_| NodeRunnerError::DriverDetail(READ_LOGIN_FAILURE.to_owned()))?;
    let provider = command::native_provider(&adapter.config.provider);
    let selected: Vec<Value> = entries
        .into_iter()
        .filter(|entry| {
            entry
                .get("integrationID")
                .and_then(Value::as_str)
                .map(|id| id == provider)
                .unwrap_or(false)
        })
        .collect();
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    let mut import_environment = export_environment;
    for (name, value) in private_environment(files.home())? {
        import_environment.insert(name.to_owned(), value);
    }
    let input = serde_json::to_vec(&selected)
        .map_err(|_| NodeRunnerError::DriverDetail(STORE_LOGIN_FAILURE.to_owned()))?;
    run_store_process(StoreProcessRequest {
        adapter,
        files,
        control,
        environment: &import_environment,
        arguments: &["auth", "import", "--standalone"],
        input: Some(&input),
        failure: STORE_LOGIN_FAILURE,
        capture: false,
    })
    .await?;
    let mut secrets = Vec::new();
    for entry in &selected {
        if let Some(value) = entry.get("value").and_then(Value::as_object) {
            for (key, leaf) in value {
                if key != "type" {
                    if let Some(text) = leaf.as_str() {
                        secrets.push(text.to_owned());
                    }
                }
            }
        }
    }
    Ok(secrets)
}

/// Environment for a store process that reads the user's own store: the base and
/// local environment, the user's home, and the scratch directory. No private
/// `XDG_*` values are set, so the process sees the user's store.
fn store_environment(
    adapter: &OpenCodeAdapter,
    files: &ProviderExecutionFiles,
) -> Result<BTreeMap<String, String>, NodeRunnerError> {
    let mut environment = adapter.config.base_environment.clone_values();
    for (name, value) in &adapter.local_environment {
        environment.insert(name.clone(), value.clone());
    }
    let home = adapter
        .config
        .local_user_home
        .as_deref()
        .unwrap_or_else(|| files.home());
    let home = home
        .to_str()
        .ok_or_else(|| NodeRunnerError::DriverDetail(READ_LOGIN_FAILURE.to_owned()))?;
    environment.insert("HOME".to_owned(), home.to_owned());
    #[cfg(windows)]
    environment.insert("USERPROFILE".to_owned(), home.to_owned());
    environment.insert(
        "TMPDIR".to_owned(),
        files
            .scratch_text()
            .map_err(|error| NodeRunnerError::DriverDetail(error.to_string()))?,
    );
    environment.insert("OPENCODE_DISABLE_AUTOUPDATE".to_owned(), "1".to_owned());
    Ok(environment)
}

struct StoreProcessRequest<'a> {
    adapter: &'a OpenCodeAdapter,
    files: &'a ProviderExecutionFiles,
    control: &'a DriverControl,
    environment: &'a BTreeMap<String, String>,
    arguments: &'a [&'a str],
    input: Option<&'a [u8]>,
    failure: &'static str,
    capture: bool,
}

/// Runs one `opencode auth` store process against the user's or the session's store.
/// The process output never appears in any error: it may contain secrets.
async fn run_store_process(
    request: StoreProcessRequest<'_>,
) -> Result<Option<Vec<u8>>, NodeRunnerError> {
    let StoreProcessRequest {
        adapter,
        files,
        control,
        environment,
        arguments,
        input,
        failure,
        capture,
    } = request;
    let mut argv = adapter.config.prefix_arguments.clone();
    for argument in arguments {
        argv.push(argument.to_string());
    }
    let command = ProcessSessionCommand {
        program: adapter.config.executable.clone(),
        argv,
        environment: environment.clone(),
        workspace: WorkspaceCapability {
            current_dir: files.workspace.clone(),
            mode: WorkspaceAccessMode::ReadWrite,
        },
        deadline: Some(tokio::time::Instant::now() + STORE_DEADLINE),
    };
    let mut process = match LocalProcessRunner::new()
        .open(command, control.cancellation())
        .await
    {
        Ok(process) => process,
        Err(_) => return Err(failure_error(control, failure)),
    };
    // An input failure still drains and reaps the process before the node fails.
    let input_failed = match input {
        Some(input) => send_process_input(&mut process, input).await.is_err(),
        None => process.close_stdin().await.is_err(),
    };
    let mut buffer = Vec::new();
    let mut truncated = false;
    let mut stdout = process.detach_stdout();
    while let Some(chunk) = stdout.recv().await {
        if capture && !truncated {
            buffer.extend_from_slice(chunk.as_slice());
            if buffer.len() > MAX_STORE_OUTPUT {
                truncated = true;
                buffer.truncate(MAX_STORE_OUTPUT);
            }
        }
    }
    let completion = process.wait().await;
    let output = require_process_cleanup(&completion)?;
    if output.cancelled {
        return Err(NodeRunnerError::Cancelled);
    }
    if input_failed || truncated || output.exit_code != Some(0) {
        return Err(NodeRunnerError::DriverDetail(failure.to_owned()));
    }
    Ok(if capture { Some(buffer) } else { None })
}

fn failure_error(control: &DriverControl, failure: &'static str) -> NodeRunnerError {
    if control.is_cancelled() {
        NodeRunnerError::Cancelled
    } else {
        NodeRunnerError::DriverDetail(failure.to_owned())
    }
}
