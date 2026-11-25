//! Transport-generic native-v2 run subscription client.

use openengine_cluster_protocol::{
    Cursor, JsonRpcNotification, RunAttachEventNotification, RunAttachParams, RunAttachResult,
    RunLogEventNotification, RunLogsParams, RunLogsResult, RunWatchEventNotification,
    RunWatchParams, RunWatchResult, SubscriptionCloseReason, SubscriptionId, RUN_ATTACH_METHOD,
    RUN_LOGS_METHOD, RUN_WATCH_METHOD,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use crate::ndjson_subscription::{
    impl_cursor_subscription_controls, open_subscription, parse_subscription_close,
    parse_subscription_notification, PumpedLine, SubscriptionClientCore, SubscriptionStreamCore,
};
use crate::{ClientError, NdjsonTransport, SubscriptionTransport};

#[derive(Clone, Debug, PartialEq)]
pub enum RunSubscriptionEvent<E> {
    Event(E),
    Closed {
        reason: SubscriptionCloseReason,
        last_delivered_cursor: Option<Cursor>,
    },
}

pub struct RunSubscriptionClient<'a, T> {
    core: SubscriptionClientCore<'a, T>,
}

pub type NdjsonRunSubscriptionClient<'a, R, W> = RunSubscriptionClient<'a, NdjsonTransport<R, W>>;

struct RunSubscriptionShape<R, E> {
    result_subscription_id: fn(&R) -> &SubscriptionId,
    event_subscription_id: fn(&E) -> &SubscriptionId,
    event_cursor: fn(&E) -> Option<&Cursor>,
}

impl<'a, T> RunSubscriptionClient<'a, T>
where
    T: SubscriptionTransport,
{
    #[must_use]
    pub const fn new(transport: &'a T) -> Self {
        Self {
            core: SubscriptionClientCore::new(transport),
        }
    }

    pub async fn run_watch(
        &self,
        params: RunWatchParams,
    ) -> Result<
        (
            RunWatchResult,
            RunSubscriptionEventStream<'a, T, RunWatchEventNotification>,
        ),
        ClientError,
    > {
        self.open(
            RUN_WATCH_METHOD,
            params,
            RunSubscriptionShape {
                result_subscription_id: watch_subscription_id,
                event_subscription_id: watch_event_subscription_id,
                event_cursor: watch_event_cursor,
            },
        )
        .await
    }

    pub async fn run_logs(
        &self,
        params: RunLogsParams,
    ) -> Result<
        (
            RunLogsResult,
            RunSubscriptionEventStream<'a, T, RunLogEventNotification>,
        ),
        ClientError,
    > {
        self.open(
            RUN_LOGS_METHOD,
            params,
            RunSubscriptionShape {
                result_subscription_id: logs_subscription_id,
                event_subscription_id: log_event_subscription_id,
                event_cursor: log_event_cursor,
            },
        )
        .await
    }

    pub async fn run_attach(
        &self,
        params: RunAttachParams,
    ) -> Result<
        (
            RunAttachResult,
            RunSubscriptionEventStream<'a, T, RunAttachEventNotification>,
        ),
        ClientError,
    > {
        self.open(
            RUN_ATTACH_METHOD,
            params,
            RunSubscriptionShape {
                result_subscription_id: attach_subscription_id,
                event_subscription_id: attach_event_subscription_id,
                event_cursor: no_event_cursor,
            },
        )
        .await
    }

    async fn open<P, R, E>(
        &self,
        method: &str,
        params: P,
        shape: RunSubscriptionShape<R, E>,
    ) -> Result<(R, RunSubscriptionEventStream<'a, T, E>), ClientError>
    where
        P: Serialize + Send,
        R: DeserializeOwned,
        E: DeserializeOwned,
    {
        let (result, subscription) =
            open_subscription(self.core.transport(), method, params).await?;
        let subscription_id = (shape.result_subscription_id)(&result).clone();
        let subscription = subscription.ok_or_else(|| {
            ClientError::InvalidResponse(
                "successful native-v2 subscription response had no notification stream".to_owned(),
            )
        })?;
        Ok((
            result,
            RunSubscriptionEventStream {
                core: SubscriptionStreamCore::new(
                    self.core.transport(),
                    subscription,
                    subscription_id,
                ),
                event_subscription_id: shape.event_subscription_id,
                event_cursor: shape.event_cursor,
                closed: false,
            },
        ))
    }
}

pub struct RunSubscriptionEventStream<'a, T, E> {
    core: SubscriptionStreamCore<'a, T>,
    event_subscription_id: fn(&E) -> &SubscriptionId,
    event_cursor: fn(&E) -> Option<&Cursor>,
    closed: bool,
}

impl<'a, T, E> RunSubscriptionEventStream<'a, T, E>
where
    T: SubscriptionTransport,
    E: DeserializeOwned,
{
    pub async fn next(&mut self) -> Option<Result<RunSubscriptionEvent<E>, ClientError>> {
        if self.closed {
            return None;
        }
        let line = match self.core.next_line().await {
            PumpedLine::Frame(line) => line,
            PumpedLine::SlowConsumer => {
                self.closed = true;
                return Some(Ok(RunSubscriptionEvent::Closed {
                    reason: SubscriptionCloseReason::SlowConsumer,
                    last_delivered_cursor: self.core.last_delivered_cursor().cloned(),
                }));
            }
            PumpedLine::End => return None,
        };
        Some(self.parse_notification(&line))
    }

    fn parse_notification(&mut self, line: &str) -> Result<RunSubscriptionEvent<E>, ClientError> {
        let (method, value) = parse_subscription_notification(line)?;
        match method.as_deref() {
            Some("event") => {
                let notification: JsonRpcNotification<E> = serde_json::from_value(value)
                    .map_err(|error| ClientError::InvalidResponse(error.to_string()))?;
                let event = notification.params;
                if (self.event_subscription_id)(&event) != self.core.subscription_id() {
                    return Err(ClientError::InvalidResponse(
                        "native-v2 event subscription id mismatch".to_owned(),
                    ));
                }
                if let Some(cursor) = (self.event_cursor)(&event) {
                    self.core.record_delivered_cursor(cursor.clone());
                }
                Ok(RunSubscriptionEvent::Event(event))
            }
            Some("subscription/closed") => {
                let (reason, observed_cursor) =
                    parse_subscription_close(value, self.core.subscription_id())?;
                let last_delivered_cursor =
                    observed_cursor.or_else(|| self.core.last_delivered_cursor().cloned());
                if let Some(cursor) = &last_delivered_cursor {
                    self.core.record_delivered_cursor(cursor.clone());
                }
                self.closed = true;
                Ok(RunSubscriptionEvent::Closed {
                    reason,
                    last_delivered_cursor,
                })
            }
            other => Err(ClientError::InvalidResponse(format!(
                "unexpected subscription notification method {other:?}"
            ))),
        }
    }

    impl_cursor_subscription_controls!();
}

fn watch_subscription_id(result: &RunWatchResult) -> &SubscriptionId {
    &result.subscription_id
}

fn logs_subscription_id(result: &RunLogsResult) -> &SubscriptionId {
    &result.subscription_id
}

fn attach_subscription_id(result: &RunAttachResult) -> &SubscriptionId {
    &result.subscription_id
}

fn watch_event_subscription_id(event: &RunWatchEventNotification) -> &SubscriptionId {
    &event.subscription_id
}

fn log_event_subscription_id(event: &RunLogEventNotification) -> &SubscriptionId {
    &event.subscription_id
}

fn attach_event_subscription_id(event: &RunAttachEventNotification) -> &SubscriptionId {
    &event.subscription_id
}

fn watch_event_cursor(event: &RunWatchEventNotification) -> Option<&Cursor> {
    Some(&event.cursor)
}

fn log_event_cursor(event: &RunLogEventNotification) -> Option<&Cursor> {
    Some(&event.cursor)
}

fn no_event_cursor(_event: &RunAttachEventNotification) -> Option<&Cursor> {
    None
}

#[cfg(test)]
#[path = "native_v2/tests.rs"]
mod tests;
