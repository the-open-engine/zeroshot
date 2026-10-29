use crate::harnesses::ProviderAccessContract;
use crate::native_v2_contract::CopilotProvider;

pub(super) fn provider_access(provider: CopilotProvider) -> ProviderAccessContract {
    match provider {
        CopilotProvider::Github => ProviderAccessContract::new(
            true,
            "github",
            &["COPILOT_GITHUB_TOKEN"],
            &[&["COPILOT_GITHUB_TOKEN"]],
        ),
        CopilotProvider::OpenAi => {
            ProviderAccessContract::new(true, "openai", &["OPENAI_API_KEY"], &[&["OPENAI_API_KEY"]])
        }
        CopilotProvider::Anthropic => ProviderAccessContract::new(
            true,
            "anthropic",
            &["ANTHROPIC_API_KEY"],
            &[&["ANTHROPIC_API_KEY"]],
        ),
        CopilotProvider::OpenRouter => ProviderAccessContract::new(
            true,
            "openrouter",
            &["OPENROUTER_API_KEY"],
            &[&["OPENROUTER_API_KEY"]],
        ),
        CopilotProvider::Bedrock => ProviderAccessContract::new(
            false,
            "bedrock",
            &["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"],
            &[&["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"]],
        ),
        CopilotProvider::Gateway => ProviderAccessContract::new(
            false,
            "gateway",
            &["GATEWAY_BASE_URL", "GATEWAY_API_KEY", "GATEWAY_API"],
            &[&["GATEWAY_BASE_URL", "GATEWAY_API_KEY", "GATEWAY_API"]],
        ),
    }
}
