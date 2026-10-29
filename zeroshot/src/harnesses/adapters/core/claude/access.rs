use crate::harnesses::ProviderAccessContract;
use crate::native_v2_contract::ClaudeProvider;

pub(super) fn provider_access(provider: ClaudeProvider) -> ProviderAccessContract {
    match provider {
        ClaudeProvider::Anthropic => ProviderAccessContract::new(
            true,
            "anthropic",
            &["ANTHROPIC_API_KEY"],
            &[
                &["ANTHROPIC_API_KEY"],
                &["ANTHROPIC_AUTH_TOKEN"],
                &["CLAUDE_CODE_OAUTH_TOKEN"],
                &["CLAUDE_CODE_OAUTH_REFRESH_TOKEN"],
            ],
        ),
        ClaudeProvider::OpenRouter => ProviderAccessContract::new(
            false,
            "openrouter",
            &["OPENROUTER_API_KEY"],
            &[&["OPENROUTER_API_KEY"]],
        ),
        ClaudeProvider::Gateway => ProviderAccessContract::new(
            false,
            "gateway",
            &["GATEWAY_BASE_URL", "GATEWAY_API_KEY"],
            &[&["GATEWAY_BASE_URL", "GATEWAY_API_KEY"]],
        ),
        ClaudeProvider::Bedrock => ProviderAccessContract::new(
            false,
            "bedrock",
            &["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"],
            &[&["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"]],
        ),
    }
}
