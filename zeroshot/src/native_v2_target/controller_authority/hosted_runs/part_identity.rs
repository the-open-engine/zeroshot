//! Translate the physical-ID contract only for hosts predating run-parts discovery.
use openengine_cluster_protocol::{RunId, bounded_json_from_slice, physical_runs};
use reqwest::Url;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::{TargetAuthorityError, authority_error};

pub(in super::super) fn negotiate(
    request: reqwest::RequestBuilder,
    supported: bool,
) -> reqwest::RequestBuilder {
    if supported {
        request.header(openengine_cluster_protocol::RUN_PARTS_HEADER, "1")
    } else {
        request
    }
}

pub(super) fn selected_url(
    mut url: Url,
    supports_parts: bool,
    run: &RunId,
    part: Option<&RunId>,
) -> Result<Url, TargetAuthorityError> {
    if let Some(part) = part {
        validate_part(supports_parts, run, part)?;
        if supports_parts {
            url.query_pairs_mut().append_pair("partId", part.as_str());
        }
    }
    Ok(url)
}

pub(super) fn validate_part(
    supports_parts: bool,
    run: &RunId,
    part: &RunId,
) -> Result<(), TargetAuthorityError> {
    if !supports_parts && part != run {
        return Err(unsupported());
    }
    Ok(())
}

fn unsupported() -> TargetAuthorityError {
    authority_error(
        "target requires openengine.run-parts/v1 to address a different part of this run",
    )
}

pub(super) fn request<T: Serialize>(
    supports_parts: bool,
    params: &T,
) -> Result<Value, TargetAuthorityError> {
    let mut body =
        serde_json::to_value(params).map_err(|_| authority_error("malformed run request"))?;
    if !supports_parts && !physical_runs::write_request(&mut body) {
        return Err(unsupported());
    }
    Ok(body)
}

pub(super) fn decode<T: DeserializeOwned>(
    bytes: &[u8],
    supports_parts: bool,
    upgrade: impl FnOnce(&mut Value),
) -> Result<T, serde_json::Error> {
    if supports_parts {
        bounded_json_from_slice(bytes)
    } else {
        let mut value = bounded_json_from_slice(bytes)?;
        upgrade(&mut value);
        serde_json::from_value(value)
    }
}

#[cfg(test)]
#[path = "part_identity_tests.rs"]
mod tests;
