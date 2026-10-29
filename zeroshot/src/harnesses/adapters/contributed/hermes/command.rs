use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use crate::execution::{driver::WorkspaceCapability, process::ProcessSessionCommand};
use crate::native_v2_capsule::provider_process::{
    ProviderExecutionFiles, agent_workspace_access, effort_token, extend_declared_environment,
    merge_local_environment, write_provider_file,
};
use crate::native_v2_contract::NodeRuntimeBinding;
use crate::native_v2_runner::{DriverInvocation, NodeRunnerError};

use super::HermesAdapter;

pub(super) const NATIVE_PATHS: &[&str] = &[
    "HERMES_HOME",
    "HERMES_MANAGED_DIR",
    "HERMES_REAL_HOME",
    "HERMES_BUNDLED_PLUGINS",
    "HERMES_BUNDLED_SKILLS",
    "HERMES_OPTIONAL_SKILLS",
    "HERMES_OPTIONAL_MCPS",
    "HERMES_PREFILL_MESSAGES_FILE",
    "AWS_CA_BUNDLE",
    "AWS_CONFIG_FILE",
    "AWS_SHARED_CREDENTIALS_FILE",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
    "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
];

pub(super) const LOCAL_ENVIRONMENT: &[&str] = &[
    "USER",
    "LOGNAME",
    "SHELL",
    "USERPROFILE",
    "PYTHONPATH",
    "AWS_CONFIG_FILE",
    "AWS_SHARED_CREDENTIALS_FILE",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
    "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "HERMES_HOME",
    "HERMES_MANAGED_DIR",
    "HERMES_MANAGED",
    "HERMES_REAL_HOME",
    "HERMES_BUNDLED_PLUGINS",
    "HERMES_BUNDLED_SKILLS",
    "HERMES_OPTIONAL_SKILLS",
    "HERMES_OPTIONAL_MCPS",
    "HERMES_ENABLE_PROJECT_PLUGINS",
    "HERMES_EPHEMERAL_SYSTEM_PROMPT",
    "HERMES_PREFILL_MESSAGES_FILE",
    "HERMES_YOLO_MODE",
    "HERMES_ACCEPT_HOOKS",
    "HERMES_EXEC_ASK",
    "HERMES_REDACT_SECRETS",
    "HERMES_SAFE_MODE",
    "HERMES_IGNORE_USER_CONFIG",
    "HERMES_IGNORE_RULES",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "OPENAI_API_BASE",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "OPENROUTER_API_KEY",
    "OPENROUTER_BASE_URL",
    "AWS_CA_BUNDLE",
    "AWS_ENDPOINT_URL",
    "AWS_ENDPOINT_URL_BEDROCK_RUNTIME",
    "AWS_ENDPOINT_URL_BEDROCK",
    "AWS_ENDPOINT_URL_STS",
    "AWS_MAX_ATTEMPTS",
    "AWS_RETRY_MODE",
    "AWS_USE_FIPS_ENDPOINT",
    "AWS_USE_DUALSTACK_ENDPOINT",
    "AWS_IGNORE_CONFIGURED_ENDPOINT_URLS",
    "AWS_EC2_METADATA_SERVICE_ENDPOINT",
    "AWS_EC2_METADATA_SERVICE_ENDPOINT_MODE",
    "AWS_EC2_METADATA_V1_DISABLED",
    "AWS_METADATA_SERVICE_TIMEOUT",
    "AWS_METADATA_SERVICE_NUM_ATTEMPTS",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "AWS_PROFILE",
    "AWS_DEFAULT_PROFILE",
    "AWS_ROLE_ARN",
    "AWS_ROLE_SESSION_NAME",
    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
    "AWS_EC2_METADATA_DISABLED",
    "OP_SERVICE_ACCOUNT_TOKEN",
    "BWS_ACCESS_TOKEN",
];
const CREDENTIALS: &[&str] = &[
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "OPENROUTER_API_KEY",
    "GATEWAY_API_KEY",
    "AWS_BEARER_TOKEN_BEDROCK",
    "COPILOT_GITHUB_TOKEN",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_CONTAINER_AUTHORIZATION_TOKEN",
];

fn credential_fields(provider: &str) -> &'static [&'static str] {
    match provider {
        "openai" => &["OPENAI_API_KEY"],
        "anthropic" => &[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ],
        "openrouter" => &["OPENROUTER_API_KEY"],
        "gateway" => &["GATEWAY_API_KEY"],
        "github" => &["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"],
        "bedrock" => &[
            "AWS_BEARER_TOKEN_BEDROCK",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "AWS_CONTAINER_AUTHORIZATION_TOKEN",
        ],
        _ => &[],
    }
}

pub(super) fn credential_field(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some("OPENAI_API_KEY"),
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "openrouter" => Some("OPENROUTER_API_KEY"),
        "gateway" => Some("GATEWAY_API_KEY"),
        "github" => Some("COPILOT_GITHUB_TOKEN"),
        "bedrock" => Some("AWS_BEARER_TOKEN_BEDROCK"),
        _ => None,
    }
}

fn endpoint_fields(provider: &str) -> &'static [&'static str] {
    match provider {
        "openai" => &["OPENAI_BASE_URL", "OPENAI_API_BASE"],
        "anthropic" => &["ANTHROPIC_BASE_URL"],
        "openrouter" => &["OPENROUTER_BASE_URL"],
        "gateway" => &["GATEWAY_BASE_URL"],
        _ => &[],
    }
}

pub(super) fn prepare(
    adapter: &HermesAdapter,
    invocation: &DriverInvocation,
    files: &ProviderExecutionFiles,
    session_id: &str,
) -> Result<(ProcessSessionCommand, Value), NodeRunnerError> {
    let NodeRuntimeBinding::Agent { model, effort, .. } = &invocation.node.binding else {
        return Err(NodeRunnerError::InvalidRole);
    };
    credential_field(&adapter.config.provider)
        .ok_or_else(|| failure("Hermes provider is unsupported"))?;
    let mut environment = environment(adapter, invocation, files)?;
    let authority = authority(&adapter.config.provider, invocation)?;
    let declared_auth = !authority["apiKey"].is_null()
        || authority["aws"]
            .as_object()
            .is_some_and(|aws| aws.keys().any(|name| aws_auth_field(name)));
    let native_environment = native_context(adapter, declared_auth);
    for name in CREDENTIALS {
        environment.remove(*name);
    }
    bootstrap_environment(&mut environment, files.home())?;
    let memory = memory_capability(adapter, invocation, files);
    let init = json!({
        "type":"init", "provider":adapter.config.provider, "model":model.as_str(),
        "effort":effort.map(effort_token), "sessionId":session_id,
        "dbPath":files.home().join("hermes.sqlite"), "contained":adapter.runners.is_hosted(),
        "authority":authority, "memoryCapability":memory, "nativeEnvironment":native_environment,
        "declaredEnvironment":declared_context(invocation, false),
        "providerEnvironment":declared_context(invocation, true),
    });
    let mut argv = adapter.config.prefix_arguments.clone();
    // If an installed launcher ignores our import hook, these arguments print help and exit;
    // native Hermes cannot interpret the private stdin payload as a prompt or attach a gateway.
    argv.extend(["--version".to_owned(), "--help".to_owned()]);
    Ok((
        ProcessSessionCommand {
            program: adapter.config.executable.clone(),
            argv,
            environment,
            workspace: WorkspaceCapability {
                current_dir: files.workspace.clone(),
                mode: agent_workspace_access(invocation.role)?,
            },
            deadline: None,
        },
        init,
    ))
}

fn bootstrap_environment(
    environment: &mut BTreeMap<String, String>,
    home: &Path,
) -> Result<(), NodeRunnerError> {
    install_runtime(home)?;
    let mut python_path = vec![home.to_path_buf()];
    if let Some(original) = environment.get("PYTHONPATH") {
        python_path.extend(std::env::split_paths(original));
    }
    environment.insert(
        "PYTHONPATH".to_owned(),
        std::env::join_paths(python_path)
            .map_err(|_| failure("Hermes PYTHONPATH is invalid"))?
            .to_string_lossy()
            .into_owned(),
    );
    environment.insert(
        "ZEROSHOT_HERMES_BRIDGE".to_owned(),
        home.join("native_runtime.py")
            .to_string_lossy()
            .into_owned(),
    );
    Ok(())
}

fn memory_capability(
    adapter: &HermesAdapter,
    invocation: &DriverInvocation,
    files: &ProviderExecutionFiles,
) -> Option<std::path::PathBuf> {
    adapter.active_memory.as_ref().and_then(|memory| {
        match memory.capability_file(invocation, &files.workspace, files.home()) {
            Ok(path) => path,
            Err(_) => {
                memory.execution_failed();
                None
            }
        }
    })
}

fn authority(provider: &str, invocation: &DriverInvocation) -> Result<Value, NodeRunnerError> {
    let fields = credential_fields(provider);
    let key_fields = if provider == "bedrock" {
        &["AWS_BEARER_TOKEN_BEDROCK"][..]
    } else {
        fields
    };
    let key = key_fields.iter().find_map(|name| {
        invocation
            .environment
            .iter()
            .find(|(field, _)| field.as_str() == *name)
            .map(|(_, value)| value)
    });
    let mut base: Option<&str> = None;
    for (name, value) in invocation.environment.iter() {
        validate_credential(name.as_str(), value, fields)?;
        if endpoint_fields(provider).contains(&name.as_str()) {
            if value.trim().is_empty() || base.is_some_and(|base| base != value) {
                return Err(failure("Hermes declared endpoints are empty or disagree"));
            }
            base = Some(value);
        }
    }
    let api_mode = if provider == "gateway" {
        Some(gateway_mode(invocation)?)
    } else {
        None
    };
    let aws = aws_authority(provider, invocation)?;
    Ok(json!({"apiKey":key,"baseUrl":base,"apiMode":api_mode,"aws":aws}))
}

fn validate_credential(name: &str, value: &str, fields: &[&str]) -> Result<(), NodeRunnerError> {
    if !CREDENTIALS.contains(&name) {
        return Ok(());
    }
    if !fields.contains(&name) {
        return Err(failure(
            "Hermes credential conflicts with the selected provider",
        ));
    }
    if value.trim().is_empty() {
        return Err(failure("Hermes declared credential is empty"));
    }
    Ok(())
}

fn aws_auth_field(name: &str) -> bool {
    matches!(
        name,
        "AWS_BEARER_TOKEN_BEDROCK"
            | "AWS_ACCESS_KEY_ID"
            | "AWS_SECRET_ACCESS_KEY"
            | "AWS_SESSION_TOKEN"
            | "AWS_PROFILE"
            | "AWS_DEFAULT_PROFILE"
            | "AWS_WEB_IDENTITY_TOKEN_FILE"
            | "AWS_ROLE_ARN"
            | "AWS_ROLE_SESSION_NAME"
            | "AWS_CONTAINER_AUTHORIZATION_TOKEN"
            | "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE"
            | "AWS_CONTAINER_CREDENTIALS_FULL_URI"
            | "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI"
    )
}

fn required_aws_value(name: &str) -> bool {
    aws_auth_field(name) || matches!(name, "AWS_REGION" | "AWS_DEFAULT_REGION")
}

fn aws_authority(
    provider: &str,
    invocation: &DriverInvocation,
) -> Result<BTreeMap<String, String>, NodeRunnerError> {
    if provider != "bedrock" {
        return Ok(BTreeMap::new());
    }
    let aws = invocation
        .environment
        .iter()
        .filter(|(name, _)| name.as_str().starts_with("AWS_"))
        .map(|(name, value)| (name.as_str().to_owned(), value.to_owned()))
        .collect::<BTreeMap<_, _>>();
    let bearer = aws.contains_key("AWS_BEARER_TOKEN_BEDROCK");
    if aws
        .iter()
        .any(|(name, value)| required_aws_value(name) && value.trim().is_empty())
    {
        return Err(failure("Hermes declared AWS field is empty"));
    }
    if bearer && !aws.contains_key("AWS_REGION") {
        return Err(failure(
            "Hermes declared Bedrock bearer requires AWS_REGION",
        ));
    }
    if bearer
        && aws
            .keys()
            .any(|name| aws_auth_field(name) && name != "AWS_BEARER_TOKEN_BEDROCK")
    {
        return Err(failure(
            "Hermes Bedrock bearer and IAM credentials conflict",
        ));
    }
    Ok(aws)
}

fn gateway_mode(invocation: &DriverInvocation) -> Result<&'static str, NodeRunnerError> {
    let values = invocation
        .environment
        .iter()
        .map(|(name, value)| (name.as_str().to_owned(), value.to_owned()))
        .collect::<BTreeMap<_, _>>();
    crate::native_v2_capsule::gateway::connection(&values)?;
    match values.get("GATEWAY_API").map(String::as_str) {
        Some("openai-responses") => Ok("codex_responses"),
        Some("openai-completions") => Ok("chat_completions"),
        Some("anthropic-messages") => Ok("anthropic_messages"),
        _ => Err(failure("Hermes gateway requires a supported GATEWAY_API")),
    }
}

fn native_context(adapter: &HermesAdapter, declared: bool) -> BTreeMap<String, String> {
    let selected_credentials = credential_fields(&adapter.config.provider);
    adapter
        .config
        .native_environment
        .clone()
        .into_values()
        .into_iter()
        .filter(|(name, _)| !name.to_ascii_uppercase().starts_with("ZEROSHOT_"))
        .filter(|(name, _)| {
            !(declared
                && (selected_credentials
                    .iter()
                    .any(|field| name.eq_ignore_ascii_case(field))
                    || (adapter.config.provider == "bedrock" && aws_auth_field(name))))
        })
        .collect()
}

fn declared_context(invocation: &DriverInvocation, provider: bool) -> BTreeMap<String, String> {
    invocation
        .environment
        .iter()
        .filter(|(name, _)| CREDENTIALS.contains(&name.as_str()) == provider)
        .map(|(name, value)| (name.as_str().to_owned(), value.to_owned()))
        .collect()
}

fn environment(
    adapter: &HermesAdapter,
    invocation: &DriverInvocation,
    files: &ProviderExecutionFiles,
) -> Result<BTreeMap<String, String>, NodeRunnerError> {
    let mut environment = adapter.config.base_environment.clone_values();
    for (name, _) in invocation.environment.iter() {
        validate_connection_name(name.as_str())?;
    }
    extend_declared_environment(&mut environment, &invocation.environment)?;
    merge_local_environment(
        &mut environment,
        &invocation.environment,
        &adapter.local_environment,
        CREDENTIALS,
    );
    let home = adapter
        .config
        .local_user_home
        .as_deref()
        .unwrap_or_else(|| files.home());
    let home = home
        .to_str()
        .ok_or_else(|| failure("Hermes home is not UTF-8"))?;
    environment.insert("HOME".to_owned(), home.to_owned());
    #[cfg(windows)]
    environment.insert("USERPROFILE".to_owned(), home.to_owned());
    environment.insert(
        "TMPDIR".to_owned(),
        files
            .scratch_text()
            .map_err(super::connection::process_error)?,
    );
    Ok(environment)
}

pub(super) fn validate_connection_name(name: &str) -> Result<(), NodeRunnerError> {
    let reserved = [
        "HOME",
        "USERPROFILE",
        "PATH",
        "PYTHONPATH",
        "PYTHONHOME",
        "TMPDIR",
    ];
    let upper = name.to_ascii_uppercase();
    let permission = [
        "HERMES_YOLO_MODE",
        "HERMES_ACCEPT_HOOKS",
        "HERMES_EXEC_ASK",
        "HERMES_REDACT_SECRETS",
    ];
    if reserved.contains(&upper.as_str())
        || upper.starts_with("XDG_")
        || (upper.starts_with("HERMES_") && !permission.contains(&name))
        || upper.starts_with("ZEROSHOT_")
        || (cfg!(windows)
            && name != upper
            && (LOCAL_ENVIRONMENT.contains(&upper.as_str())
                || CREDENTIALS.contains(&upper.as_str())))
    {
        return Err(failure(
            "Hermes connection shadows a reserved or noncanonical launch control",
        ));
    }
    Ok(())
}

fn install_runtime(home: &Path) -> Result<(), NodeRunnerError> {
    for (name, source) in [
        ("sitecustomize.py", include_str!("sitecustomize.py")),
        ("native_runtime.py", include_str!("native_runtime.py")),
        ("native_auth.py", include_str!("native_auth.py")),
        ("native_providers.py", include_str!("native_providers.py")),
        ("native_memory.py", include_str!("native_memory.py")),
        (
            "native_environment.py",
            include_str!("native_environment.py"),
        ),
    ] {
        write_provider_file(home, name, source.as_bytes())
            .map_err(|_| failure("Hermes private runtime could not be installed"))?;
    }
    Ok(())
}

fn failure(message: &str) -> NodeRunnerError {
    NodeRunnerError::DriverDetail(message.to_owned())
}
