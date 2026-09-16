//! Shell configuration inherited only by the local controller and local harness adapters.
//!
//! Credentials remain explicit connection inputs. Hosted adapters never call this helper.

use std::collections::BTreeMap;

pub(crate) const CODEX_LOCAL_ENVIRONMENT: &[&str] =
    &["CODEX_BASE_URL", "OPENAI_BASE_URL", "OPENAI_API_BASE"];

pub(crate) const CLAUDE_LOCAL_ENVIRONMENT: &[&str] = &[
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_FORCE_SANDBOX",
    "CLAUDE_CODE_RESTRICTED",
    "CLAUDE_CODE_AUTO_MODE_EXTERNAL_PERMISSIONS",
    "CLAUDE_BG_SESSION_PERMISSION_RULES",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_BEDROCK_BASE_URL",
    "ANTHROPIC_BEDROCK_MANTLE_BASE_URL",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_GATEWAY",
    "CLAUDE_CODE_USE_MANTLE",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
];

pub(crate) fn local_environment(names: &[&str]) -> BTreeMap<String, String> {
    names
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| ((*name).to_owned(), value))
        })
        .collect()
}

/// Local endpoint settings may contain credentials in URL components.
pub(crate) fn provider_redactions(
    environment: &crate::native_v2_runner::ResolvedEnvironment,
    local: &BTreeMap<String, String>,
) -> Vec<String> {
    super::redaction_values(
        environment.iter().map(|(_, value)| value).chain(
            local
                .iter()
                .filter(|(name, _)| {
                    name.ends_with("_BASE_URL") || name.as_str() == "OPENAI_API_BASE"
                })
                .map(|(_, value)| value.as_str()),
        ),
    )
}
