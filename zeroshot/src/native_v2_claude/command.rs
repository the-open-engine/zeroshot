use std::collections::BTreeMap;

use crate::native_v2_contract::ClaudeProvider;
use crate::native_v2_capsule::gateway;
use crate::native_v2_capsule::provider_process::{effort_token, with_driver_detail};
use crate::native_v2_runner::{
    render_agent_prompt, DriverInvocation, NodeRole, NodeRunnerError, ResolvedEnvironment,
};
use crate::worker_catalog::ReasoningEffort;

pub(super) const AWS_BEARER_TOKEN_BEDROCK: &str = "AWS_BEARER_TOKEN_BEDROCK";
pub(super) const AWS_REGION: &str = "AWS_REGION";
pub(super) const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api";
pub(super) const OPENROUTER_KEY: &str = "OPENROUTER_API_KEY";
const ANTHROPIC_TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";
pub(super) const ANTHROPIC_KEY: &str = "ANTHROPIC_API_KEY";
const ANTHROPIC_BASE_URL: &str = "ANTHROPIC_BASE_URL";
const CLAUDE_CODE_OAUTH_REFRESH_TOKEN: &str = "CLAUDE_CODE_OAUTH_REFRESH_TOKEN";
const CLAUDE_CODE_OAUTH_TOKEN: &str = "CLAUDE_CODE_OAUTH_TOKEN";

pub(super) struct ClaudeTurnArguments<'a> {
    pub(super) model: &'a str,
    pub(super) effort: Option<ReasoningEffort>,
    pub(super) role: NodeRole,
    pub(super) resume_id: Option<&'a str>,
    pub(super) json_schema: String,
}

pub(super) fn claude_arguments(
    mut argv: Vec<String>,
    turn: ClaudeTurnArguments<'_>,
) -> Result<Vec<String>, NodeRunnerError> {
    argv.extend([
        "--print".to_owned(),
        "--input-format".to_owned(),
        "text".to_owned(),
        "--output-format".to_owned(),
        "stream-json".to_owned(),
        "--verbose".to_owned(),
        "--include-partial-messages".to_owned(),
        "--model".to_owned(),
        turn.model.to_owned(),
        "--json-schema".to_owned(),
        turn.json_schema,
    ]);
    if let Some(effort) = turn.effort {
        argv.extend(["--effort".to_owned(), effort_token(effort).to_owned()]);
    }
    match turn.role {
        NodeRole::Worker | NodeRole::Verifier => {}
        NodeRole::GitDelivery => return Err(NodeRunnerError::Driver),
    }
    if let Some(resume_id) = turn.resume_id {
        argv.extend(["--resume".to_owned(), resume_id.to_owned()]);
    }
    Ok(argv)
}

pub(super) fn extend_declared_environment(
    environment: &mut BTreeMap<String, String>,
    resolved: &ResolvedEnvironment,
) -> Result<(), NodeRunnerError> {
    for (name, value) in resolved.iter() {
        if value.contains('\0') || environment.contains_key(name.as_str()) {
            return Err(NodeRunnerError::Driver);
        }
        environment.insert(name.as_str().to_owned(), value.to_owned());
    }
    Ok(())
}

pub(super) fn configure_bedrock(
    environment: &mut BTreeMap<String, String>,
) -> Result<(), NodeRunnerError> {
    if [AWS_BEARER_TOKEN_BEDROCK, AWS_REGION].iter().any(|name| {
        !environment
            .get(*name)
            .is_some_and(|value| !value.is_empty())
    }) || [
        ANTHROPIC_TOKEN,
        ANTHROPIC_KEY,
        CLAUDE_CODE_OAUTH_REFRESH_TOKEN,
        CLAUDE_CODE_OAUTH_TOKEN,
        OPENROUTER_KEY,
    ]
    .iter()
    .any(|name| environment.contains_key(*name))
    {
        return Err(NodeRunnerError::Driver);
    }
    environment.insert("CLAUDE_CODE_USE_BEDROCK".to_owned(), "1".to_owned());
    Ok(())
}

pub(super) fn configure_gateway(
    environment: &mut BTreeMap<String, String>,
) -> Result<(), NodeRunnerError> {
    if [
        ANTHROPIC_TOKEN,
        ANTHROPIC_KEY,
        CLAUDE_CODE_OAUTH_REFRESH_TOKEN,
        CLAUDE_CODE_OAUTH_TOKEN,
        OPENROUTER_KEY,
        AWS_BEARER_TOKEN_BEDROCK,
    ]
    .iter()
    .any(|name| environment.contains_key(*name))
    {
        return Err(NodeRunnerError::Driver);
    }
    let (base_url, api_key) = gateway::connection(environment)?;
    let (base_url, api_key) = (base_url.to_owned(), api_key.to_owned());
    environment.insert(ANTHROPIC_BASE_URL.to_owned(), base_url);
    environment.insert(ANTHROPIC_KEY.to_owned(), api_key);
    Ok(())
}

pub(super) fn configure_openrouter(
    environment: &mut BTreeMap<String, String>,
) -> Result<(), NodeRunnerError> {
    let token = environment
        .get(OPENROUTER_KEY)
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or(NodeRunnerError::Driver)?;
    if [ANTHROPIC_TOKEN, ANTHROPIC_KEY]
        .iter()
        .any(|name| environment.contains_key(*name))
    {
        return Err(NodeRunnerError::Driver);
    }
    environment.insert(ANTHROPIC_TOKEN.to_owned(), token);
    environment.insert(ANTHROPIC_KEY.to_owned(), String::new());
    environment
        .entry(ANTHROPIC_BASE_URL.to_owned())
        .or_insert_with(|| OPENROUTER_BASE_URL.to_owned());
    Ok(())
}

pub(super) fn prompt(invocation: &DriverInvocation) -> Result<String, NodeRunnerError> {
    render_agent_prompt(
        invocation.agent_instructions()?,
        &invocation.node.input,
        &invocation.response,
    )
    .map_err(|error| with_driver_detail(error, "Claude prompt could not be serialized"))
}

pub(super) fn configure_provider(
    environment: &mut BTreeMap<String, String>,
    provider: ClaudeProvider,
) -> Result<(), NodeRunnerError> {
    validate_provider_selection(environment, provider)?;
    match provider {
        ClaudeProvider::Anthropic => {}
        ClaudeProvider::Gateway => {
            configure_gateway(environment).map_err(|error| {
                with_driver_detail(
                    error,
                    "Claude gateway credentials are missing or conflict with other credentials",
                )
            })?;
        }
        ClaudeProvider::OpenRouter => {
            configure_openrouter(environment).map_err(|error| {
                    with_driver_detail(
                        error,
                        "Claude OpenRouter credentials are missing or conflict with Anthropic credentials",
                    )
                })?;
        }
        ClaudeProvider::Bedrock => {
            configure_bedrock(environment).map_err(|error| {
                    with_driver_detail(
                        error,
                        "Claude Bedrock credentials are missing or conflict with other provider credentials",
                    )
                })?;
        }
    }
    Ok(())
}

// Endpoint settings are caller-owned. Active transport selectors must still agree with an
// explicitly selected provider; otherwise Claude can silently send gateway credentials elsewhere.
fn validate_provider_selection(
    environment: &BTreeMap<String, String>,
    provider: ClaudeProvider,
) -> Result<(), NodeRunnerError> {
    if provider == ClaudeProvider::Anthropic {
        return Ok(());
    }
    for name in [
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_MANTLE",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "CLAUDE_CODE_USE_ANTHROPIC_AWS",
        "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
        "CLAUDE_CODE_USE_GATEWAY",
    ] {
        let compatible = provider == ClaudeProvider::Bedrock
            && matches!(name, "CLAUDE_CODE_USE_BEDROCK" | "CLAUDE_CODE_USE_MANTLE");
        let active = environment.get(name).is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        });
        if active && !compatible {
            return Err(NodeRunnerError::DriverDetail(format!(
                "Claude {name} conflicts with the selected provider"
            )));
        }
    }
    Ok(())
}
