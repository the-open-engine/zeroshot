//! Private OpenCode state per node session and the one-time saved-login transfer.
//!
//! Every OpenCode process of a node session runs against private `XDG_DATA_HOME`,
//! `XDG_STATE_HOME` and `XDG_CACHE_HOME` directories inside the session's private home.
//! On local runs the user's active saved login for the lane's integration is copied into
//! that private store, unless the node declares its own credential. Only that integration is
//! exported, and Zeroshot never writes to the user's store. The export loads a small owned
//! plugin that resolves the active connection first, so OpenCode itself renews an expiring
//! OAuth login in the user's profile. An OAuth login with a lifetime moves without its refresh
//! token and without an expiry, so the private node never renews it itself; later turns replace
//! it when the user's profile renewed it. Any other login moves once per session.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use crate::execution::driver::WorkspaceCapability;
use crate::execution::process::{ProcessRunnerError, ProcessSessionCommand};
use crate::execution::WorkspaceAccessMode;
use crate::native_v2_capsule::provider_process::{
    ProviderExecutionFiles, open_provider_process, require_process_cleanup, send_process_input,
};
use crate::native_v2_runner::{DriverControl, DriverInvocation, NodeRunnerError};

use super::command;
use super::OpenCodeAdapter;

const MAX_STORE_OUTPUT: usize = 128 * 1024;
const STORE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

const READ_LOGIN_FAILURE: &str = "OpenCode could not read the saved login from the user's store";
const STORE_LOGIN_FAILURE: &str =
    "OpenCode could not store the saved login in the private session store";
const RENEW_LOGIN_FAILURE: &str = "OpenCode could not renew the saved login in the user's profile";
const RENEWAL_PLUGIN: &str = "zeroshot-login-renewal";
// Resolving the active connection makes OpenCode renew an expiring OAuth login in this profile,
// under its own lock, before `auth export` reads the store. The marker is written only after
// the resolution succeeded.
const RENEWAL: &str = r#"import fs from 'node:fs';
import path from 'node:path';
const integration = RENEWAL_INTEGRATION;
export default {
  id: 'zeroshot-login-renewal',
  async setup(ctx) {
    const connection = await ctx.integration.connection.active(integration);
    if (connection) await ctx.integration.connection.resolve(connection);
    fs.mkdirSync(path.dirname(LOADED_MARKER), {recursive: true}); fs.writeFileSync(LOADED_MARKER, '');
  }
}
"#;

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
    pub(super) files: &'a Arc<ProviderExecutionFiles>,
    pub(super) control: &'a DriverControl,
    /// The login an earlier turn of this session transferred, if any.
    pub(super) previous: Option<&'a TransferredLogin>,
}

/// The login a session holds in its private store.
#[derive(Clone, Debug, Default)]
pub(super) struct TransferredLogin {
    /// Secret strings of every login this session exported, for redaction.
    pub(super) secrets: Vec<String>,
    /// Set for an OAuth login with a lifetime; later turns renew it in the user's profile.
    renewable: Option<RenewableLogin>,
}

#[derive(Clone, Debug)]
struct RenewableLogin {
    id: String,
    access: String,
}

/// Copies the user's active saved login for the lane's integration into the session's private
/// store after OpenCode had the chance to renew it in the user's profile. Returns what the
/// session holds, including every exported secret string for redaction.
pub(super) async fn transfer_login(
    request: TransferRequest<'_>,
) -> Result<TransferredLogin, NodeRunnerError> {
    let TransferRequest {
        adapter,
        invocation,
        files,
        control,
        previous,
    } = request;
    if adapter.runners.is_hosted() {
        return Ok(TransferredLogin::default());
    }
    let declared = invocation.environment.iter().any(|(name, _)| {
        Some(name.as_str()) == command::credential_field(&adapter.config.provider)
    });
    if declared {
        return Ok(TransferredLogin::default());
    }
    // A saved key or a login without a lifetime moves once per session.
    if let Some(previous) = previous {
        if previous.renewable.is_none() {
            return Ok(previous.clone());
        }
    }
    let provider = command::native_provider(&adapter.config.provider);
    let base_environment = store_environment(adapter, files)?;
    let mut export_environment = base_environment.clone();
    install_renewal(&mut export_environment, files, provider)?;
    let exported = run_store_process(StoreProcessRequest {
        adapter,
        files,
        control,
        environment: &export_environment,
        arguments: &["auth", "export", "--standalone", provider],
        input: None,
        failure: READ_LOGIN_FAILURE,
        capture: true,
    })
    .await?
    .ok_or_else(|| NodeRunnerError::DriverDetail(READ_LOGIN_FAILURE.to_owned()))?;
    let entries: Vec<Value> = serde_json::from_slice(&exported)
        .map_err(|_| NodeRunnerError::DriverDetail(READ_LOGIN_FAILURE.to_owned()))?;
    // Of the lane's credentials only the active one moves; an inactive one stays behind.
    let Some(mut entry) = entries.into_iter().find(|entry| {
        entry.get("integrationID").and_then(Value::as_str) == Some(provider)
            && entry.get("active").and_then(Value::as_bool) == Some(true)
    }) else {
        return Ok(previous.cloned().unwrap_or_default());
    };
    // Without a login there is nothing to renew; with one, the renewal must have run.
    if !renewal_marker(files)?.exists() {
        return Err(NodeRunnerError::DriverDetail(
            RENEW_LOGIN_FAILURE.to_owned(),
        ));
    }
    let mut secrets = previous
        .map(|previous| previous.secrets.clone())
        .unwrap_or_default();
    collect_secrets(&entry, &mut secrets);
    let renewable = private_copy(&mut entry);
    let replaced = previous.and_then(|previous| previous.renewable.as_ref());
    if let (Some(replaced), Some(current)) = (replaced, renewable.as_ref()) {
        if replaced.access == current.access {
            return Ok(TransferredLogin { secrets, renewable });
        }
    }
    let mut private = base_environment;
    for (name, value) in private_environment(files.home())? {
        private.insert(name.to_owned(), value);
    }
    if let Some(replaced) = replaced {
        run_store_process(StoreProcessRequest {
            adapter,
            files,
            control,
            environment: &private,
            arguments: &["auth", "logout", "--standalone", provider, &replaced.id],
            input: None,
            failure: STORE_LOGIN_FAILURE,
            capture: false,
        })
        .await?;
    }
    let input = serde_json::to_vec(&[entry])
        .map_err(|_| NodeRunnerError::DriverDetail(STORE_LOGIN_FAILURE.to_owned()))?;
    run_store_process(StoreProcessRequest {
        adapter,
        files,
        control,
        environment: &private,
        arguments: &["auth", "import", "--standalone"],
        input: Some(&input),
        failure: STORE_LOGIN_FAILURE,
        capture: false,
    })
    .await?;
    Ok(TransferredLogin { secrets, renewable })
}

fn collect_secrets(entry: &Value, secrets: &mut Vec<String>) {
    let Some(value) = entry.get("value").and_then(Value::as_object) else {
        return;
    };
    for (key, leaf) in value {
        if key == "type" {
            continue;
        }
        if let Some(text) = leaf.as_str() {
            if !text.is_empty() && !secrets.iter().any(|known| known == text) {
                secrets.push(text.to_owned());
            }
        }
    }
}

/// An OAuth login with a lifetime moves without its refresh token and with `expires: 0`, so the
/// private node never asks the token endpoint; only the user's profile renews it, before a turn.
/// A login without a lifetime moves unchanged.
fn private_copy(entry: &mut Value) -> Option<RenewableLogin> {
    let id = entry.get("id")?.as_str()?.to_owned();
    let value = entry.get_mut("value")?.as_object_mut()?;
    if value.get("type")?.as_str()? != "oauth" {
        return None;
    }
    let expires = value.get("expires").and_then(Value::as_f64).unwrap_or(0.0);
    if expires == 0.0 {
        return None;
    }
    // Without an access token there is nothing to renew: the login moves unchanged.
    let access = value.get("access")?.as_str()?.to_owned();
    value.insert("refresh".to_owned(), Value::String(String::new()));
    value.insert("expires".to_owned(), Value::from(0));
    Some(RenewableLogin { id, access })
}

fn install_renewal(
    environment: &mut BTreeMap<String, String>,
    files: &ProviderExecutionFiles,
    integration: &str,
) -> Result<(), NodeRunnerError> {
    let failure = || NodeRunnerError::DriverDetail(RENEW_LOGIN_FAILURE.to_owned());
    match std::fs::remove_file(renewal_marker(files)?) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(failure()),
    }
    let source = RENEWAL.replace(
        "RENEWAL_INTEGRATION",
        &serde_json::to_string(integration).map_err(|_| failure())?,
    );
    super::plugin::install_into(environment, files.home(), RENEWAL_PLUGIN, &source)
        .map_err(|_| failure())
}

fn renewal_marker(files: &ProviderExecutionFiles) -> Result<PathBuf, NodeRunnerError> {
    let scratch = files
        .scratch_text()
        .map_err(|error| NodeRunnerError::DriverDetail(error.to_string()))?;
    Ok(PathBuf::from(scratch)
        .join("zeroshot-plugins")
        .join(format!("{RENEWAL_PLUGIN}.loaded")))
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
    files: &'a Arc<ProviderExecutionFiles>,
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
    let opened = open_provider_process(Arc::clone(files), command, control).await;
    let mut process = launched(opened, control, failure)?;
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

/// A launch that may have started a process keeps its fatal error. Launch details stay out of the
/// failure because the command environment can carry credentials.
pub(super) fn launched<T>(
    opened: Result<Result<T, ProcessRunnerError>, NodeRunnerError>,
    control: &DriverControl,
    failure: &'static str,
) -> Result<T, NodeRunnerError> {
    match opened? {
        Ok(process) => Ok(process),
        Err(_) => Err(failure_error(control, failure)),
    }
}

fn failure_error(control: &DriverControl, failure: &'static str) -> NodeRunnerError {
    if control.is_cancelled() {
        NodeRunnerError::Cancelled
    } else {
        NodeRunnerError::DriverDetail(failure.to_owned())
    }
}
