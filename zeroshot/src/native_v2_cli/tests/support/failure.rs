use super::*;

impl FakeBackend {
    /// A runtime failure closes the watch before a terminal snapshot; status reports the failure.
    pub(in super::super) fn with_runtime_failure_before_terminal() -> Self {
        Self {
            runtime_failure_before_terminal: true,
            ..Self::default()
        }
    }

    /// The watch closes before a terminal snapshot, and the status read signals `started`, then
    /// never answers.
    pub(in super::super) fn with_unanswered_status_after_early_close(
        started: tokio::sync::watch::Sender<()>,
    ) -> Self {
        Self {
            runtime_failure_before_terminal: true,
            unanswered_status: Some(started),
            ..Self::default()
        }
    }

    pub(in super::super) fn with_failed_watch_and_error_logs(count: usize) -> Self {
        Self {
            failed_watch: true,
            error_logs: count,
            ..Self::default()
        }
    }

    pub(in crate::native_v2_cli) fn with_pending_logs() -> Self {
        Self {
            pending_logs: true,
            ..Self::default()
        }
    }
}

pub(super) fn runtime_failure_watch(
    params: &RunWatchParams,
) -> FakeSubscription<CliRunWatchEventNotification> {
    FakeSubscription::items(vec![
        CliSubscriptionItem::Event(
            serde_json::from_value(json!({
                "subscriptionId":"watch-1",
                "runId":params.run_id,
                "title":"Repair checkout",
                "source":source(),
                "size":"medium",
                "cursor":"v2:2",
                "status":{"phase":"running","activeExecutions":[]}
            }))
            .assert_value(),
        ),
        CliSubscriptionItem::Closed {
            reason: SubscriptionCloseReason::Done,
        },
    ])
}

pub(super) fn runtime_failure_status(run_id: &RunId) -> CliRunStatusResult {
    serde_json::from_value(json!({
        "runId":run_id,
        "title":"Repair checkout",
        "source":source(),
        "size":"medium",
        "atCursor":"v2:3",
        "status":{
            "phase":"finished",
            "terminalResult":{"status":"failed","reason":"runtime_failed"},
            "metadata":{}
        }
    }))
    .assert_value()
}

/// Interleaves `count` numbered error records with progress records. Like a local controller that
/// exits after the terminal record, the first stream ends halfway; the reconnect replays from its
/// last cursor, including that boundary record.
pub(super) fn error_logs(
    params: &RunLogsParams,
    count: usize,
) -> FakeSubscription<RunLogEventNotification> {
    let mut events = Vec::<RunLogEventNotification>::new();
    for index in 1..=count {
        for (sequence, level, message) in [
            (2 * index, "info", format!("progress {index}")),
            (2 * index + 1, "error", format!("error {index}")),
        ] {
            events.push(
                serde_json::from_value(json!({
                    "subscriptionId":"logs-1",
                    "runId":params.run_id,
                    "cursor":format!("v2:{sequence}"),
                    "timestamp":1_725_000_000_123_u64,
                    "record":{"level":level,"target":"agent","message":message}
                }))
                .assert_value(),
            );
        }
    }
    let resumed = params
        .from_cursor
        .as_ref()
        .and_then(|cursor| events.iter().position(|event| &event.cursor == cursor));
    let (range, closed) = match resumed {
        None => (0..events.len() / 2, None),
        Some(boundary) => (
            boundary..events.len(),
            Some(CliSubscriptionItem::Closed {
                reason: SubscriptionCloseReason::Done,
            }),
        ),
    };
    let items = events[range]
        .iter()
        .cloned()
        .map(CliSubscriptionItem::Event)
        .chain(closed)
        .collect();
    FakeSubscription::items(items)
}
