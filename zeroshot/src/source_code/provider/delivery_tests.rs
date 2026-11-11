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

#[derive(Default)]
pub(crate) struct RecordingReviewCheckpoint {
    pub(crate) states: std::sync::Mutex<Vec<serde_json::Value>>,
    pub(crate) fail_at: Option<usize>,
}
#[async_trait]
impl SourceReviewCheckpoint for RecordingReviewCheckpoint {
    async fn retain(&self, state: serde_json::Value) -> Result<(), SourceDeliveryError> {
        let mut states = self.states.lock().unwrap();
        states.push(state);
        if self.fail_at == Some(states.len()) {
            return Err(SourceDeliveryError::api(
                None,
                "test receipt persistence failed",
            ));
        }
        Ok(())
    }
}
