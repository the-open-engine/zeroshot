use std::collections::BTreeMap;
use std::path::Path;
use serde_json::{Value, json};
use crate::execution::WorkspaceAccessMode;
use crate::execution::driver::WorkspaceCapability;
use crate::execution::process::ProcessSessionCommand;
use crate::native_v2_capsule::provider_process::{ProviderExecutionFiles, effort_token};
use crate::native_v2_contract::NodeRuntimeBinding;
use crate::native_v2_runner::{DriverInvocation, NodeRole, NodeRunnerError};
use super::{CopilotConfig, auth};

pub(super) fn command(
    config: &CopilotConfig,
    invocation: &DriverInvocation,
    files: &ProviderExecutionFiles,
) -> Result<ProcessSessionCommand, NodeRunnerError> {
    let environment = process_environment(config, invocation, files)?;
    let mode = match invocation.role {
        NodeRole::Worker => WorkspaceAccessMode::ReadWrite,
        NodeRole::Verifier => WorkspaceAccessMode::ReadOnly,
        NodeRole::GitDelivery => return Err(NodeRunnerError::Driver),
    };
    Ok(ProcessSessionCommand {
        program: path_text(&config.executable)?,
        argv: [
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
        .collect(),
        environment,
        workspace: WorkspaceCapability {
            current_dir: files.workspace.clone(),
            mode,
        },
        deadline: None,
    })
}

fn process_environment(
    config: &CopilotConfig,
    invocation: &DriverInvocation,
    files: &ProviderExecutionFiles,
) -> Result<BTreeMap<String, String>, NodeRunnerError> {
    let home = path_text(files.home())?;
    let mut environment = BTreeMap::from([
        ("PATH".to_owned(), config.search_path.clone()),
        ("HOME".to_owned(), home.clone()),
        ("COPILOT_HOME".to_owned(), home),
        ("COPILOT_AUTO_UPDATE".to_owned(), "false".to_owned()),
        ("COPILOT_DISABLE_KEYTAR".to_owned(), "1".to_owned()),
        (
            "TMPDIR".to_owned(),
            files.scratch_text().map_err(super::rpc::process_error)?,
        ),
    ]);
    for (name, value) in invocation.environment.iter() {
        let name = name.as_str();
        if auth::is_credential(name) {
            continue;
        }
        if environment.contains_key(name)
            || name.starts_with("COPILOT_")
            || matches!(
                name,
                "GH_TOKEN" | "GITHUB_TOKEN" | "NODE_OPTIONS" | "NODE_DEBUG"
            )
        {
            return Err(super::rpc::failure(
                "Copilot environment overrides reserved runtime configuration",
            ));
        }
        environment.insert(name.to_owned(), value.to_owned());
    }
    Ok(environment)
}

pub(super) fn session_parameters(
    invocation: &DriverInvocation,
    files: &ProviderExecutionFiles,
    session_id: &str,
) -> Result<Value, NodeRunnerError> {
    let NodeRuntimeBinding::Agent { model, effort, .. } = &invocation.node.binding else {
        return Err(NodeRunnerError::Driver);
    };
    let mut params = json!({
        "sessionId":session_id, "model":model.as_str(),
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
    auth::configure(&mut params, &invocation.environment)?;
    Ok(params)
}

fn path_text(path: &Path) -> Result<String, NodeRunnerError> {
    path.to_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| super::rpc::failure("Copilot runtime path is invalid"))
}
