use super::*;

#[derive(Clone, Copy)]
pub(in crate::native_v2_cli) enum ObservationOutage {
    TransportError,
    Pending,
}

impl ObservationOutage {
    pub(super) async fn reopen<E>(self) -> Result<FakeSubscription<E>, NativeV2CliError> {
        match self {
            Self::TransportError => Err(target_transport_error()),
            Self::Pending => std::future::pending().await,
        }
    }
}

impl FakeBackend {
    pub(in crate::native_v2_cli) fn with_observation_outage(pending: bool) -> Self {
        Self {
            queued_lifecycle: true,
            reconnect_logs: true,
            attach_behavior: AttachBehavior::Disconnect,
            observation_outage: Some(if pending {
                ObservationOutage::Pending
            } else {
                ObservationOutage::TransportError
            }),
            ..Self::default()
        }
    }

    pub(in super::super) fn with_target_transport_reopen_error() -> Self {
        Self {
            queued_lifecycle: true,
            target_transport_reopen_watch: true,
            ..Self::default()
        }
    }

    pub(in super::super) fn with_permanent_reopen_watch_error() -> Self {
        Self {
            permanent_reopen_watch: true,
            ..Self::default()
        }
    }
}

pub(super) fn permanent_reopen_watch(
    backend: &FakeBackend,
    params: &RunWatchParams,
    attempt: usize,
) -> Option<Result<FakeSubscription<CliRunWatchEventNotification>, NativeV2CliError>> {
    if !backend.permanent_reopen_watch {
        return None;
    }
    if attempt > 1 {
        return Some(Err(NativeV2CliError::Protocol(
            "hosted watch authorization rejected".to_owned(),
        )));
    }
    Some(Ok(FakeSubscription::items(vec![watch_event(
        params,
        "watch-1",
        "cloud:1",
        json!({"phase":"queued"}),
    )])))
}

pub(super) fn queued_watch(
    params: &RunWatchParams,
    attempt: usize,
) -> FakeSubscription<CliRunWatchEventNotification> {
    if attempt == 1 {
        return FakeSubscription::disconnect_after(vec![watch_event(
            params,
            "watch-queued",
            "cloud:1",
            json!({"phase":"queued"}),
        )]);
    }
    FakeSubscription::items(vec![
        // A replayed boundary must not be printed twice when a target reconnects.
        watch_event(
            params,
            "watch-reopened",
            "cloud:1",
            json!({"phase":"queued"}),
        ),
        watch_event(
            params,
            "watch-admitted",
            "cloud:2",
            json!({"phase":"admitted"}),
        ),
        watch_event(
            params,
            "watch-admitted",
            "cloud:3",
            json!({
                "phase":"finished",
                "terminalResult":{"status":"succeeded","output":null},
                "metadata":{}
            }),
        ),
    ])
}

fn watch_event(
    params: &RunWatchParams,
    subscription_id: &str,
    cursor: &str,
    status: Value,
) -> CliSubscriptionItem<CliRunWatchEventNotification> {
    CliSubscriptionItem::Event(
        serde_json::from_value(json!({
            "subscriptionId":subscription_id,
            "runId":params.run_id,
            "title":"Repair checkout",
            "source":source(),
            "size":"medium",
            "cursor":cursor,
            "status":status
        }))
        .assert_value(),
    )
}
