use std::sync::atomic::{AtomicBool, Ordering};

use openengine_cluster_protocol::{Cursor, RequestId, SubscriptionCloseReason, SubscriptionId};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::json;

use super::*;

#[tokio::test]
async fn pumped_line_reports_buffered_data_one_overflow_and_then_clean_end() {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let overflowed = AtomicBool::new(false);
    sender.send("frame".to_owned()).await.assert_value();
    assert!(matches!(
        next_pumped_line(&mut receiver, &overflowed).await,
        PumpedLine::Frame(line) if line == "frame"
    ));

    overflowed.store(true, Ordering::Release);
    drop(sender);
    assert!(matches!(
        next_pumped_line(&mut receiver, &overflowed).await,
        PumpedLine::SlowConsumer
    ));
    assert!(!overflowed.load(Ordering::Acquire));
    assert!(matches!(
        next_pumped_line(&mut receiver, &overflowed).await,
        PumpedLine::End
    ));
}

#[test]
fn subscription_response_and_notification_parsers_fail_closed_on_identity_and_shape() {
    let request_id = RequestId::String("request-1".to_owned());
    let result: serde_json::Value = parse_subscription_response(
        &json!({"jsonrpc":"2.0","id":"request-1","result":{"accepted":true}}).to_string(),
        &request_id,
    )
    .assert_value();
    assert_eq!(result, json!({"accepted": true}));

    let rpc: Result<serde_json::Value, _> = parse_subscription_response(
        &json!({"jsonrpc":"2.0","id":"request-1",
            "error":{"code":-32000,"message":"refused"}})
        .to_string(),
        &request_id,
    );
    assert!(matches!(rpc.assert_error(), crate::ClientError::Rpc(_)));
    for malformed in [
        "not-json".to_owned(),
        json!({"jsonrpc":"2.0","id":"other","result":{}}).to_string(),
    ] {
        let parsed: Result<serde_json::Value, _> =
            parse_subscription_response(&malformed, &request_id);
        assert!(matches!(
            parsed.assert_error(),
            crate::ClientError::InvalidResponse(_)
        ));
    }

    assert!(parse_subscription_notification("not-json").is_err());
    assert_eq!(
        parse_subscription_notification(r#"{"jsonrpc":"2.0","params":{}}"#)
            .assert_value()
            .0,
        None
    );
}

#[test]
fn close_parser_returns_the_durable_cursor_and_rejects_cross_subscription_close() {
    let expected = SubscriptionId::new("subscription-1");
    let close = json!({
        "jsonrpc":"2.0",
        "method":"subscription/closed",
        "params":{
            "subscriptionId":"subscription-1",
            "reason":"done",
            "lastDeliveredCursor":"cursor-7"
        }
    });
    assert_eq!(
        parse_subscription_close(close.clone(), &expected).assert_value(),
        (SubscriptionCloseReason::Done, Some(Cursor::new("cursor-7")))
    );

    let mut wrong = close;
    wrong["params"]["subscriptionId"] = json!("subscription-2");
    let error = parse_subscription_close(wrong, &expected).assert_error();
    assert!(error.to_string().contains("subscription id mismatch"));
}
