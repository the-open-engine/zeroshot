use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use serde_json::{Value, json};
use crate::execution::driver::WorkspaceCapability;
use crate::execution::process::ProcessSessionCommand;
use crate::native_v2_capsule::provider_process::{
    ProviderExecutionFiles, agent_workspace_access, effort_token,
};
use crate::native_v2_contract::NodeRuntimeBinding;
use crate::native_v2_runner::{DriverInvocation, NodeRunnerError};
use super::{CopilotConfig, auth, provider};

const SECRET_ENVIRONMENT: &[&str] = &[
    "COPILOT_PROVIDER_BASE_URL",
    "COPILOT_PROVIDER_API_KEY",
    "COPILOT_PROVIDER_API_KEY_COMMAND",
    "COPILOT_PROVIDER_BEARER_TOKEN",
    "COPILOT_PROVIDER_HEADERS",
    "COPILOT_PROVIDERS_CONFIG",
    "COPILOT_OFFLINE",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_RUNTIME_DIR",
];
const MAX_SECRET_ENVIRONMENT_ARGUMENT_BYTES: usize =
    if cfg!(windows) { 16 * 1024 } else { 64 * 1024 };
const MAX_COMMAND_ENVIRONMENT_BYTES: usize = if cfg!(windows) {
    24 * 1024
} else {
    4 * 1024 * 1024
};

pub(super) fn command(
    config: &CopilotConfig,
    invocation: &DriverInvocation,
    files: &ProviderExecutionFiles,
    local_provider: Option<&provider::LocalProvider>,
) -> Result<ProcessSessionCommand, NodeRunnerError> {
    validate_command_environment(config, invocation, local_provider)?;
    let environment = process_environment(config, invocation, files, local_provider)?;
    let mode = agent_workspace_access(invocation.role)?;
    let model = agent_model(invocation)?;
    let mut argv = [
        "--headless",
        "--stdio",
        "--no-auto-update",
        "--no-auto-login",
        "--disable-builtin-mcps",
        "--no-ask-user",
        "--no-remote-export",
        "--log-level",
        "error",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    argv.push(secret_environment_argument(
        config,
        invocation,
        local_provider,
    )?);
    argv.extend(["--model".to_owned(), model.to_owned()]);
    Ok(ProcessSessionCommand {
        program: path_text(&config.executable)?,
        argv,
        environment,
        workspace: WorkspaceCapability {
            current_dir: files.workspace.clone(),
            mode,
        },
        deadline: None,
    })
}

pub(super) fn agent_model(invocation: &DriverInvocation) -> Result<&str, NodeRunnerError> {
    let NodeRuntimeBinding::Agent { model, .. } = &invocation.node.binding else {
        return Err(NodeRunnerError::Driver);
    };
    Ok(model.as_str())
}

fn process_environment(
    config: &CopilotConfig,
    invocation: &DriverInvocation,
    files: &ProviderExecutionFiles,
    local_provider: Option<&provider::LocalProvider>,
) -> Result<BTreeMap<String, String>, NodeRunnerError> {
    let (home, copilot_home) = provider_homes(config, files)?;
    let mut environment = config.base_environment.clone();
    if local_provider.is_some_and(provider::LocalProvider::uses_api_key_command) {
        extend_command_environment(&mut environment, &config.local_command_environment);
    }
    extend_declared_environment(&mut environment, invocation)?;
    if let Some(provider) = local_provider {
        environment.extend(provider.process_environment().clone());
    }
    insert_reserved_environment(
        &mut environment,
        [
            ("PATH".to_owned(), config.search_path.clone()),
            ("HOME".to_owned(), home),
            ("COPILOT_HOME".to_owned(), copilot_home),
            ("COPILOT_AUTO_UPDATE".to_owned(), "false".to_owned()),
            (
                "TMPDIR".to_owned(),
                files.scratch_text().map_err(super::rpc::process_error)?,
            ),
        ],
    )?;
    if config.local_user.is_none() {
        environment.insert("COPILOT_DISABLE_KEYTAR".to_owned(), "1".to_owned());
    }
    Ok(environment)
}

fn extend_command_environment(
    environment: &mut BTreeMap<String, String>,
    invoking: &BTreeMap<String, String>,
) {
    for (name, value) in invoking {
        if private_command_environment_name(name) {
            environment.insert(name.clone(), value.clone());
        }
    }
}

fn validate_command_environment(
    config: &CopilotConfig,
    invocation: &DriverInvocation,
    local_provider: Option<&provider::LocalProvider>,
) -> Result<(), NodeRunnerError> {
    if !local_provider.is_some_and(provider::LocalProvider::uses_api_key_command) {
        return Ok(());
    }
    let mut bytes = 0_usize;
    for (name, value) in config
        .local_command_environment
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .chain(
            invocation
                .environment
                .iter()
                .map(|(name, value)| (name.as_str(), value)),
        )
    {
        if !private_command_environment_name(name) {
            continue;
        }
        if !representable_secret_name(name) {
            return Err(super::rpc::failure(
                "Copilot credential-command environment has an unsupported field name",
            ));
        }
        bytes = bytes
            .checked_add(name.len())
            .and_then(|bytes| bytes.checked_add(value.len()))
            .ok_or_else(|| {
                super::rpc::failure("Copilot credential-command environment is too large")
            })?;
        if bytes > MAX_COMMAND_ENVIRONMENT_BYTES {
            return Err(super::rpc::failure(
                "Copilot credential-command environment is too large",
            ));
        }
    }
    Ok(())
}

fn secret_environment_argument(
    config: &CopilotConfig,
    invocation: &DriverInvocation,
    local_provider: Option<&provider::LocalProvider>,
) -> Result<String, NodeRunnerError> {
    let mut names = SECRET_ENVIRONMENT
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    if local_provider.is_some_and(provider::LocalProvider::uses_api_key_command) {
        names.extend(
            config
                .local_command_environment
                .keys()
                .filter(|name| private_command_environment_name(name))
                .map(|name| secret_environment_name(name)),
        );
        names.extend(
            invocation
                .environment
                .iter()
                .map(|(name, _)| name.as_str())
                .filter(|name| private_command_environment_name(name))
                .map(secret_environment_name),
        );
    }
    let argument = format!(
        "--secret-env-vars={}",
        names.into_iter().collect::<Vec<_>>().join(",")
    );
    if argument.len() > MAX_SECRET_ENVIRONMENT_ARGUMENT_BYTES {
        return Err(super::rpc::failure(
            "Copilot credential-command environment is too large",
        ));
    }
    Ok(argument)
}

fn private_command_environment_name(name: &str) -> bool {
    private_command_environment_name_for_platform(name, cfg!(windows))
}

fn private_command_environment_name_for_platform(name: &str, windows: bool) -> bool {
    !name.is_empty()
        && !auth::is_credential_for_platform(name, windows)
        && !provider::is_configuration_for_platform(name, windows)
        && !reserved_process_environment_for_platform(name, windows)
}

fn reserved_process_environment(name: &str) -> bool {
    reserved_process_environment_for_platform(name, cfg!(windows))
}

fn reserved_process_environment_for_platform(name: &str, windows: bool) -> bool {
    [
        "PATH",
        "HOME",
        "USERPROFILE",
        "TMPDIR",
        "TEMP",
        "TMP",
        "COPILOT_HOME",
        "COPILOT_AUTO_UPDATE",
        "COPILOT_DISABLE_KEYTAR",
        "NODE_OPTIONS",
        "NODE_DEBUG",
        "NODE_PATH",
        "BUN_OPTIONS",
        "LD_PRELOAD",
        "LD_AUDIT",
        "LD_LIBRARY_PATH",
        "DYLD_INSERT_LIBRARIES",
        "DYLD_LIBRARY_PATH",
        "OPENSSL_CONF",
        "OPENSSL_MODULES",
        "OPENSSL_ENGINES",
    ]
    .iter()
    .any(|reserved| environment_names_match_for_platform(name, reserved, windows))
        || (!windows && (name.starts_with("LD_") || name.starts_with("DYLD_")))
}

fn representable_secret_name(name: &str) -> bool {
    !name
        .chars()
        .any(|character| matches!(character, ',' | '\r' | '\n'))
}

fn secret_environment_name(name: &str) -> String {
    secret_environment_name_for_platform(name, cfg!(windows))
}

fn secret_environment_name_for_platform(name: &str, windows: bool) -> String {
    if windows {
        name.to_ascii_uppercase()
    } else {
        name.to_owned()
    }
}

fn environment_names_match_for_platform(left: &str, right: &str, windows: bool) -> bool {
    if windows {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

fn provider_homes(
    config: &CopilotConfig,
    files: &ProviderExecutionFiles,
) -> Result<(String, String), NodeRunnerError> {
    let runtime_home = path_text(files.home())?;
    config.local_user.as_ref().map_or_else(
        || Ok((runtime_home.clone(), runtime_home)),
        |local| Ok((path_text(&local.home)?, path_text(&local.copilot_home)?)),
    )
}

fn insert_reserved_environment<const N: usize>(
    environment: &mut BTreeMap<String, String>,
    values: [(String, String); N],
) -> Result<(), NodeRunnerError> {
    for (name, value) in values {
        if environment.insert(name, value).is_some() {
            return Err(super::rpc::failure(
                "Copilot native environment overrides reserved runtime configuration",
            ));
        }
    }
    Ok(())
}

fn extend_declared_environment(
    environment: &mut BTreeMap<String, String>,
    invocation: &DriverInvocation,
) -> Result<(), NodeRunnerError> {
    for (name, value) in invocation.environment.iter() {
        let name = name.as_str();
        if auth::is_credential(name) || provider::is_configuration(name) {
            continue;
        }
        if reserved_process_environment(name) {
            return Err(super::rpc::failure(
                "Copilot environment overrides reserved runtime configuration",
            ));
        }
        environment.insert(name.to_owned(), value.to_owned());
    }
    Ok(())
}

pub(super) struct SessionProvider<'a> {
    pub authentication: &'a auth::CopilotAuthentication<'a>,
    pub local: Option<&'a provider::LocalProvider>,
}

pub(super) fn session_parameters(
    invocation: &DriverInvocation,
    files: &ProviderExecutionFiles,
    session_id: &str,
    provider: SessionProvider<'_>,
) -> Result<Value, NodeRunnerError> {
    let NodeRuntimeBinding::Agent { model, effort, .. } = &invocation.node.binding else {
        return Err(NodeRunnerError::Driver);
    };
    let session_model = provider
        .local
        .map(|provider| provider.session_model(model.as_str()))
        .unwrap_or_else(|| model.as_str());
    let mut params = json!({
        "sessionId":session_id, "model":session_model,
        "workingDirectory":path_text(&files.workspace)?,
        "requestPermission":true, "requestUserInput":false,
        "mcpServers":{}, "customAgents":[], "excludedTools":["ask_user"],
        "enableConfigDiscovery":false, "enableFileHooks":false,
        "enableHostGitOperations":false, "enableSkills":false,
        "enableSessionTelemetry":false, "streaming":false,
        "envValueMode":"direct", "clientName":"zeroshot",
    });
    if let Some(effort) = effort {
        params["reasoningEffort"] = json!(effort_token(*effort));
    }
    if let Some(local) = provider.local {
        local.configure(&mut params);
        if !local.bypasses_github_auth() {
            provider.authentication.configure(&mut params)?;
        }
    } else {
        provider.authentication.configure(&mut params)?;
    }
    Ok(params)
}

fn path_text(path: &Path) -> Result<String, NodeRunnerError> {
    path.to_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| super::rpc::failure("Copilot runtime path is invalid"))
}

#[cfg(test)]
#[path = "command/tests.rs"]
mod tests;
