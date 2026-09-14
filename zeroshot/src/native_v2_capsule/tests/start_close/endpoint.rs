use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use openengine_cluster_protocol::RunId;
use tokio::sync::Notify;

use super::{Gate, token_usage, within_one_second};
use super::super::request;
use super::super::super::*;
use crate::native_v2_runner::remote_node_handle;
use openengine_cluster_testkit::assertions::AssertValue;

#[derive(Default)]
pub(super) struct DelayedRunner {
    pub(super) start: Gate,
    close: Gate,
    pub(super) ready: Notify,
    pub(super) cancelled: Arc<Notify>,
    pub(super) finish: Arc<Notify>,
    dropped_start: Arc<Notify>,
    start_calls: AtomicUsize,
    pub(super) work_started: AtomicUsize,
}

struct PendingStart(Arc<Notify>);

impl Drop for PendingStart {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

#[async_trait]
impl NodeRunner for DelayedRunner {
    async fn start(&self, request: NodeRunRequest) -> Result<NodeHandle, NodeRunnerError> {
        self.start_calls.fetch_add(1, Ordering::SeqCst);
        let _pending = PendingStart(self.dropped_start.clone());
        self.start.enter().await;
        self.work_started.fetch_add(1, Ordering::SeqCst);
        let (handle, mut bridge) = remote_node_handle(request.invocation.reference);
        let cancelled = self.cancelled.clone();
        let finish = self.finish.clone();
        tokio::spawn(async move {
            bridge.cancelled().await;
            bridge
                .record_token_usage(Some(token_usage()))
                .await
                .assert_value();
            cancelled.notify_one();
            finish.notified().await;
            bridge.finish(Err(NodeRunnerError::Cancelled));
        });
        self.ready.notify_one();
        Ok(handle)
    }

    async fn close_run(&self, _run_id: &RunId) {
        self.close.enter().await;
    }
}

pub(super) async fn ready_endpoint(
    run_id: &str,
) -> (
    Arc<DelayedRunner>,
    Arc<NativeCapsuleNodeEndpoint>,
    CapsuleExecutionStream,
) {
    let runner = Arc::new(DelayedRunner::default());
    let endpoint = Arc::new(NativeCapsuleNodeEndpoint::new(runner.clone()));
    let stream = within_one_second(endpoint.start(request(run_id, 1)))
        .await
        .assert_value();
    runner.start.wait().await;
    runner.start.open();
    within_one_second(runner.ready.notified()).await;
    (runner, endpoint, stream)
}

async fn assert_cancelled_stream(stream: &mut CapsuleExecutionStream, usage: bool) {
    if usage {
        assert!(matches!(within_one_second(stream.recv()).await,
            Some(CapsuleNodeEvent::TokenUsage { usage: Some(value) }) if value == token_usage()));
    }
    assert!(matches!(
        within_one_second(stream.recv()).await,
        Some(CapsuleNodeEvent::Failed {
            failure: CapsuleNodeFailure::Cancelled
        })
    ));
    assert!(within_one_second(stream.recv()).await.is_none());
}

#[tokio::test]
async fn endpoint_reservation_can_cancel_never_ready_start_without_closing_siblings() {
    let runner = Arc::new(DelayedRunner::default());
    let endpoint = NativeCapsuleNodeEndpoint::new(runner.clone());
    let run = request("endpoint-pending", 1);
    let reference = run.invocation.reference.clone();
    let mut stream = within_one_second(endpoint.start(run)).await.assert_value();
    runner.start.wait().await;
    assert!(matches!(
        endpoint.start(request("endpoint-pending", 1)).await,
        Err(CapsuleConnectionError::Rejected(
            CapsuleNodeFailure::ExecutionActive
        ))
    ));
    endpoint.cancel(&reference).await.assert_value();
    assert_cancelled_stream(&mut stream, false).await;
    within_one_second(runner.dropped_start.notified()).await;
    assert_eq!(runner.work_started.load(Ordering::SeqCst), 0);
    let sibling = request("endpoint-pending", 2);
    let reference = sibling.invocation.reference.clone();
    let mut sibling = endpoint.start(sibling).await.assert_value();
    runner.start.wait().await;
    endpoint.cancel(&reference).await.assert_value();
    assert_cancelled_stream(&mut sibling, false).await;
    assert_eq!(runner.start_calls.load(Ordering::SeqCst), 2);
    assert_eq!(runner.work_started.load(Ordering::SeqCst), 0);
    assert!(!*endpoint.connection_loss().borrow());
}

#[tokio::test]
async fn dropping_reserved_endpoint_stream_cancels_never_ready_start() {
    let runner = Arc::new(DelayedRunner::default());
    let endpoint = NativeCapsuleNodeEndpoint::new(runner.clone());
    let stream = endpoint
        .start(request("endpoint-drop-pending", 1))
        .await
        .assert_value();
    runner.start.wait().await;
    drop(stream);
    within_one_second(runner.dropped_start.notified()).await;
    runner.start.open();
    tokio::task::yield_now().await;
    assert_eq!(runner.work_started.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn repeated_endpoint_cancellation_preserves_usage_and_waits_for_cleanup() {
    let (runner, endpoint, mut stream) = ready_endpoint("endpoint-cancel").await;
    let reference = request("endpoint-cancel", 1).invocation.reference;
    for _ in 0..4096 {
        endpoint.cancel(&reference).await.assert_value();
    }
    within_one_second(runner.cancelled.notified()).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), stream.recv())
            .await
            .is_err(),
        "terminal metadata must not acknowledge incomplete provider cleanup"
    );
    runner.finish.notify_one();
    assert_cancelled_stream(&mut stream, true).await;
}

#[tokio::test]
async fn endpoint_close_waits_for_provider_cleanup_and_preserves_usage() {
    let (runner, endpoint, mut stream) = ready_endpoint("endpoint-close").await;
    let closing_endpoint = endpoint.clone();
    let close = tokio::spawn(async move {
        closing_endpoint
            .close_run(&RunId::new("endpoint-close"))
            .await
    });
    within_one_second(runner.cancelled.notified()).await;
    finish_endpoint_close(&runner, close).await;
    assert_cancelled_stream(&mut stream, true).await;
    assert!(matches!(
        endpoint.start(request("endpoint-close", 2)).await,
        Err(CapsuleConnectionError::Rejected(
            CapsuleNodeFailure::RunClosed
        ))
    ));
    assert!(!*endpoint.connection_loss().borrow());
}

#[tokio::test]
async fn dropped_ready_endpoint_stream_cancels_even_without_provider_output() {
    let (runner, endpoint, stream) = ready_endpoint("endpoint-drop-ready").await;
    drop(stream);
    within_one_second(runner.cancelled.notified()).await;
    let closing_endpoint = endpoint.clone();
    let close = tokio::spawn(async move {
        closing_endpoint
            .close_run(&RunId::new("endpoint-drop-ready"))
            .await
    });
    finish_endpoint_close(&runner, close).await;
}

async fn finish_endpoint_close(
    runner: &DelayedRunner,
    mut close: tokio::task::JoinHandle<Result<(), CapsuleConnectionError>>,
) {
    runner.close.wait().await;
    runner.close.open();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut close)
            .await
            .is_err()
    );
    runner.finish.notify_one();
    within_one_second(close).await.assert_value().assert_value();
}

struct RejectedRunner;

#[async_trait]
impl NodeRunner for RejectedRunner {
    async fn start(&self, _request: NodeRunRequest) -> Result<NodeHandle, NodeRunnerError> {
        Err(NodeRunnerError::SessionLost)
    }

    async fn close_run(&self, _run_id: &RunId) {}
}

#[tokio::test]
async fn accepted_endpoint_start_failure_is_reported_by_its_owned_stream() {
    let endpoint = Arc::new(NativeCapsuleNodeEndpoint::new(Arc::new(RejectedRunner)));
    let mut stream = endpoint
        .start(request("endpoint-start-rejected", 1))
        .await
        .assert_value();
    assert!(matches!(
        within_one_second(stream.recv()).await,
        Some(CapsuleNodeEvent::Failed {
            failure: CapsuleNodeFailure::SessionLost
        })
    ));
    assert!(within_one_second(stream.recv()).await.is_none());
    let proxy = RemoteCapsuleNodeRunner::new(endpoint);
    let mut handle = proxy
        .start(request("endpoint-start-rejected", 2))
        .await
        .assert_value();
    assert_eq!(
        within_one_second(handle.completion()).await,
        Err(NodeRunnerError::SessionLost)
    );
    assert!(!*proxy.connection_loss().borrow());
}

#[tokio::test]
async fn endpoint_close_cancels_pending_provider_without_releasing_start() {
    let runner = Arc::new(DelayedRunner::default());
    let endpoint = Arc::new(NativeCapsuleNodeEndpoint::new(runner.clone()));
    let mut stream = endpoint
        .start(request("endpoint-close-pending", 1))
        .await
        .assert_value();
    runner.start.wait().await;
    let closing = endpoint.clone();
    let close = tokio::spawn(async move {
        closing
            .close_run(&RunId::new("endpoint-close-pending"))
            .await
    });
    runner.close.wait().await;
    within_one_second(runner.dropped_start.notified()).await;
    runner.close.open();
    within_one_second(close).await.assert_value().assert_value();
    assert_cancelled_stream(&mut stream, false).await;
    assert!(matches!(
        endpoint.start(request("endpoint-close-pending", 2)).await,
        Err(CapsuleConnectionError::Rejected(
            CapsuleNodeFailure::RunClosed
        ))
    ));
    runner.start.open();
    tokio::task::yield_now().await;
    assert_eq!(runner.work_started.load(Ordering::SeqCst), 0);
}
