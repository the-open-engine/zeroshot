use std::collections::BTreeMap;
use std::ffi::OsString;
use crate::native_v2_delivery::validation::TrustedDeliveryValidator;
use std::path::Path;

use crate::native_v2_delivery::{
    GITHUB_TOKEN_FILE_ENV, TrustedGitHubTokenFile, validate_github_delivery_token,
};
use super::LocalCompositionError;

pub(crate) const DELIVERY_CHECK_COMMAND_ENV: &str = "ZEROSHOT_DELIVERY_CHECK_COMMAND";

/// Validate caller controls before the native UTF-8 snapshot can discard malformed values.
pub(crate) fn validate_caller_authority<F>(available: F) -> Result<(), LocalCompositionError>
where
    F: Fn(&str) -> Option<OsString>,
{
    for name in [GITHUB_TOKEN_FILE_ENV, DELIVERY_CHECK_COMMAND_ENV] {
        if available(name).is_some_and(|value| value.to_str().is_none()) {
            return Err(LocalCompositionError::NativeEnvironment);
        }
    }
    if available(GITHUB_TOKEN_FILE_ENV).is_some() {
        if let Some(token) = available("GH_TOKEN") {
            let token = token
                .into_string()
                .map_err(|_| LocalCompositionError::GitHubTokenFile)?;
            validate_github_delivery_token(&token)
                .map_err(|_| LocalCompositionError::GitHubTokenFile)?;
        }
    }
    Ok(())
}

pub(crate) fn take_delivery_validator(
    environment: &mut BTreeMap<String, String>,
    workspace: &Path,
) -> Result<Option<TrustedDeliveryValidator>, LocalCompositionError> {
    take_environment_field(environment, DELIVERY_CHECK_COMMAND_ENV)
        .map(|path| {
            TrustedDeliveryValidator::new(path.into(), workspace)
                .map_err(|_| LocalCompositionError::DeliveryValidator)
        })
        .transpose()
}

/// Extract controller authority before handing the invoking-shell snapshot to harness adapters.
pub(crate) fn take_github_authority(
    environment: &mut BTreeMap<String, String>,
    explicit: Option<String>,
    invoking_directory: &Path,
    workspace: &Path,
) -> Result<(Option<String>, Option<TrustedGitHubTokenFile>), LocalCompositionError> {
    let Some(path) = take_environment_field(environment, GITHUB_TOKEN_FILE_ENV) else {
        return Ok((explicit, None));
    };
    let token = explicit.or_else(|| take_environment_field(environment, "GH_TOKEN"));
    take_environment_field(environment, "GH_TOKEN");
    if let Some(token) = token {
        validate_github_delivery_token(&token)
            .map_err(|_| LocalCompositionError::GitHubTokenFile)?;
        return Ok((Some(token), None));
    }
    let path = std::path::PathBuf::from(path);
    if path.as_os_str().is_empty() {
        return Err(LocalCompositionError::GitHubTokenFile);
    }
    let path = if path.is_absolute() {
        path
    } else {
        invoking_directory.join(path)
    };
    let file = TrustedGitHubTokenFile::new(path, workspace)
        .map_err(|_| LocalCompositionError::GitHubTokenFile)?;
    Ok((None, Some(file)))
}

/// The final harness boundary also covers callers that construct local candidates directly.
pub(crate) fn provider_environment(
    environment: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut selected = environment.clone();
    for name in [GITHUB_TOKEN_FILE_ENV, DELIVERY_CHECK_COMMAND_ENV] {
        take_environment_field(&mut selected, name);
    }
    selected
}

fn take_environment_field(
    environment: &mut BTreeMap<String, String>,
    name: &str,
) -> Option<String> {
    let key = environment
        .keys()
        .find(|key| {
            if cfg!(windows) {
                key.eq_ignore_ascii_case(name)
            } else {
                key.as_str() == name
            }
        })
        .cloned()?;
    environment.remove(&key)
}

#[cfg(test)]
#[path = "token_file/tests.rs"]
mod tests;
