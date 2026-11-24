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
mod tests {
    use super::*;
    use openengine_cluster_protocol::RunResumeResult;
    use serde_json::json;

    #[test]
    fn hosted_guards_survive_transport_and_legacy_adaptation() {
        let root = RunId::new("root");
        let child = RunId::new("child");
        let address = Url::parse("https://example.test/logs?execution=node").unwrap();
        let selected = selected_url(address.clone(), true, &root, Some(&child)).unwrap();
        assert_eq!(selected.query(), Some("execution=node&partId=child"));
        assert!(selected_url(address, false, &root, Some(&child)).is_err());

        let mut body = json!({"runId":root,"partId":child,"successorPartId":"next"});
        assert_eq!(request(true, &body).unwrap(), body);
        assert!(request(false, &body).is_err());
        body["partId"] = json!(root);
        assert_eq!(
            request(false, &body).unwrap(),
            json!({"runId":root,"successorRunId":"next"})
        );

        let old = br#"{"runId":"next","resumedFrom":"root"}"#;
        let result: RunResumeResult = decode(old, false, |v| {
            physical_runs::read_response(openengine_cluster_protocol::RUN_RESUME_METHOD, v)
        })
        .unwrap();
        assert_eq!(result.current_part_id.as_str(), "next");
        assert_eq!(result.resumed_from, root);
        assert!(decode::<RunResumeResult>(old, true, |_| {}).is_err());
    }
}
