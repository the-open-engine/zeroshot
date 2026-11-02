//! Connection fields shared by the Codex and Claude Microsoft Foundry lanes.
//!
//! One connection names a Foundry resource either by its resource name or by its root URL. Each
//! harness appends its own API path to that root, so one stored connection serves a run that mixes
//! Codex and Claude nodes on the same resource.

use std::collections::BTreeMap;

use crate::native_v2_capsule::gateway;
use crate::native_v2_runner::NodeRunnerError;

pub(crate) const API_KEY: &str = "AZURE_FOUNDRY_API_KEY";
pub(crate) const RESOURCE: &str = "AZURE_FOUNDRY_RESOURCE";
pub(crate) const BASE_URL: &str = "AZURE_FOUNDRY_BASE_URL";
pub(crate) const FIELDS: [&str; 3] = [API_KEY, RESOURCE, BASE_URL];

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Endpoint<'a> {
    /// An Azure resource name, such as `contoso-ai`.
    Resource(&'a str),
    /// The resource root URL without a trailing slash, for custom domains.
    BaseUrl(&'a str),
}

pub(crate) fn connection(
    values: &BTreeMap<String, String>,
) -> Result<(Endpoint<'_>, &str), NodeRunnerError> {
    let api_key = valid(values, API_KEY).ok_or_else(|| {
        NodeRunnerError::DriverDetail(format!("Foundry requires a non-empty valid {API_KEY}"))
    })?;
    let endpoint = match (values.get(RESOURCE), values.get(BASE_URL)) {
        (Some(_), None) => {
            let resource = valid(values, RESOURCE)
                .filter(|resource| valid_resource(resource))
                .ok_or_else(|| {
                    NodeRunnerError::DriverDetail(
                        concat!(
                            "AZURE_FOUNDRY_RESOURCE must be an Azure resource name of 2 to 64 ",
                            "letters, digits, or inner hyphens"
                        )
                        .to_owned(),
                    )
                })?;
            Endpoint::Resource(resource)
        }
        (None, Some(_)) => {
            let base_url = valid(values, BASE_URL)
                .filter(|base_url| gateway::validate_base_url(base_url).is_ok())
                .ok_or_else(|| {
                    NodeRunnerError::DriverDetail(
                        concat!(
                            "AZURE_FOUNDRY_BASE_URL must be an absolute HTTP(S) base URL ",
                            "without credentials, query, or fragment"
                        )
                        .to_owned(),
                    )
                })?;
            Endpoint::BaseUrl(base_url.trim_end_matches('/'))
        }
        _ => {
            return Err(NodeRunnerError::DriverDetail(format!(
                "Foundry requires exactly one of {RESOURCE} and {BASE_URL}"
            )));
        }
    };
    Ok((endpoint, api_key))
}

fn valid<'a>(values: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    values
        .get(name)
        .filter(|value| !value.trim().is_empty() && !value.chars().any(char::is_control))
        .map(String::as_str)
}

fn valid_resource(value: &str) -> bool {
    (2..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}

#[cfg(test)]
#[path = "foundry/tests.rs"]
mod tests;
