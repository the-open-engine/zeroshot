use std::collections::BTreeMap;

use serde_json::{Value, json};
use url::Url;
use crate::native_v2_runner::{NodeRunnerError, ResolvedEnvironment, refresh_environment};
use super::rpc::failure;

pub(super) const TOKEN: &str = "COPILOT_GITHUB_TOKEN";
pub(super) const EXPIRES_AT: &str = "COPILOT_GITHUB_TOKEN_EXPIRES_AT";
pub(super) const REGISTRATION: &str = "zeroshot-github-user";
const LOCAL_TOKEN_NAMES: &[&str] = &[TOKEN, "GH_TOKEN", "GITHUB_TOKEN"];

pub(super) struct CopilotAuthentication<'a> {
    environment: &'a ResolvedEnvironment,
    local_token: Option<&'a str>,
    native: bool,
    github_origin: String,
}

impl<'a> CopilotAuthentication<'a> {
    pub(super) fn new(
        environment: &'a ResolvedEnvironment,
        local_token: Option<&'a str>,
        native: bool,
        native_environment: &BTreeMap<String, String>,
    ) -> Result<Self, NodeRunnerError> {
        let authentication = Self {
            environment,
            local_token,
            native,
            github_origin: github_origin(environment, native_environment)?,
        };
        authentication.validate()?;
        Ok(authentication)
    }

    fn validate(&self) -> Result<(), NodeRunnerError> {
        for token in LOCAL_TOKEN_NAMES
            .iter()
            .filter_map(|name| field(self.environment, name))
        {
            if !valid_token(token) {
                return Err(failure("Copilot declared GitHub credential is invalid"));
            }
        }
        if self.token().is_none() && !self.native {
            return Err(failure(
                "Copilot requires COPILOT_GITHUB_TOKEN from a user-backed GitHub connection",
            ));
        }
        if let Some(expiry) = field(self.environment, EXPIRES_AT) {
            expiry
                .parse::<u64>()
                .map_err(|_| failure("Copilot credential expiry is invalid"))?;
            if field(self.environment, TOKEN).is_none() {
                return Err(failure(
                    "Copilot credential expiry requires a resolved user token",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn token(&self) -> Option<&'a str> {
        LOCAL_TOKEN_NAMES
            .iter()
            .find_map(|name| field(self.environment, name))
            .or(self.local_token)
            .filter(|value| valid_token(value))
    }

    pub(super) fn has_declared_token(&self) -> bool {
        LOCAL_TOKEN_NAMES
            .iter()
            .any(|name| field(self.environment, name).is_some())
    }

    pub(super) fn configure(&self, params: &mut Value) -> Result<(), NodeRunnerError> {
        if field(self.environment, EXPIRES_AT).is_some() {
            params["gitHubTokenProviderRegistrationId"] = json!(REGISTRATION);
        } else if let Some(token) = self.token() {
            params["gitHubToken"] = json!(token);
        }
        Ok(())
    }

    pub(super) async fn acquire(
        &self,
        params: &Value,
        session_id: &str,
    ) -> Result<ResolvedEnvironment, NodeRunnerError> {
        acquire_for_origin(params, self.environment, session_id, &self.github_origin).await
    }
}

pub(super) fn is_credential(name: &str) -> bool {
    is_credential_for_platform(name, cfg!(windows))
}

pub(super) fn is_credential_for_platform(name: &str, windows: bool) -> bool {
    std::iter::once(EXPIRES_AT)
        .chain(LOCAL_TOKEN_NAMES.iter().copied())
        .any(|candidate| {
            if windows {
                name.eq_ignore_ascii_case(candidate)
            } else {
                name == candidate
            }
        })
}

fn field<'a>(environment: &'a ResolvedEnvironment, name: &str) -> Option<&'a str> {
    environment
        .iter()
        .find(|(key, _)| key.as_str() == name)
        .map(|(_, value)| value)
}

pub(super) fn remove_local_credentials(environment: &mut BTreeMap<String, String>) {
    for name in LOCAL_TOKEN_NAMES {
        environment.remove(*name);
    }
}

pub(super) fn take_local_token(environment: &mut BTreeMap<String, String>) -> Option<String> {
    let token = LOCAL_TOKEN_NAMES
        .iter()
        .find_map(|name| environment.get(*name).filter(|value| valid_token(value)))
        .cloned();
    remove_local_credentials(environment);
    token
}

#[cfg(all(test, unix))]
pub(super) async fn acquire(
    params: &Value,
    environment: &ResolvedEnvironment,
    session_id: &str,
) -> Result<ResolvedEnvironment, NodeRunnerError> {
    let origin = github_origin(environment, &BTreeMap::new())?;
    acquire_for_origin(params, environment, session_id, &origin).await
}

async fn acquire_for_origin(
    params: &Value,
    environment: &ResolvedEnvironment,
    session_id: &str,
    expected_origin: &str,
) -> Result<ResolvedEnvironment, NodeRunnerError> {
    if params["registrationId"].as_str() != Some(REGISTRATION)
        || params["host"]
            .as_str()
            .and_then(|host| normalized_origin(host, false).ok())
            .as_deref()
            != Some(expected_origin)
        || params["sessionId"]
            .as_str()
            .is_some_and(|id| id != session_id)
    {
        return Err(failure(
            "Copilot requested a credential for an unexpected identity",
        ));
    }
    match params["reason"].as_str() {
        Some("initial") if remaining_lifetime(environment).is_ok() => Ok(environment.clone()),
        Some("initial" | "refresh") => refresh_environment(environment)
            .await
            .map_err(|_| failure("Copilot GitHub credential refresh failed")),
        _ => Err(failure("Copilot credential request reason is invalid")),
    }
}

pub(super) fn result(environment: &ResolvedEnvironment) -> Result<Value, NodeRunnerError> {
    let token = field(environment, TOKEN)
        .filter(|value| valid_token(value))
        .ok_or_else(|| failure("Copilot refreshed GitHub credential is missing"))?;
    Ok(json!({"kind":"token", "accessToken":token,
        "expiresIn":remaining_lifetime(environment)?}))
}

fn valid_token(value: &str) -> bool {
    !value.trim().is_empty() && !value.contains('\0')
}

fn github_origin(
    environment: &ResolvedEnvironment,
    native_environment: &BTreeMap<String, String>,
) -> Result<String, NodeRunnerError> {
    let configured = ["COPILOT_GH_HOST", "GH_HOST"]
        .into_iter()
        .find_map(|name| field(environment, name).filter(|value| !value.trim().is_empty()))
        .or_else(|| {
            ["COPILOT_GH_HOST", "GH_HOST"].into_iter().find_map(|name| {
                native_environment
                    .get(name)
                    .map(String::as_str)
                    .filter(|value| !value.trim().is_empty())
            })
        })
        .unwrap_or("github.com");
    normalized_origin(configured, true)
}

fn normalized_origin(value: &str, allow_hostname: bool) -> Result<String, NodeRunnerError> {
    let text = if allow_hostname && !value.contains("://") {
        format!("https://{value}")
    } else {
        value.to_owned()
    };
    let url = Url::parse(&text).map_err(|_| failure("Copilot GitHub host is invalid"))?;
    if !valid_github_url(&url) {
        return Err(failure("Copilot GitHub host is invalid"));
    }
    Ok(url.origin().ascii_serialization())
}

fn valid_github_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.path(), "" | "/")
        && url.query().is_none()
        && url.fragment().is_none()
}

fn remaining_lifetime(environment: &ResolvedEnvironment) -> Result<u64, NodeRunnerError> {
    let expiry = field(environment, EXPIRES_AT)
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| failure("Copilot credential expiry is missing or invalid"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| failure("system clock is invalid"))?
        .as_secs();
    expiry
        .checked_sub(now)
        .filter(|seconds| (3601..=9_007_199_254_740_991).contains(seconds))
        .ok_or_else(|| {
            failure("Copilot requires a refreshed GitHub token valid for more than one hour")
        })
}
