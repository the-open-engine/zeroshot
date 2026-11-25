//! A single project read includes the Enterprise version header.
use super::*;

#[cfg(test)]
mod tests;

pub(super) async fn check(
    authority: &GhCliDeliveryAuthority,
    repository: &str,
    credential: SourceDeliveryCredential<'_>,
    stopping: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<(), SourceDeliveryError> {
    let path = repository_path(repository, &[])?;
    let http = authority.http().await?;
    let request = http.send(
        Request::get(&path),
        credential,
        (authority.api_deadline(), 8 * 1024 * 1024),
    );
    let reply = tokio::select! {
        biased;
        () = crate::execution::process::wait_for_stop_request(stopping) => {
            return Err(SourceDeliveryError::api(None, "GitHub project access request was stopped before it completed"));
        }
        reply = request => reply.map_err(|failure| authority.http_failure(failure, credential))?,
    };
    let value = authority
        .http_json(&reply, credential)
        .map_err(|failure| failure.error)?;
    if !value["full_name"]
        .as_str()
        .is_some_and(|name| name.eq_ignore_ascii_case(repository))
    {
        return Err(SourceDeliveryError::identity(
            "GitHub project read did not return the selected repository",
        ));
    }
    if !authority.is_cloud() {
        let version = reply
            .head
            .headers
            .get("x-github-enterprise-version")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                super::instance::server_version(
                    value
                        .trim()
                        .strip_prefix("enterprise-server@")
                        .unwrap_or(value.trim()),
                )
            });
        if !version.is_some_and(|version| version >= (3, 22, 2)) {
            return Err(SourceDeliveryError::identity(
                "GitHub server version check failed: Enterprise Server 3.22.2 or newer is required; version is unsupported or unavailable",
            ));
        }
    }
    Ok(())
}
