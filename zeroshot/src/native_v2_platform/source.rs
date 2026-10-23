use std::collections::BTreeSet;

use openengine_cluster_protocol::EnvironmentVariableName;

use super::{RunSecretEnvelope, RunSourceCredentials};
use crate::native_v2_delivery::GITHUB_TOKEN_ENV;
use crate::native_v2_supervisor::RunEnvironmentError;

pub(super) async fn source_github_token(
    secrets: &RunSecretEnvelope,
) -> Result<Option<String>, RunEnvironmentError> {
    if secrets.environment.has_git_source_connection() {
        return Ok(secrets.source_credentials.github_token.clone());
    }
    let field = EnvironmentVariableName::new(GITHUB_TOKEN_ENV)
        .map_err(|_| RunEnvironmentError::InvalidPlan)?;
    let Some(values) = secrets
        .environment
        .resolve_source(BTreeSet::from([field.clone()]))
        .await?
    else {
        return Ok(secrets.source_credentials.github_token.clone());
    };
    values
        .as_map()
        .get(&field)
        .cloned()
        .map(Some)
        .ok_or(RunEnvironmentError::InvalidPlan)
}

pub(super) async fn source_credentials(
    secrets: &RunSecretEnvelope,
) -> Result<RunSourceCredentials, RunEnvironmentError> {
    let git_credentials = secrets
        .environment
        .resolve_git_source()
        .await?
        .or_else(|| secrets.source_credentials.git_credentials.clone());
    let github_token = source_github_token(secrets).await?;
    Ok(RunSourceCredentials {
        github_token,
        git_credentials,
    })
}

#[cfg(test)]
#[path = "source/tests.rs"]
mod tests;
