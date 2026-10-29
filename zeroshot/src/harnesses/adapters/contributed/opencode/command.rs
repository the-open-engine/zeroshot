use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::execution::{driver::WorkspaceCapability, process::ProcessSessionCommand};
use crate::native_v2_capsule::provider_process::{
    ProviderExecutionFiles, agent_workspace_access, effort_token, extend_declared_environment,
    merge_local_environment,
};
use crate::native_v2_contract::NodeRuntimeBinding;
use crate::native_v2_runner::{DriverInvocation, NodeRunnerError};

use super::OpenCodeAdapter;

pub(super) const LOCAL_ENVIRONMENT: &[&str] = &[
    "USER",
    "ProgramData",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_DIR",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_AUTH_CONTENT",
    "OPENCODE_PERMISSION",
    "OPENCODE_GIT_BASH_PATH",
    "OPENCODE_MODELS_PATH",
    "OPENCODE_DISABLE_AUTOCOMPACT",
    "OPENCODE_DISABLE_MODELS_FETCH",
    "OPENCODE_DISABLE_LSP_DOWNLOAD",
    "OPENCODE_DISABLE_DEFAULT_PLUGINS",
    "OPENCODE_DISABLE_PROJECT_CONFIG",
    "OPENCODE_PURE",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "OPENAI_API_BASE",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_BASE_URL",
    "OPENROUTER_API_KEY",
    "OPENROUTER_BASE_URL",
];

/// Native AWS credential-chain inputs; inherited only by the Bedrock lane.
pub(super) const AWS_ENVIRONMENT: &[&str] = &[
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "AWS_BEDROCK_BASE_URL",
    "AWS_PROFILE",
    "AWS_CONFIG_FILE",
    "AWS_SHARED_CREDENTIALS_FILE",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
    "AWS_ROLE_ARN",
    "AWS_ROLE_SESSION_NAME",
    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
    "AWS_CONTAINER_AUTHORIZATION_TOKEN",
    "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
    "AWS_EC2_METADATA_DISABLED",
];
pub(super) const AWS_SECRETS: &[&str] = &[
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_CONTAINER_AUTHORIZATION_TOKEN",
];

const CREDENTIALS: &[&str] = &[
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENROUTER_API_KEY",
    "GATEWAY_API_KEY",
    "AWS_BEARER_TOKEN_BEDROCK",
    "COPILOT_GITHUB_TOKEN",
];
const AUTHENTICATION: &[&str] = &[
    "COPILOT_GITHUB_TOKEN",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENROUTER_API_KEY",
    "GATEWAY_API_KEY",
    "AWS_BEARER_TOKEN_BEDROCK",
    "OPENCODE_AUTH_CONTENT",
    "AWS_PROFILE",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_ROLE_ARN",
    "AWS_ROLE_SESSION_NAME",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
    "AWS_CONTAINER_AUTHORIZATION_TOKEN",
    "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
    "AWS_CONFIG_FILE",
    "AWS_SHARED_CREDENTIALS_FILE",
];

pub(super) fn native_provider(provider: &str) -> &str {
    match provider {
        "bedrock" => "amazon-bedrock",
        "github" => "github-copilot",
        _ => provider,
    }
}

pub(super) fn credential_field(provider: &str) -> Option<&'static str> {
    match provider {
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "bedrock" => Some("AWS_BEARER_TOKEN_BEDROCK"),
        "gateway" => Some("GATEWAY_API_KEY"),
        "github" => Some("COPILOT_GITHUB_TOKEN"),
        "openai" => Some("OPENAI_API_KEY"),
        "openrouter" => Some("OPENROUTER_API_KEY"),
        _ => None,
    }
}

pub(super) fn endpoint_fields(provider: &str) -> &'static [&'static str] {
    match provider {
        "openai" => &["OPENAI_BASE_URL", "OPENAI_API_BASE"],
        "anthropic" => &["ANTHROPIC_BASE_URL"],
        "openrouter" => &["OPENROUTER_BASE_URL"],
        "bedrock" => &["AWS_BEDROCK_BASE_URL"],
        "gateway" => &["GATEWAY_BASE_URL"],
        _ => &[],
    }
}

pub(super) struct CommandRequest<'a> {
    pub(super) adapter: &'a OpenCodeAdapter,
    pub(super) invocation: &'a DriverInvocation,
    pub(super) files: &'a ProviderExecutionFiles,
    pub(super) session_id: Option<&'a str>,
}

pub(super) fn command(
    request: CommandRequest<'_>,
) -> Result<ProcessSessionCommand, NodeRunnerError> {
    let CommandRequest {
        adapter,
        invocation,
        files,
        session_id,
    } = request;
    let mut environment = process_environment(adapter, invocation, files)?;
    let declared = declared_authority(&adapter.config.provider, &invocation.environment);
    let mut config = inline_configuration(&environment)?;
    configure_provider(adapter, invocation, &mut environment, &mut config)?;
    environment.insert(
        "OPENCODE_CONFIG_CONTENT".to_owned(),
        serde_json::to_string(&config).map_err(|_| NodeRunnerError::Driver)?,
    );
    let mut command = ProcessSessionCommand {
        program: adapter.config.executable.clone(),
        argv: arguments(adapter, invocation, session_id)?,
        environment,
        workspace: WorkspaceCapability {
            current_dir: files.workspace.clone(),
            mode: agent_workspace_access(invocation.role)?,
        },
        deadline: None,
    };
    if declared {
        super::auth::install(super::auth::Installation {
            adapter,
            invocation,
            files,
            command: &mut command,
        })?;
    }
    install_memory(adapter, invocation, files, &mut command);
    Ok(command)
}

pub(super) fn install_memory(
    adapter: &OpenCodeAdapter,
    invocation: &DriverInvocation,
    files: &ProviderExecutionFiles,
    command: &mut ProcessSessionCommand,
) {
    if let Some(memory) = &adapter.active_memory {
        memory.configure_with(
            command,
            crate::active_memory::hooks::HookInstallation {
                invocation,
                home: files.home(),
            },
            super::memory::install,
        );
    }
}

pub(super) fn declared_authority(
    provider: &str,
    environment: &crate::native_v2_runner::ResolvedEnvironment,
) -> bool {
    environment.iter().any(|(name, _)| {
        Some(name.as_str()) == credential_field(provider)
            || endpoint_fields(provider).contains(&name.as_str())
            || (provider == "bedrock" && name.as_str() == "AWS_REGION")
    })
}

fn process_environment(
    adapter: &OpenCodeAdapter,
    invocation: &DriverInvocation,
    files: &ProviderExecutionFiles,
) -> Result<BTreeMap<String, String>, NodeRunnerError> {
    let mut environment = adapter.config.base_environment.clone_values();
    // Declared connections never replace host-owned launch controls.
    for (name, _) in invocation.environment.iter() {
        validate_connection_name(name.as_str(), cfg!(windows))?;
    }
    extend_declared_environment(&mut environment, &invocation.environment)?;
    merge_local_environment(
        &mut environment,
        &invocation.environment,
        &adapter.local_environment,
        AUTHENTICATION,
    );
    let home = adapter
        .config
        .local_user_home
        .as_deref()
        .unwrap_or_else(|| files.home());
    let home = home
        .to_str()
        .filter(|home| !home.is_empty())
        .ok_or_else(|| {
            NodeRunnerError::DriverDetail(
                "OpenCode home is not a valid nonempty UTF-8 path".to_owned(),
            )
        })?;
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
    if adapter.runners.is_hosted() {
        environment.insert("OPENCODE_DISABLE_PROJECT_CONFIG".to_owned(), "1".to_owned());
    }
    Ok(environment)
}

pub(super) fn validate_connection_name(name: &str, windows: bool) -> Result<(), NodeRunnerError> {
    let upper = name.to_ascii_uppercase();
    let reserved = [
        "HOME",
        "USERPROFILE",
        "PATH",
        "TMPDIR",
        "XDG_DATA_HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ];
    let native_control = upper.starts_with("OPENCODE_") && name != "OPENCODE_PERMISSION";
    let reserved = reserved.contains(&name) || (windows && reserved.contains(&upper.as_str()));
    let native_names = LOCAL_ENVIRONMENT
        .iter()
        .chain(AWS_ENVIRONMENT)
        .chain(CREDENTIALS);
    let alias = windows && name != upper && native_names.into_iter().any(|field| *field == upper);
    let host_control = ["ProgramData", super::login::SOURCE_ENV]
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate));
    if native_control || reserved || alias || host_control {
        return Err(NodeRunnerError::DriverDetail(
            "OpenCode connection shadows a reserved or noncanonical launch control".to_owned(),
        ));
    }
    Ok(())
}

fn arguments(
    adapter: &OpenCodeAdapter,
    invocation: &DriverInvocation,
    session_id: Option<&str>,
) -> Result<Vec<String>, NodeRunnerError> {
    let NodeRuntimeBinding::Agent { model, effort, .. } = &invocation.node.binding else {
        return Err(NodeRunnerError::InvalidRole);
    };
    let mut argv = adapter.config.prefix_arguments.clone();
    argv.extend([
        "run".to_owned(),
        "--format".to_owned(),
        "json".to_owned(),
        "--model".to_owned(),
        format!(
            "{}/{}",
            native_provider(&adapter.config.provider),
            model.as_str()
        ),
    ]);
    if let Some(id) = session_id {
        argv.extend(["--session".to_owned(), id.to_owned()]);
    }
    if let Some(effort) = effort {
        argv.extend(["--variant".to_owned(), effort_token(*effort).to_owned()]);
    }
    Ok(argv)
}

fn inline_configuration(environment: &BTreeMap<String, String>) -> Result<Value, NodeRunnerError> {
    let value = environment
        .get("OPENCODE_CONFIG_CONTENT")
        .filter(|source| !source.is_empty())
        .map_or(Ok(json!({})), |source| {
            jsonc_parser::parse_to_serde_value::<Value>(source, &inline_parse_options()).map_err(
                |_| {
                    NodeRunnerError::DriverDetail(
                        "OpenCode inline configuration must be a JSONC object".to_owned(),
                    )
                },
            )
        })?;
    if !value.is_object() {
        return Err(NodeRunnerError::DriverDetail(
            "OpenCode inline configuration must be a JSONC object".to_owned(),
        ));
    }
    Ok(value)
}

fn inline_parse_options() -> jsonc_parser::ParseOptions {
    jsonc_parser::ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    }
}

pub(super) fn configuration_secrets(environment: &BTreeMap<String, String>) -> Vec<String> {
    let mut values = Vec::new();
    for name in [
        "OPENCODE_AUTH_CONTENT",
        "OPENCODE_CONFIG_CONTENT",
        super::login::SOURCE_ENV,
    ] {
        if let Some(value) = environment
            .get(name)
            .and_then(|value| serde_json::from_str::<Value>(value).ok())
        {
            collect_configuration_secrets(&value, &mut values, false);
        }
    }
    values
}

fn collect_configuration_secrets(value: &Value, values: &mut Vec<String>, sensitive: bool) {
    match value {
        Value::Object(object) => {
            for (name, value) in object {
                collect_configuration_secrets(value, values, sensitive || secret_field(name));
            }
        }
        Value::Array(array) => {
            for value in array {
                collect_configuration_secrets(value, values, sensitive);
            }
        }
        Value::String(secret) if sensitive && !secret.is_empty() => values.push(secret.clone()),
        _ => {}
    }
}

fn secret_field(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "key"
            | "apikey"
            | "api_keys"
            | "authtoken"
            | "access"
            | "refresh"
            | "authorization"
            | "token"
            | "password"
            | "headers"
    )
}

pub(super) fn configure_provider(
    adapter: &OpenCodeAdapter,
    invocation: &DriverInvocation,
    environment: &mut BTreeMap<String, String>,
    config: &mut Value,
) -> Result<(), NodeRunnerError> {
    let provider = &adapter.config.provider;
    let field = credential_field(provider).ok_or(NodeRunnerError::Driver)?;
    reject_foreign_credentials(invocation, field)?;
    let declared =
        declared_credential(invocation, environment, field, adapter.runners.is_hosted())?;
    validate_extended_provider(provider, invocation, environment, declared)?;
    let endpoints = endpoint_fields(provider);
    let declared_endpoint = invocation
        .environment
        .iter()
        .any(|(name, _)| endpoints.contains(&name.as_str()));
    if declared_endpoint && !declared {
        return Err(NodeRunnerError::DriverDetail(
            "OpenCode declared endpoint requires its own declared credential".to_owned(),
        ));
    }
    let endpoint = endpoints
        .iter()
        .filter(|field| {
            invocation
                .environment
                .iter()
                .any(|(name, _)| name.as_str() == **field)
        })
        .find_map(|field| environment.get(*field).filter(|value| !value.is_empty()))
        .cloned();
    apply_provider_options(
        config,
        native_provider(provider),
        declared.then_some(field),
        endpoint.as_deref(),
    )?;
    // Only the selected lane's ambient credentials enter this child.
    for foreign in CREDENTIALS.iter().copied() {
        if foreign != field {
            environment.remove(foreign);
        }
    }
    Ok(())
}

fn validate_extended_provider(
    provider: &str,
    invocation: &DriverInvocation,
    environment: &BTreeMap<String, String>,
    declared: bool,
) -> Result<(), NodeRunnerError> {
    if provider == "gateway" {
        crate::native_v2_capsule::gateway::connection(environment)?;
        gateway_protocol(environment)?;
    }
    if provider == "bedrock" {
        validate_bedrock(invocation, environment, declared)?;
    }
    Ok(())
}

fn validate_bedrock(
    invocation: &DriverInvocation,
    environment: &BTreeMap<String, String>,
    declared: bool,
) -> Result<(), NodeRunnerError> {
    let region = invocation
        .environment
        .iter()
        .any(|(name, _)| name.as_str() == "AWS_REGION");
    if (declared && !region)
        || (region && !declared)
        || (region && environment["AWS_REGION"].trim().is_empty())
    {
        return Err(NodeRunnerError::DriverDetail(
            "OpenCode declared Bedrock authentication requires its own bearer token and region"
                .to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn gateway_protocol(
    environment: &BTreeMap<String, String>,
) -> Result<(&'static str, &'static str), NodeRunnerError> {
    match environment.get("GATEWAY_API").map(String::as_str) {
        Some("openai-responses") => Ok(("@ai-sdk/openai", "Authorization")),
        Some("openai-completions") => Ok(("@ai-sdk/openai-compatible", "Authorization")),
        Some("anthropic-messages") => Ok(("@ai-sdk/anthropic", "x-api-key")),
        _ => Err(NodeRunnerError::DriverDetail(
            "OpenCode gateway requires a supported GATEWAY_API".to_owned(),
        )),
    }
}

fn apply_provider_options(
    config: &mut Value,
    provider: &str,
    credential: Option<&str>,
    endpoint: Option<&str>,
) -> Result<(), NodeRunnerError> {
    if credential.is_none() && endpoint.is_none() {
        return Ok(());
    }
    let options = provider_options(config, provider)?;
    if let Some(field) = credential {
        options.insert("apiKey".to_owned(), json!(format!("{{env:{field}}}")));
    }
    if let Some(endpoint) = endpoint {
        options.insert("baseURL".to_owned(), json!(endpoint));
    }
    Ok(())
}

fn provider_options<'a>(
    config: &'a mut Value,
    provider: &str,
) -> Result<&'a mut serde_json::Map<String, Value>, NodeRunnerError> {
    let providers = config
        .as_object_mut()
        .ok_or(NodeRunnerError::Driver)?
        .entry("provider")
        .or_insert_with(|| json!({}));
    let provider_config = providers
        .as_object_mut()
        .ok_or(NodeRunnerError::Driver)?
        .entry(provider.to_owned())
        .or_insert_with(|| json!({}));
    let options = provider_config
        .as_object_mut()
        .ok_or(NodeRunnerError::Driver)?
        .entry("options")
        .or_insert_with(|| json!({}));
    options.as_object_mut().ok_or(NodeRunnerError::Driver)
}

fn reject_foreign_credentials(
    invocation: &DriverInvocation,
    own: &str,
) -> Result<(), NodeRunnerError> {
    let credential_fields = CREDENTIALS;
    let foreign = invocation
        .environment
        .iter()
        .any(|(name, _)| credential_fields.contains(&name.as_str()) && name.as_str() != own);
    if foreign {
        Err(NodeRunnerError::DriverDetail(
            "OpenCode connection contains another provider's credential".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn declared_credential(
    invocation: &DriverInvocation,
    environment: &BTreeMap<String, String>,
    field: &str,
    contained: bool,
) -> Result<bool, NodeRunnerError> {
    let declared = invocation
        .environment
        .iter()
        .any(|(name, _)| name.as_str() == field);
    if contained && !declared {
        return Err(NodeRunnerError::DriverDetail(
            "OpenCode contained lane requires a declared provider credential".to_owned(),
        ));
    }
    if declared
        && environment
            .get(field)
            .is_none_or(|value| value.trim().is_empty())
    {
        Err(NodeRunnerError::DriverDetail(
            "OpenCode declared credential is empty".to_owned(),
        ))
    } else {
        Ok(declared)
    }
}
