use std::fmt::Debug;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{JsonRpcRequest, RequestId, RunId, RunWatchParams, SubscriptionId};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::*;
use crate::{JsonRpcTransport, PumpedSubscription, TransportError};

fn checked_result<T, E: Debug>(result: Result<T, E>) -> T {
    let mut values = result.into_iter().collect::<Vec<_>>();
    assert_eq!(values.len(), 1, "expected a successful result");
    values.swap_remove(0)
}

fn checked_option<T>(value: Option<T>) -> T {
    let mut values = value.into_iter().collect::<Vec<_>>();
    assert_eq!(values.len(), 1, "expected a value");
    values.swap_remove(0)
}

fn checked_error<T, E>(result: Result<T, E>) -> E {
    assert!(result.is_err(), "expected an error");
    let mut errors = result.err().into_iter().collect::<Vec<_>>();
    errors.swap_remove(0)
}

struct ScriptedSubscriptionTransport {
    result: Value,
    notifications: Vec<String>,
    provide_stream: bool,
    expected_method: &'static str,
    next_id: AtomicI64,
}

impl ScriptedSubscriptionTransport {
    fn new(
        expected_method: &'static str,
        result: Value,
        notifications: Vec<Value>,
        provide_stream: bool,
    ) -> Self {
        Self {
            result,
            notifications: notifications
                .into_iter()
                .map(|value| value.to_string())
                .collect(),
            provide_stream,
            expected_method,
            next_id: AtomicI64::new(1),
        }
    }
}

#[async_trait]
impl JsonRpcTransport for ScriptedSubscriptionTransport {
    async fn request(&self, _request: String) -> Result<String, TransportError> {
        Err(TransportError::Protocol(
            "subscription tests use open_subscription".to_owned(),
        ))
    }
}

#[async_trait]
impl SubscriptionTransport for ScriptedSubscriptionTransport {
    async fn open_subscription(
        &self,
        request: String,
        id: RequestId,
    ) -> Result<(String, Option<PumpedSubscription>), TransportError> {
        let request: JsonRpcRequest<Value> = checked_result(serde_json::from_str(&request));
        assert_eq!(request.id, id);
        assert_eq!(request.method, self.expected_method);
        let response = json!({"jsonrpc":"2.0","id":id,"result":self.result}).to_string();
        if !self.provide_stream {
            return Ok((response, None));
        }
        let (sender, receiver) = mpsc::channel(self.notifications.len().max(1));
        for notification in &self.notifications {
            checked_result(sender.try_send(notification.clone()));
        }
        drop(sender);
        Ok((
            response,
            Some(PumpedSubscription {
                receiver,
                overflowed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }),
        ))
    }

    async fn cancel_subscription(&self, _id: SubscriptionId) -> Result<(), TransportError> {
        Ok(())
    }

    async fn cancel_request(&self, _id: RequestId) -> Result<(), TransportError> {
        Ok(())
    }

    fn next_watch_request_id(&self) -> RequestId {
        RequestId::Integer(self.next_id.fetch_add(1, Ordering::Relaxed))
    }
}

fn watch_result() -> Value {
    json!({"subscriptionId":"watch-1","runId":"run-1","atCursor":"v2:7"})
}

#[tokio::test]
async fn successful_subscription_without_a_stream_is_an_invalid_response() {
    let transport =
        ScriptedSubscriptionTransport::new(RUN_WATCH_METHOD, watch_result(), vec![], false);
    let error = checked_error(
        RunSubscriptionClient::new(&transport)
            .run_watch(RunWatchParams {
                run_id: RunId::new("run-1"),
                from_cursor: None,
            })
            .await,
    );
    assert!(matches!(error, ClientError::InvalidResponse(_)));
}

#[tokio::test]
async fn close_retains_the_local_cursor_and_is_terminal() {
    let transport = ScriptedSubscriptionTransport::new(
        RUN_WATCH_METHOD,
        watch_result(),
        vec![
            json!({
                "jsonrpc":"2.0","method":"event","params":{
                    "subscriptionId":"watch-1","runId":"run-1","cursor":"v2:8",
                    "title":"Protocol client test",
                    "source":{
                        "repository":"open-engine/zeroshot",
                        "branch":"main",
                        "revision":"0123456789abcdef0123456789abcdef01234567"
                    },
                    "size":"small",
                    "status":{"phase":"running","activeExecutions":[]}
                }
            }),
            json!({
                "jsonrpc":"2.0","method":"subscription/closed","params":{
                    "subscriptionId":"watch-1","reason":"done"
                }
            }),
            json!({
                "jsonrpc":"2.0","method":"event","params":{
                    "subscriptionId":"watch-1","runId":"run-1","cursor":"v2:9",
                    "title":"Protocol client test",
                    "source":{
                        "repository":"open-engine/zeroshot",
                        "branch":"main",
                        "revision":"0123456789abcdef0123456789abcdef01234567"
                    },
                    "size":"small",
                    "status":{"phase":"running","activeExecutions":[]}
                }
            }),
        ],
        true,
    );
    let (_, mut stream) = checked_result(
        RunSubscriptionClient::new(&transport)
            .run_watch(RunWatchParams {
                run_id: RunId::new("run-1"),
                from_cursor: None,
            })
            .await,
    );

    assert!(matches!(
        checked_result(checked_option(stream.next().await)),
        RunSubscriptionEvent::Event(_)
    ));
    assert_eq!(
        checked_result(checked_option(stream.next().await)),
        RunSubscriptionEvent::Closed {
            reason: SubscriptionCloseReason::Done,
            last_delivered_cursor: Some(Cursor::new("v2:8")),
        }
    );
    assert!(stream.next().await.is_none());
    assert_eq!(stream.last_delivered_cursor(), Some(&Cursor::new("v2:8")));
}

#[tokio::test]
async fn logs_and_attach_use_their_run_scoped_method_names() {
    let logs = ScriptedSubscriptionTransport::new(
        RUN_LOGS_METHOD,
        json!({"subscriptionId":"logs-1","runId":"run-1","atCursor":"v2:1"}),
        vec![],
        true,
    );
    checked_result(
        RunSubscriptionClient::new(&logs)
            .run_logs(RunLogsParams {
                run_id: RunId::new("run-1"),
                from_cursor: None,
                execution: None,
            })
            .await,
    );

    let attach = ScriptedSubscriptionTransport::new(
        RUN_ATTACH_METHOD,
        json!({
            "subscriptionId":"attach-1","runId":"run-1","execution":"execution-1"
        }),
        vec![],
        true,
    );
    checked_result(
        RunSubscriptionClient::new(&attach)
            .run_attach(RunAttachParams {
                run_id: RunId::new("run-1"),
                execution: checked_result(openengine_cluster_protocol::ExecutionRef::new(
                    "execution-1",
                )),
            })
            .await,
    );
}
