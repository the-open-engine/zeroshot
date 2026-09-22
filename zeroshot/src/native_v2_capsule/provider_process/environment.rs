//! Native CLI context inherited only by the local controller and local harness adapters.
//!
//! Declared connections overlay these values. Hosted adapters never call this helper.

use std::collections::BTreeMap;
use std::fmt;

pub(crate) const CODEX_LOCAL_ENVIRONMENT: &[&str] = &[
    "CODEX_API_KEY",
    "OPENAI_API_KEY",
    "CODEX_BASE_URL",
    "OPENAI_BASE_URL",
    "OPENAI_API_BASE",
];

pub(crate) const CLAUDE_LOCAL_ENVIRONMENT: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
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

pub(crate) const COPILOT_LOCAL_ENVIRONMENT: &[&str] = &[
    "COPILOT_GITHUB_TOKEN",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_HOST",
    "COPILOT_GH_HOST",
    "COPILOT_PROVIDER_BASE_URL",
    "COPILOT_PROVIDER_TYPE",
    "COPILOT_PROVIDER_API_KEY",
    "COPILOT_PROVIDER_API_KEY_COMMAND",
    "COPILOT_PROVIDER_BEARER_TOKEN",
    "COPILOT_PROVIDER_WIRE_API",
    "COPILOT_PROVIDER_TRANSPORT",
    "COPILOT_PROVIDER_AZURE_API_VERSION",
    "COPILOT_PROVIDER_MODEL_ID",
    "COPILOT_PROVIDER_WIRE_MODEL",
    "COPILOT_PROVIDER_MAX_PROMPT_TOKENS",
    "COPILOT_PROVIDER_MAX_OUTPUT_TOKENS",
    "COPILOT_PROVIDER_HEADERS",
    "COPILOT_PROVIDERS_CONFIG",
    "COPILOT_OFFLINE",
];

const LOCAL_TRANSPORT_ENVIRONMENT: &[&str] = &[
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_RUNTIME_DIR",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
];

pub(crate) fn current_process_environment() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

/// Private snapshot of the invoking shell used only by built-in local harnesses.
///
/// Its values cross the one-shot controller bootstrap but never the controller process
/// environment, hosted composition, durable state, or debug output. Harnesses select only the
/// names they understand; Codex may additionally resolve names reported by its native config.
#[derive(Clone, Eq, PartialEq)]
pub struct LocalHarnessEnvironment {
    values: BTreeMap<String, String>,
    windows: bool,
}

impl LocalHarnessEnvironment {
    pub(crate) fn new(values: BTreeMap<String, String>) -> Self {
        Self::for_platform(values, cfg!(windows))
    }

    fn for_platform(values: BTreeMap<String, String>, windows: bool) -> Self {
        Self { values, windows }
    }

    #[cfg(test)]
    fn for_windows(values: BTreeMap<String, String>) -> Self {
        Self::for_platform(values, true)
    }

    pub(crate) fn selected(&self, names: &[&str]) -> BTreeMap<String, String> {
        names
            .iter()
            .chain(LOCAL_TRANSPORT_ENVIRONMENT)
            .filter_map(|name| {
                self.get(name)
                    .map(|value| (self.output_name(name), value.clone()))
            })
            .collect()
    }

    pub(crate) fn get(&self, name: &str) -> Option<&String> {
        self.values
            .iter()
            .find(|(candidate, _)| self.names_match(candidate, name))
            .map(|(_, value)| value)
    }

    pub(crate) fn get_mut(&mut self, name: &str) -> Option<&mut String> {
        let key = self
            .values
            .keys()
            .find(|candidate| self.names_match(candidate, name))?
            .clone();
        self.values.get_mut(&key)
    }

    pub(crate) fn user_home(&self) -> Option<&String> {
        let names = if self.windows {
            ["USERPROFILE", "HOME"]
        } else {
            ["HOME", "USERPROFILE"]
        };
        names
            .into_iter()
            .find_map(|name| self.get(name).filter(|value| !value.is_empty()))
    }

    pub(crate) fn into_values(self) -> BTreeMap<String, String> {
        self.values
    }

    fn names_match(&self, left: &str, right: &str) -> bool {
        if self.windows {
            left.eq_ignore_ascii_case(right)
        } else {
            left == right
        }
    }

    fn output_name(&self, name: &str) -> String {
        if self.windows {
            name.to_ascii_uppercase()
        } else {
            name.to_owned()
        }
    }
}

impl Default for LocalHarnessEnvironment {
    fn default() -> Self {
        Self::new(BTreeMap::new())
    }
}

impl fmt::Debug for LocalHarnessEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalHarnessEnvironment")
            .field("fields", &self.values.len())
            .finish()
    }
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
                .filter(|(name, _)| private_local_value(name))
                .map(|(_, value)| value.as_str()),
        ),
    )
}

fn private_local_value(name: &str) -> bool {
    const PRIVATE_SUFFIXES: &[&str] = &[
        "_BASE_URL",
        "_API_KEY",
        "_AUTH_TOKEN",
        "_OAUTH_TOKEN",
        "_OAUTH_REFRESH_TOKEN",
        "_HEADERS",
        "_API_KEY_COMMAND",
        "_PROXY",
        "_proxy",
    ];
    const PRIVATE_NAMES: &[&str] = &[
        "OPENAI_API_BASE",
        "DBUS_SESSION_BUS_ADDRESS",
        "XDG_RUNTIME_DIR",
    ];
    PRIVATE_SUFFIXES.iter().any(|suffix| name.ends_with(suffix)) || PRIVATE_NAMES.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_environment_lookup_is_case_insensitive_and_prefers_userprofile() {
        let mut environment = LocalHarnessEnvironment::for_windows(BTreeMap::from([
            ("Path".to_owned(), r"C:\tools".to_owned()),
            ("openai_Api_Key".to_owned(), "provider-secret".to_owned()),
            ("Home".to_owned(), "/msys/home".to_owned()),
            ("UserProfile".to_owned(), r"C:\Users\native".to_owned()),
        ]));

        assert_eq!(
            environment.get("PATH").map(String::as_str),
            Some(r"C:\tools")
        );
        assert_eq!(
            environment.get("OPENAI_API_KEY").map(String::as_str),
            Some("provider-secret")
        );
        assert_eq!(
            environment.user_home().map(String::as_str),
            Some(r"C:\Users\native")
        );
        *environment
            .get_mut("OpenAI_API_Key")
            .expect("mixed-case key") = "updated-secret".to_owned();
        assert_eq!(
            environment.selected(&["PATH", "OPENAI_API_KEY"]),
            BTreeMap::from([
                ("OPENAI_API_KEY".to_owned(), "updated-secret".to_owned()),
                ("PATH".to_owned(), r"C:\tools".to_owned()),
            ])
        );
    }
}
