//! GitHub observes its target through its API; the common transport owns Git reconciliation.
use super::*;
use serde::Deserialize;
use crate::source_code_provider::DeliveryTarget;
use super::wire::reference_revision;

pub(super) async fn reconcile(
    authority: &GhCliDeliveryAuthority,
    request: SourceTargetReconciliation<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceTargetIntegration, SourceDeliveryError> {
    if queue_owns_integration(authority, request.target, credential).await? {
        return Ok(SourceTargetIntegration::Deferred);
    }
    let revision = observe_target(authority, request.target, credential).await?;
    crate::native_v2_delivery::transport::reconcile_target(
        &authority.git_transport(request.workspace, &request.target.repository),
        request,
        &revision,
        credential.with_github_git_token(credential.expose()),
    )
    .await
}

const TARGET_QUEUE_QUERY: &str = r#"
query($owner: String!, $name: String!, $branch: String!) {
  repository(owner: $owner, name: $name) {
    nameWithOwner
    mergeQueue(branch: $branch) { id }
  }
}
"#;

#[derive(Deserialize)]
struct TargetQueueResponse {
    data: TargetQueueData,
}

#[derive(Deserialize)]
struct TargetQueueData {
    repository: TargetQueueRepository,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetQueueRepository {
    name_with_owner: String,
    // Required presence distinguishes an absent queue (null) from an incomplete response.
    merge_queue: serde_json::Value,
}

#[derive(Deserialize)]
struct TargetQueue {
    id: String,
}

async fn queue_owns_integration(
    authority: &GhCliDeliveryAuthority,
    target: &DeliveryTarget,
    credential: SourceDeliveryCredential<'_>,
) -> Result<bool, SourceDeliveryError> {
    let (owner, name) = target
        .repository
        .split_once('/')
        .filter(|(owner, name)| !owner.is_empty() && !name.is_empty() && !name.contains('/'))
        .ok_or(SourceDeliveryError::Rejected)?;
    let variables = json!({"owner": owner, "name": name, "branch": target.target_branch});
    let value = authority
        .graphql(TARGET_QUEUE_QUERY, &variables, credential)
        .await?;
    let response: TargetQueueResponse = authority.decode_response(value, credential)?;
    if response.data.repository.name_with_owner != target.repository {
        return Err(SourceDeliveryError::Rejected);
    }
    let queue = response.data.repository.merge_queue;
    if queue.is_null() {
        return Ok(false);
    }
    let queue: TargetQueue = authority.decode_response(queue, credential)?;
    if queue.id.trim().is_empty() {
        return Err(SourceDeliveryError::Rejected);
    }
    Ok(true)
}

async fn observe_target(
    authority: &GhCliDeliveryAuthority,
    target: &DeliveryTarget,
    credential: SourceDeliveryCredential<'_>,
) -> Result<String, SourceDeliveryError> {
    let reference = authority
        .read_reference(&target.repository, &target.target_branch, credential)
        .await?;
    reference_revision(reference, &target.target_branch)
}

#[cfg(all(test, unix))]
#[path = "target_tests.rs"]
mod tests;
