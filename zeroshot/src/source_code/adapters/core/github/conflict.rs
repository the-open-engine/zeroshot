//! Provider observation precedes shared exact-target conflict materialization.
use super::*;

pub(super) async fn materialize(
    authority: &GhCliDeliveryAuthority,
    request: &SourceConflictRequest,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceConflictOutcome, SourceDeliveryError> {
    let revision = authority
        .target_revision(&request.review, credential)
        .await?;
    crate::native_v2_delivery::transport::materialize_conflict(
        &authority.git_transport(&request.workspace, &request.review.repository),
        request,
        &revision,
        credential.with_github_git_token(credential.expose()),
    )
    .await
}

#[cfg(all(test, unix))]
#[path = "conflict/tests.rs"]
mod tests;
