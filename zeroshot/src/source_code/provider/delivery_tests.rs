use super::*;

pub(crate) struct TestReviewCheckpoint;
#[async_trait]
impl SourceReviewCheckpoint for TestReviewCheckpoint {
    async fn retain(&self, _state: serde_json::Value) -> Result<(), SourceDeliveryError> {
        Ok(())
    }
}

pub(crate) struct RefuseReviewCheckpoint;
#[async_trait]
impl SourceReviewCheckpoint for RefuseReviewCheckpoint {
    async fn retain(&self, _state: serde_json::Value) -> Result<(), SourceDeliveryError> {
        Err(SourceDeliveryError::api(
            None,
            "test receipt persistence failed",
        ))
    }
}
