//! Authentication, project readability and server version in one read-only GraphQL query.
use serde::Deserialize;
use serde_json::json;
use super::*;

const QUERY: &str = "query ZeroshotForgeAccess($project: ID!) { currentUser { id } metadata { version } project(fullPath: $project) { fullPath webUrl } }";

#[derive(Deserialize)]
struct AccessResponse {
    data: Option<AccessData>,
    errors: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccessData {
    current_user: Option<User>,
    metadata: Option<wire::ServerMetadata>,
    project: Option<AccessProject>,
}

#[derive(Deserialize)]
struct User {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccessProject {
    full_path: String,
    web_url: String,
}

pub(super) async fn check(
    authority: &GitLabDeliveryAuthority,
    repository: &str,
    credential: SourceDeliveryCredential<'_>,
    stopping: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<(), SourceDeliveryError> {
    let request = authority.api.json::<AccessResponse>(
        ApiRequest::graphql(QUERY, json!({"project": repository})),
        credential,
    );
    // A stop request drops the HTTP request; this client starts no process to clean up.
    let response = tokio::select! {
        biased;
        () = crate::execution::process::wait_for_stop_request(stopping) => {
            return Err(SourceDeliveryError::api(
                None,
                "GitLab project access request was stopped before it completed",
            ));
        }
        response = request => response?,
    };
    if response.errors.is_some_and(|errors| !errors.is_empty()) {
        return Err(SourceDeliveryError::identity(
            "GitLab authenticated project/version query returned GraphQL errors; project access or server metadata is unavailable",
        ));
    }
    let data = response.data.ok_or_else(|| {
        SourceDeliveryError::identity("GitLab project access query returned no data")
    })?;
    if !data.current_user.is_some_and(|user| !user.id.is_empty()) {
        return Err(SourceDeliveryError::api(
            Some(401),
            "GitLab authentication check failed: token did not identify a user",
        ));
    }
    let project = data.project.ok_or_else(|| {
        SourceDeliveryError::identity(
            "GitLab project read failed: project is absent or token cannot read it",
        )
    })?;
    if project.full_path != repository
        || project.web_url != format!("{}/{repository}", authority.base_url)
    {
        return Err(SourceDeliveryError::identity(
            "GitLab project read returned a different project identity",
        ));
    }
    let metadata = data.metadata.ok_or_else(|| SourceDeliveryError::identity("GitLab server version check failed: version is unavailable; 19.5.1 or newer is required"))?;
    api::require_version(&metadata.version)
}
