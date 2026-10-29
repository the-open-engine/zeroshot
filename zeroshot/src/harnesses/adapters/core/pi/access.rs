use crate::harnesses::ProviderAccessContract;
use crate::native_v2_contract::PiProvider;

pub(super) fn provider_access(provider: PiProvider) -> ProviderAccessContract {
    match provider {
        PiProvider::Github => ProviderAccessContract::new(
            true,
            "github",
            &["COPILOT_GITHUB_TOKEN"],
            &[&["COPILOT_GITHUB_TOKEN"]],
        ),
        PiProvider::Anthropic => ProviderAccessContract::new(
            true,
            "anthropic",
            &["ANTHROPIC_API_KEY"],
            &[
                &["ANTHROPIC_API_KEY"],
                &["ANTHROPIC_AUTH_TOKEN"],
                &["ANTHROPIC_OAUTH_TOKEN"],
                // A caller-owned Messages-compatible endpoint, the same escape hatch Claude Code
                // offers through `ANTHROPIC_BASE_URL`.
                &["ANTHROPIC_BASE_URL", "ANTHROPIC_API_KEY"],
            ],
        ),
        PiProvider::OpenAi => ProviderAccessContract::new(
            true,
            "openai",
            &["OPENAI_API_KEY"],
            &[
                &["OPENAI_API_KEY"],
                // A caller-owned OpenAI-compatible endpoint, mirroring what the Codex lane
                // inherits. The lane keeps the built-in Responses protocol.
                &["OPENAI_BASE_URL", "OPENAI_API_KEY"],
                &["OPENAI_API_BASE", "OPENAI_API_KEY"],
            ],
        ),
        PiProvider::OpenRouter => ProviderAccessContract::new(
            false,
            "openrouter",
            &["OPENROUTER_API_KEY"],
            &[&["OPENROUTER_API_KEY"]],
        ),
        PiProvider::Gateway => ProviderAccessContract::new(
            false,
            "gateway",
            // The wire protocol is authored alongside the endpoint, never inferred from the
            // caller-owned model identifier.
            &["GATEWAY_BASE_URL", "GATEWAY_API_KEY", "GATEWAY_API"],
            &[&["GATEWAY_BASE_URL", "GATEWAY_API_KEY", "GATEWAY_API"]],
        ),
        PiProvider::Bedrock => ProviderAccessContract::new(
            false,
            "bedrock",
            &["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"],
            &[&["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"]],
        ),
    }
}
