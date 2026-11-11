//! Shared receipt-capacity fixtures stay outside production coverage.
use super::{MAX_CHECKPOINT_BYTES, hash};
use crate::source_code_provider::SourceReviewReceipt;

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
