//! GitHub observes its target through its API; the common transport owns Git reconciliation.
use super::*;
use crate::source_code_provider::DeliveryTarget;
use super::wire::{GitReferenceWire, reference_revision};

pub(super) async fn reconcile(
    authority: &GhCliDeliveryAuthority,
    request: SourceTargetReconciliation<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceTargetIntegration, SourceDeliveryError> {
    let revision = observe_target(authority, request.target, credential).await?;
    crate::native_v2_delivery::transport::reconcile_target(
        &authority.git_transport(request.workspace, &request.target.repository),
        request,
        &revision,
        credential.with_github_git_token(credential.expose()),
    )
    .await
}

async fn observe_target(
    authority: &GhCliDeliveryAuthority,
    target: &DeliveryTarget,
    credential: SourceDeliveryCredential<'_>,
) -> Result<String, SourceDeliveryError> {
    let value = authority
        .api(
            &[format!(
                "repos/{}/git/ref/heads/{}",
                target.repository, target.target_branch
            )],
            credential,
        )
        .await?;
    let reference: GitReferenceWire = super::api::decode_response(value, credential)?;
    reference_revision(reference, &target.target_branch)
}

#[cfg(all(test, unix))]
#[path = "target_tests.rs"]
mod tests;
