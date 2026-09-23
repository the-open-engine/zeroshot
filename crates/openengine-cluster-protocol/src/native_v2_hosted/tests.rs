use serde_json::json;

use super::{HostedRunStatus, HostedRunStreamFrame};
use crate::{RunStatus, SubscriptionCloseReason};

#[test]
fn oecp_status_conversion_never_synthesizes_queued() {
    let queued = HostedRunStatus::queued();
    assert!(queued.as_target().is_none());
    for value in [
        json!({"phase":"admitted"}),
        json!({"phase":"running","activeExecutions":[]}),
        json!({"phase":"stopping","activeExecutions":[]}),
        json!({
            "phase":"finished",
            "terminalResult":{"status":"succeeded","output":null},
            "metadata":{}
        }),
    ] {
        let oecp = serde_json::from_value::<RunStatus>(value);
        assert!(oecp.is_ok());
        let Ok(oecp) = oecp else {
            return;
        };
        let hosted = HostedRunStatus::target(oecp);
        assert!(hosted.as_target().is_some());
        assert!(!matches!(hosted, HostedRunStatus::Queued(_)));
    }
}

#[test]
fn closed_stream_frame_has_stable_ndjson_shape() {
    let frame = HostedRunStreamFrame::<serde_json::Value>::Closed {
        reason: SubscriptionCloseReason::Done,
    };
    let serialized = serde_json::to_value(frame);
    assert!(
        serialized
            .as_ref()
            .is_ok_and(|value| value == &json!({"type":"closed","reason":"done"}))
    );
}
