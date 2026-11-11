//! Receipts for trusted replies distinguish our unchanged responses from new findings.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use sha2::{Digest, Sha256};
use crate::source_code_provider::{SourceDeliveryError, SourceReviewReceipt, SourceReviewResponse};

// Leave room for the runner/ledger envelope and other execution facts in the 1 MiB event.
// Check the serialized aggregate, including receipts accumulated across repair rounds.
pub(super) const MAX_CHECKPOINT_BYTES: usize = 512 * 1024;

pub(super) fn checkpoint(
    value: &impl serde::Serialize,
) -> Result<serde_json::Value, SourceDeliveryError> {
    let bytes = serde_json::to_vec(value).map_err(|_| SourceDeliveryError::Rejected)?;
    if bytes.len() > MAX_CHECKPOINT_BYTES {
        return Err(SourceDeliveryError::Rejected.with_context(
            "trusted conversation receipt storage is full; maintainer intervention is required",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| SourceDeliveryError::Rejected)
}

pub(super) fn hash(body: &str) -> String {
    format!("{:x}", Sha256::digest(body.as_bytes()))
}

pub(super) fn response_hash(
    response: &SourceReviewResponse,
) -> Result<String, SourceDeliveryError> {
    let bytes = serde_json::to_vec(response).map_err(|_| SourceDeliveryError::Rejected)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

type Identity = (String, String, String, u64);
pub(super) type ReplyReceipt = (Identity, String);

#[derive(Clone, Debug, Default)]
pub(super) struct PublishedReplies(Arc<Mutex<BTreeMap<Identity, String>>>);

fn identity(review: &SourceReviewReceipt, thread: &str, id: u64) -> Identity {
    (
        review.repository.clone(),
        review.review_id.clone(),
        thread.to_owned(),
        id,
    )
}

impl PublishedReplies {
    pub(super) fn checkpoint(&self) -> Result<Vec<ReplyReceipt>, SourceDeliveryError> {
        Ok(self
            .0
            .lock()
            .map_err(|_| SourceDeliveryError::Unavailable)?
            .iter()
            .map(|(id, hash)| (id.clone(), hash.clone()))
            .collect())
    }

    /// Reserve the largest possible numeric reply identity before posting anything.
    pub(super) fn with_reserved_reply(
        &self,
        review: &SourceReviewReceipt,
        thread: &str,
        body: &str,
    ) -> Result<Vec<ReplyReceipt>, SourceDeliveryError> {
        let mut replies = self.checkpoint()?;
        replies.push((identity(review, thread, u64::MAX), hash(body)));
        Ok(replies)
    }

    pub(super) fn restore(&self, values: Vec<ReplyReceipt>) -> Result<(), SourceDeliveryError> {
        let count = values.len();
        let restored: BTreeMap<_, _> = values.into_iter().collect();
        if count > 100_000 || restored.len() != count {
            return Err(SourceDeliveryError::Rejected);
        }
        *self
            .0
            .lock()
            .map_err(|_| SourceDeliveryError::Unavailable)? = restored;
        Ok(())
    }

    pub(super) fn record(
        &self,
        review: &SourceReviewReceipt,
        thread: &str,
        reply: (u64, &str),
    ) -> Result<(), SourceDeliveryError> {
        let mut receipts = self
            .0
            .lock()
            .map_err(|_| SourceDeliveryError::Unavailable)?;
        if receipts.len() >= 100_000 {
            return Err(SourceDeliveryError::Rejected);
        }
        receipts.insert(identity(review, thread, reply.0), hash(reply.1));
        Ok(())
    }

    pub(super) fn matches(
        &self,
        review: &SourceReviewReceipt,
        thread: &str,
        reply: (u64, &str),
    ) -> Result<bool, SourceDeliveryError> {
        let digest = hash(reply.1);
        Ok(self
            .0
            .lock()
            .map_err(|_| SourceDeliveryError::Unavailable)?
            .get(&identity(review, thread, reply.0))
            == Some(&digest))
    }
}

#[cfg(test)]
pub(crate) fn fill_receipt_storage(
    mut state: serde_json::Value,
    review: &SourceReviewReceipt,
) -> serde_json::Value {
    use serde_json::json;
    let mut index = 1;
    loop {
        let receipt = json!([
            [review.repository, review.review_id, "x".repeat(512), index],
            hash("earlier response")
        ]);
        state["replies"].as_array_mut().unwrap().push(receipt);
        if serde_json::to_vec(&state).unwrap().len() > MAX_CHECKPOINT_BYTES - 64 {
            state["replies"].as_array_mut().unwrap().pop();
            break;
        }
        index += 1;
    }
    let remaining = MAX_CHECKPOINT_BYTES - 64 - serde_json::to_vec(&state).unwrap().len();
    let last = state["replies"].as_array_mut().unwrap().last_mut().unwrap();
    last[0][2] = json!("x".repeat(512 + remaining));
    assert_eq!(
        serde_json::to_vec(&state).unwrap().len(),
        MAX_CHECKPOINT_BYTES - 64
    );
    state
}

#[cfg(test)]
pub(crate) fn assert_fits_ledger(state: serde_json::Value) {
    use openengine_cluster_protocol::MAX_SAFE_GENERATION;
    let event = crate::v2_run_ledger::RunEvent::DeliveryState {
        execution: crate::native_v2_contract::ExecutionId::new(MAX_SAFE_GENERATION).unwrap(),
        state: crate::native_v2_runner::test_support::delivery_checkpoint(
            state,
            MAX_SAFE_GENERATION,
        ),
    };
    assert!(serde_json::to_vec(&event).unwrap().len() <= crate::v2_run_ledger::MAX_EVENT_BYTES);
}
