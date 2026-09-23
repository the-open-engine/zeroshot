//! Connection fields shared by the Codex and Claude gateway lanes.

use std::collections::BTreeMap;

use url::Url;

use crate::native_v2_runner::NodeRunnerError;

pub(crate) const BASE_URL: &str = "GATEWAY_BASE_URL";
pub(crate) const API_KEY: &str = "GATEWAY_API_KEY";

pub(crate) fn connection(
    values: &BTreeMap<String, String>,
) -> Result<(&str, &str), NodeRunnerError> {
    let base_url = required(values, BASE_URL)?;
    let api_key = required(values, API_KEY)?;
    validate_base_url(base_url)?;
    Ok((base_url, api_key))
}

fn required<'a>(
    values: &'a BTreeMap<String, String>,
    name: &str,
) -> Result<&'a str, NodeRunnerError> {
    values
        .get(name)
        .filter(|value| !value.trim().is_empty() && !value.chars().any(char::is_control))
        .map(String::as_str)
        .ok_or_else(|| {
            NodeRunnerError::DriverDetail(format!("gateway requires a non-empty valid {name}"))
        })
}

fn validate_base_url(value: &str) -> Result<(), NodeRunnerError> {
    let invalid = || {
        NodeRunnerError::DriverDetail(
            concat!(
                "GATEWAY_BASE_URL must be an absolute HTTP(S) base URL ",
                "without credentials, query, or fragment"
            )
            .to_owned(),
        )
    };
    if value.len() > 8192 || value.chars().any(char::is_whitespace) || value.contains('\\') {
        return Err(invalid());
    }
    let url = Url::parse(value).map_err(|_| invalid())?;
    let http = value.split_once("://").is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
    });
    let plain = [url.password(), url.query(), url.fragment()]
        .iter()
        .all(Option::is_none);
    if http && url.host_str().is_some() && url.username().is_empty() && plain {
        Ok(())
    } else {
        Err(invalid())
    }
}

#[cfg(test)]
#[path = "gateway/tests.rs"]
mod tests;
