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
    authority.network.validate_certificate_file()?;
    let response = project_response(authority, repository, credential, stopping).await?;
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .or_else(|| response.split_once("\n\n"))
        .ok_or_else(|| {
            SourceDeliveryError::identity("GitHub project response headers are missing")
        })?;
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| SourceDeliveryError::identity("GitHub project response is invalid JSON"))?;
    if !value["full_name"]
        .as_str()
        .is_some_and(|name| name.eq_ignore_ascii_case(repository))
    {
        return Err(SourceDeliveryError::identity(
            "GitHub project read did not return the selected repository",
        ));
    }
    if !authority.is_cloud() {
        let version = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("x-github-enterprise-version"))
            .and_then(|(_, value)| {
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

/// A controller also makes this read while it prepares a checkpoint resume. The shared capture
/// owns the `gh` process tree, so a stop request ends the read with confirmed cleanup.
async fn project_response(
    authority: &GhCliDeliveryAuthority,
    repository: &str,
    credential: SourceDeliveryCredential<'_>,
    stopping: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<String, SourceDeliveryError> {
    let mut command = authority.api_command(credential);
    command
        .arg("api")
        .arg(format!("repos/{repository}"))
        .args(["--method", "GET", "--include"])
        .args(["--header", "X-GitHub-Api-Version: 2026-03-10"]);
    let mut secrets = authority.network.secrets();
    secrets.push(credential.expose().to_owned());
    let output = crate::native_v2_delivery::command::capture_sensitive_response_until_stopped(
        &mut command,
        authority.config.api_deadline,
        &secrets,
        stopping,
    )
    .await
    .and_then(|output| output.require_success())
    .map_err(|failure| super::api::command_failure(failure, credential))?;
    if output.stdout_truncated() {
        return Err(SourceDeliveryError::identity(
            "GitHub project response exceeded its byte bound",
        ));
    }
    if !output.stdout_valid_utf8() {
        return Err(SourceDeliveryError::identity(
            "GitHub project response is not UTF-8",
        ));
    }
    Ok(output.stdout().to_owned())
}
