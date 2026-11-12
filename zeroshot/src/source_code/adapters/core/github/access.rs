//! A single project read includes the Enterprise version header.
use super::*;

#[cfg(test)]
mod tests;

pub(super) async fn check(
    authority: &GhCliDeliveryAuthority,
    repository: &str,
    credential: SourceDeliveryCredential<'_>,
) -> Result<(), SourceDeliveryError> {
    authority.network.validate_certificate_file()?;
    let bytes = authority
        .raw_api_output(
            &[
                format!("repos/{repository}"),
                "--method".into(),
                "GET".into(),
                "--include".into(),
            ],
            credential,
        )
        .await?;
    let response = std::str::from_utf8(&bytes)
        .map_err(|_| SourceDeliveryError::identity("GitHub project response is not UTF-8"))?;
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
