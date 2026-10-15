use super::*;

pub(crate) async fn reconcile(
    authority: &GhCliDeliveryAuthority,
    request: SourceHeadReconciliation<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
    let credential = credential.with_github_git_token(credential.expose());
    crate::native_v2_delivery::transport::reconcile_head(
        &authority.git_transport(request.workspace, &request.observed.repository),
        request,
        credential,
    )
    .await
}
