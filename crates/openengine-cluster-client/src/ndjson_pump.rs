//! Non-blocking notification routing for the shared NDJSON response pump.

use std::sync::atomic::Ordering;

use openengine_cluster_protocol::SubscriptionId;
use serde_json::Value;
use tokio::sync::mpsc;

use super::SubscriptionMap;

/// Forwards one `event`/`subscription/closed` notification without waiting on a consumer.
/// Returns the subscription id when the local receiver is full or gone and the server must be
/// cancelled. A full receiver retains its buffered events; once drained, the stream emits one
/// local `SLOW_CONSUMER` close from its exact last caller-delivered cursor.
pub(super) fn forward_notification(
    value: &Value,
    line: String,
    subscriptions: &SubscriptionMap,
) -> Option<SubscriptionId> {
    let subscription_id = value
        .get("params")
        .and_then(|params| params.get("subscriptionId"))
        .and_then(Value::as_str)?;
    let subscription_id = SubscriptionId::new(subscription_id);
    let terminal = value.get("method").and_then(Value::as_str) == Some("subscription/closed");
    let registration = subscriptions.lock().get(&subscription_id).cloned()?;

    match registration.sender.try_send(line) {
        Ok(()) => {
            if terminal {
                subscriptions.lock().remove(&subscription_id);
            }
            None
        }
        Err(mpsc::error::TrySendError::Full(_)) => {
            registration.overflowed.store(true, Ordering::Release);
            subscriptions.lock().remove(&subscription_id);
            (!terminal).then_some(subscription_id)
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            subscriptions.lock().remove(&subscription_id);
            (!terminal).then_some(subscription_id)
        }
    }
}
