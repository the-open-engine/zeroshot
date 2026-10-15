//! Git-only delivery uses the common trusted workflow without any forge command or API.
use async_trait::async_trait;
use crate::native_v2_delivery::transport::{GitTransport, reconcile_head};
use crate::source_code_provider::provider::*;

#[derive(Clone, Debug)]
pub struct GitDeliveryAuthority {
    transport: GitTransport,
}
impl GitDeliveryAuthority {
    #[must_use]
    pub fn new(transport: GitTransport) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl SourceDeliveryAuthority for GitDeliveryAuthority {
    async fn observe_delivery(
        &self,
        request: SourceDeliveryRead<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceDeliverySnapshot, SourceDeliveryError> {
        if request.include_review {
            return Err(SourceDeliveryError::Rejected);
        }
        Ok(SourceDeliverySnapshot {
            review: None,
            head_revision: self
                .transport
                .read_ref(request.head_branch, credential)
                .await?,
        })
    }
    async fn reconcile_delivery_target(
        &self,
        _request: SourceTargetReconciliation<'_>,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceTargetIntegration, SourceDeliveryError> {
        Err(SourceDeliveryError::Rejected)
    }
    async fn reconcile_delivery_head(
        &self,
        request: SourceHeadReconciliation<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
        reconcile_head(&self.transport, request, credential).await
    }
    async fn push_branch(
        &self,
        request: &SourcePushRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        self.transport
            .push(&request.head_branch, &request.head_revision, credential)
            .await
    }
    async fn open_or_update_review(
        &self,
        _request: &SourceReviewRequest,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewReceipt, SourceDeliveryError> {
        Err(SourceDeliveryError::Rejected)
    }
    async fn inspect_review(
        &self,
        _review: &SourceReviewReceipt,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewObservation, SourceDeliveryError> {
        Err(SourceDeliveryError::Rejected)
    }
    async fn request_merge(
        &self,
        _review: &SourceReviewReceipt,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
        Err(SourceDeliveryError::Rejected)
    }
}
