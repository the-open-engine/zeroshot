use openengine_cluster_protocol::HarnessConfiguration;
use crate::harnesses::native_config;

pub(crate) fn validate(config: &HarnessConfiguration) -> Result<(), String> {
    let options = native_config::options(config)?;
    native_config::reserved_options(
        &options,
        &[
            "sessionId",
            "model",
            "reasoningEffort",
            "provider",
            "providers",
            "githubToken",
            "gitHubToken",
            "gitHubTokenProviderRegistrationId",
            "models",
            "githubTokenExpiresAt",
            "authInfo",
            "authorization",
            "apiKey",
            "workingDirectory",
            "configDir",
            "env",
            "envValueMode",
            "systemMessage",
            "customAgents",
            "agent",
            "agentName",
            "systemPrompt",
            "hooks",
            "onPermissionRequest",
            "requestPermission",
            "requestUserInput",
            "streaming",
            "clientName",
            "enableConfigDiscovery",
            "enableFileHooks",
            "enableHostGitOperations",
            "enableSessionTelemetry",
            "outputSchema",
            "responseFormat",
        ],
    )
}
