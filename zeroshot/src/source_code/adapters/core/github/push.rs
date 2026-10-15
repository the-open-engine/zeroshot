use super::{GhCliDeliveryAuthority, SourceDeliveryError, SourceDeliveryCredential, SourcePushRequest};

pub(super) async fn push_branch(
    authority: &GhCliDeliveryAuthority,
    request: &SourcePushRequest,
    credential: SourceDeliveryCredential<'_>,
) -> Result<(), SourceDeliveryError> {
    let credential = credential.with_github_git_token(credential.expose());
    authority
        .git_transport(&request.workspace, &request.target.repository)
        .push(&request.head_branch, &request.head_revision, credential)
        .await
}

#[cfg(all(test, unix))]
#[path = "push/tests.rs"]
mod tests;
