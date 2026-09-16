use serde_json::{Value, json};
use crate::native_v2_runner::{NodeRunnerError, ResolvedEnvironment, refresh_environment};
use super::rpc::failure;

pub(super) const TOKEN: &str = "COPILOT_GITHUB_TOKEN";
pub(super) const EXPIRES_AT: &str = "COPILOT_GITHUB_TOKEN_EXPIRES_AT";
pub(super) const REGISTRATION: &str = "zeroshot-github-user";

pub(super) fn is_credential(name: &str) -> bool {
    matches!(name, TOKEN | EXPIRES_AT)
}

fn field<'a>(environment: &'a ResolvedEnvironment, name: &str) -> Option<&'a str> {
    environment
        .iter()
        .find(|(key, _)| key.as_str() == name)
        .map(|(_, value)| value)
}

pub(super) fn validate(environment: &ResolvedEnvironment) -> Result<(), NodeRunnerError> {
    token(environment)?;
    if let Some(expiry) = field(environment, EXPIRES_AT) {
        expiry
            .parse::<u64>()
            .map_err(|_| failure("Copilot credential expiry is invalid"))?;
    }
    Ok(())
}

fn token(environment: &ResolvedEnvironment) -> Result<&str, NodeRunnerError> {
    field(environment, TOKEN)
        .filter(|value| !value.trim().is_empty() && !value.contains('\0'))
        .ok_or_else(|| {
            failure("Copilot requires COPILOT_GITHUB_TOKEN from a user-backed GitHub connection")
        })
}

pub(super) fn configure(
    params: &mut Value,
    environment: &ResolvedEnvironment,
) -> Result<(), NodeRunnerError> {
    if field(environment, EXPIRES_AT).is_some() {
        params["gitHubTokenProviderRegistrationId"] = json!(REGISTRATION);
    } else {
        params["gitHubToken"] = json!(token(environment)?);
    }
    Ok(())
}

pub(super) async fn acquire(
    params: &Value,
    environment: &ResolvedEnvironment,
    session_id: &str,
) -> Result<ResolvedEnvironment, NodeRunnerError> {
    if params["registrationId"].as_str() != Some(REGISTRATION)
        || params["host"].as_str() != Some("github.com")
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
    Ok(json!({"kind":"token", "accessToken":token(environment)?,
        "expiresIn":remaining_lifetime(environment)?}))
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
