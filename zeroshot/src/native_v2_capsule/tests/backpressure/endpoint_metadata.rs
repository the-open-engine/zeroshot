use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use openengine_cluster_protocol::{RunId, TokenCount};
use tokio::sync::Notify;

use super::super::request;
use crate::native_v2_capsule::{
    CapsuleNodeChannel, CapsuleNodeEvent, CapsuleNodeFailure, NativeCapsuleNodeEndpoint,
    CapsuleConnectionError, CapsuleExecutionStream, CapsuleOutput, CapsuleOutputStream,
    RemoteCapsuleNodeRunner,
};
use crate::native_v2_contract::TokenUsageDelta;
use crate::native_v2_runner::{
    DURABLE_OUTPUT_CAPACITY, NodeHandle, NodeRunRequest, NodeRunner, NodeRunnerError,
    remote_node_handle,
};
use openengine_cluster_testkit::assertions::AssertValue;

const KNOWN_USAGE_RECORDS: usize = DURABLE_OUTPUT_CAPACITY * 3 + 17;

fn unit_usage() -> TokenUsageDelta {
    TokenUsageDelta {
        input_tokens: TokenCount::new(1).assert_value(),
        output_tokens: TokenCount::new(2).assert_value(),
        cache_read_input_tokens: None,
        cache_creation_input_tokens: None,
    }
}

#[derive(Default)]
struct UsageBurstRunner {
    saturated: Arc<Notify>,
    cleaned: Arc<AtomicBool>,
}

#[async_trait]
impl NodeRunner for UsageBurstRunner {
    async fn start(&self, request: NodeRunRequest) -> Result<NodeHandle, NodeRunnerError> {
        let (handle, bridge) = remote_node_handle(request.invocation.reference);
        let saturated = self.saturated.clone();
        let cleaned = self.cleaned.clone();
        tokio::spawn(async move {
            let usage = unit_usage();
            for index in 0..KNOWN_USAGE_RECORDS {
                if index == DURABLE_OUTPUT_CAPACITY * 2 + 1 {
                    saturated.notify_one();
                }
                bridge.record_token_usage(Some(usage)).await.assert_value();
                tokio::task::yield_now().await;
            }
            bridge.record_token_usage(None).await.assert_value();
            cleaned.store(true, Ordering::SeqCst);
            let completion = Err(NodeRunnerError::Cancelled);
            bridge.finish(completion);
        });
        Ok(handle)
    }

    async fn close_run(&self, _: &RunId) {}
}

async fn wait_for_usage_saturation(runner: &UsageBurstRunner) {
    tokio::time::timeout(Duration::from_secs(5), runner.saturated.notified())
        .await
        .assert_value();
}

#[tokio::test]
async fn unread_stream_compacts_post_cancellation_usage_without_losing_totals() {
    let runner = Arc::new(UsageBurstRunner::default());
    let endpoint = NativeCapsuleNodeEndpoint::new(runner.clone());
    let run_id = RunId::new("bounded-cancelled-metadata");
    let mut stream = endpoint
        .start(request(run_id.as_str(), 1))
        .await
        .assert_value();

    wait_for_usage_saturation(&runner).await;
    tokio::time::timeout(Duration::from_secs(5), endpoint.close_run(&run_id))
        .await
        .assert_value()
        .assert_value();
    assert!(runner.cleaned.load(Ordering::SeqCst));

    let mut known_events = 0;
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut incomplete_events = 0;
    let mut cancelled = false;
    let mut unexpected = Vec::new();
    while let Some(event) = stream.recv().await {
        match event {
            CapsuleNodeEvent::TokenUsage { usage: Some(usage) } => {
                known_events += 1;
                input_tokens += usage.input_tokens.get();
                output_tokens += usage.output_tokens.get();
            }
            CapsuleNodeEvent::TokenUsage { usage: None } => incomplete_events += 1,
            CapsuleNodeEvent::Failed {
                failure: CapsuleNodeFailure::Cancelled,
            } => cancelled = true,
            event => unexpected.push(event),
        }
    }

    assert_eq!(known_events, DURABLE_OUTPUT_CAPACITY + 1);
    assert_eq!(input_tokens, KNOWN_USAGE_RECORDS as u64);
    assert_eq!(output_tokens, (KNOWN_USAGE_RECORDS * 2) as u64);
    assert_eq!(incomplete_events, 1);
    assert!(cancelled);
    assert!(unexpected.is_empty(), "unexpected events: {unexpected:?}");
}

#[tokio::test]
async fn proxy_terminal_lane_overflow_is_sticky_and_marks_usage_incomplete() {
    let runner = Arc::new(UsageBurstRunner::default());
    let endpoint = Arc::new(NativeCapsuleNodeEndpoint::new(runner.clone()));
    let proxy = RemoteCapsuleNodeRunner::new(endpoint);
    let run_id = RunId::new("proxy-terminal-overflow");
    let mut handle = proxy
        .start(request(run_id.as_str(), 1))
        .await
        .assert_value();
    let mut durable = handle.take_initial_output().assert_value();

    wait_for_usage_saturation(&runner).await;
    tokio::time::timeout(Duration::from_secs(5), proxy.close_run(&run_id))
        .await
        .assert_value();
    assert!(runner.cleaned.load(Ordering::SeqCst));
    assert!(matches!(
        handle.completion().await,
        Err(NodeRunnerError::DriverDetail(_))
    ));

    let mut known_events = 0;
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut incomplete_events = 0;
    let mut unexpected_outputs = 0;
    while let Ok(event) = durable.recv().await {
        match event {
            crate::native_v2_runner::DurableNodeEvent::TokenUsage(Some(usage)) => {
                known_events += 1;
                input_tokens += usage.input_tokens.get();
                output_tokens += usage.output_tokens.get();
            }
            crate::native_v2_runner::DurableNodeEvent::TokenUsage(None) => {
                incomplete_events += 1;
            }
            crate::native_v2_runner::DurableNodeEvent::Output { .. } => unexpected_outputs += 1,
        }
    }

    assert_eq!(known_events, DURABLE_OUTPUT_CAPACITY * 2);
    assert_eq!(input_tokens, known_events as u64);
    assert_eq!(output_tokens, (known_events * 2) as u64);
    assert_eq!(incomplete_events, 1);
    assert_eq!(unexpected_outputs, 0);
}

struct ErrorBurstChannel {
    cancelled: tokio::sync::watch::Sender<bool>,
    loss: tokio::sync::watch::Sender<bool>,
    cancel_calls: AtomicUsize,
    started: Notify,
    overflow_seen: Arc<Notify>,
    release_cleanup: Arc<Notify>,
}

#[async_trait]
impl CapsuleNodeChannel for ErrorBurstChannel {
    async fn start(
        &self,
        _request: NodeRunRequest,
    ) -> Result<CapsuleExecutionStream, CapsuleConnectionError> {
        let (events, receiver) = tokio::sync::mpsc::channel(DURABLE_OUTPUT_CAPACITY);
        let mut cancelled = self.cancelled.subscribe();
        let release = self.release_cleanup.clone();
        tokio::spawn(async move {
            while !*cancelled.borrow_and_update() {
                cancelled.changed().await.assert_value();
            }
            for index in 0..=DURABLE_OUTPUT_CAPACITY * 2 {
                events
                    .send(CapsuleNodeEvent::Output {
                        output: CapsuleOutput {
                            stream: CapsuleOutputStream::Error,
                            text: format!("error {index}"),
                        },
                        timestamp: openengine_cluster_protocol::UnixTimestampMillis::new(1)
                            .assert_value(),
                    })
                    .await
                    .assert_value();
            }
            release.notified().await;
            events
                .send(CapsuleNodeEvent::Failed {
                    failure: CapsuleNodeFailure::CleanupUnconfirmed,
                })
                .await
                .assert_value();
        });
        self.started.notify_one();
        Ok(CapsuleExecutionStream::from_receiver(receiver))
    }

    async fn cancel(
        &self,
        _reference: &crate::native_v2_contract::ExecutionRef,
    ) -> Result<(), CapsuleConnectionError> {
        self.cancelled.send_replace(true);
        if self.cancel_calls.fetch_add(1, Ordering::SeqCst) == 1 {
            self.overflow_seen.notify_one();
        }
        Ok(())
    }

    async fn close_run(&self, _run_id: &RunId) -> Result<(), CapsuleConnectionError> {
        self.cancelled.send_replace(true);
        self.release_cleanup.notify_one();
        Ok(())
    }

    fn connection_loss(&self) -> tokio::sync::watch::Receiver<bool> {
        self.loss.subscribe()
    }
}

#[tokio::test]
async fn remote_error_overflow_waits_for_cleanup_and_preserves_cleanup_failure() {
    let (cancelled, _) = tokio::sync::watch::channel(false);
    let (loss, _) = tokio::sync::watch::channel(false);
    let channel = Arc::new(ErrorBurstChannel {
        cancelled,
        loss,
        cancel_calls: AtomicUsize::new(0),
        started: Notify::new(),
        overflow_seen: Arc::new(Notify::new()),
        release_cleanup: Arc::new(Notify::new()),
    });
    let proxy = RemoteCapsuleNodeRunner::new(channel.clone());
    let mut handle = proxy
        .start(request("error-overflow", 1))
        .await
        .assert_value();
    let _durable = handle.take_initial_output().assert_value();
    tokio::time::timeout(Duration::from_secs(5), channel.started.notified())
        .await
        .assert_value();
    handle.cancel();
    tokio::time::timeout(Duration::from_secs(5), channel.overflow_seen.notified())
        .await
        .assert_value();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), handle.completion())
            .await
            .is_err(),
        "overflow must not settle an execution whose capsule cleanup is still blocked"
    );
    channel.release_cleanup.notify_one();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), handle.completion())
            .await
            .assert_value(),
        Err(NodeRunnerError::CleanupUnconfirmed)
    );
}
