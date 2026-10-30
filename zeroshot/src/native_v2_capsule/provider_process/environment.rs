//! Native CLI context inherited only by the local controller and local harness adapters.
//!
//! Declared connections overlay these values. Hosted adapters never call this helper.

use std::collections::BTreeMap;
use crate::native_v2_runner::{NodeRunnerError, ResolvedEnvironment};
use std::fmt;

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
        .filter(|(name, _)| !name.eq_ignore_ascii_case("ZEROSHOT_API_KEY"))
        .collect()
}

/// Private snapshot of the invoking shell used only by local harness adapters.
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

    fn for_platform(mut values: BTreeMap<String, String>, windows: bool) -> Self {
        values.retain(|name, _| !name.eq_ignore_ascii_case("ZEROSHOT_API_KEY"));
        Self { values, windows }
    }

    #[cfg(test)]
    pub(crate) fn for_windows(values: BTreeMap<String, String>) -> Self {
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
        environment
            .iter()
            .map(|(_, value)| value)
            .chain(private_local_values(local)),
    )
}

/// Captured native context values that may carry credentials.
pub(crate) fn private_local_values(local: &BTreeMap<String, String>) -> impl Iterator<Item = &str> {
    local
        .iter()
        .filter(|(name, _)| private_local_value(name))
        .map(|(_, value)| value.as_str())
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

/// Declaring one authentication form suppresses every ambient form in that credential group.
pub(crate) fn merge_local_environment(
    environment: &mut BTreeMap<String, String>,
    resolved: &ResolvedEnvironment,
    local: &BTreeMap<String, String>,
    credentials: &[&str],
) {
    let declared_auth = resolved
        .iter()
        .any(|(name, _)| credentials.contains(&name.as_str()));
    for (name, value) in local {
        if declared_auth && credentials.contains(&name.as_str()) {
            continue;
        }
        environment
            .entry(name.clone())
            .or_insert_with(|| value.clone());
    }
}

/// Overlay declared connections without replacing harness-owned launch settings.
pub(crate) fn extend_declared_environment(
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

#[cfg(test)]
#[path = "environment/tests.rs"]
mod tests;
