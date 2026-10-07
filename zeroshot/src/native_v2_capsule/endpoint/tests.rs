use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use openengine_cluster_protocol::RunId;
use openengine_cluster_testkit::assertions::AssertValue;

use super::*;
use tokio::sync::Notify;
use crate::native_v2_runner::remote_node_handle;

struct NeverStarted(AtomicUsize);

#[async_trait]
impl NodeRunner for NeverStarted {
    async fn start(&self, _request: NodeRunRequest) -> Result<NodeHandle, NodeRunnerError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(NodeRunnerError::Driver)
    }

    async fn close_run(&self, _run_id: &RunId) {}
}

struct MissingOutput;

#[async_trait]
impl NodeRunner for MissingOutput {
    async fn start(&self, request: NodeRunRequest) -> Result<NodeHandle, NodeRunnerError> {
        let (mut handle, bridge) = remote_node_handle(request.invocation.reference);
        let _ = handle.take_initial_output().assert_value();
        bridge.finish(Err(NodeRunnerError::Driver));
        Ok(handle)
    }

    async fn close_run(&self, _run_id: &RunId) {}
}

#[test]
fn boundary_contract_run_open_validation_gives_connection_loss_precedence_over_closed_run() {
    let endpoint = NativeCapsuleNodeEndpoint::new(Arc::new(NeverStarted(AtomicUsize::new(0))));
    let run_id = RunId::new("run-open-contract");
    let mut state = EndpointState::default();
    assert!(endpoint.ensure_run_open(&state, &run_id).is_ok());
    state.closed_runs.insert(run_id.clone());
    assert_eq!(
        endpoint.ensure_run_open(&state, &run_id),
        Err(CapsuleConnectionError::Rejected(
            CapsuleNodeFailure::RunClosed
        ))
    );
    endpoint.loss.send_replace(true);
    assert_eq!(
        endpoint.ensure_run_open(&state, &run_id),
        Err(CapsuleConnectionError::Lost)
    );
}

#[tokio::test]
async fn boundary_contract_pre_cancelled_reservation_fails_without_starting_provider_work() {
    let runner = Arc::new(NeverStarted(AtomicUsize::new(0)));
    let endpoint = NativeCapsuleNodeEndpoint::new(runner.clone());
    let request = crate::native_v2_capsule::tests::request("pre-cancelled", 1);
    let reference = request.invocation.reference.clone();
    let (events, mut receiver) = mpsc::channel(1);
    let (terminal, terminal_receiver) = oneshot::channel();
    let (_cancel, commands) = watch::channel(true);
    let (done, done_receiver) = watch::channel(false);
    let (cancel, _) = watch::channel(false);
    endpoint.state.lock().await.active.push(EndpointExecution {
        reference: reference.clone(),
        cancel,
        done: done_receiver,
    });

    endpoint
        .clone()
        .start_reserved(
            request,
            ReservedLocalStart {
                reference,
                done,
                commands,
                events,
                terminal,
            },
        )
        .await;

    assert_eq!(runner.0.load(Ordering::SeqCst), 0);
    assert!(receiver.recv().await.is_none());
    assert!(matches!(
        terminal_receiver.await.assert_value().as_slice(),
        [CapsuleNodeEvent::Failed {
            failure: CapsuleNodeFailure::Cancelled
        }]
    ));
    assert!(endpoint.state.lock().await.active.is_empty());
}

#[tokio::test]
async fn boundary_contract_handle_without_durable_output_is_cancelled_settled_and_reported() {
    let endpoint = NativeCapsuleNodeEndpoint::new(Arc::new(MissingOutput));
    let mut stream = endpoint
        .start(crate::native_v2_capsule::tests::request(
            "missing-output",
            1,
        ))
        .await
        .assert_value();
    assert!(matches!(
        stream.recv().await,
        Some(CapsuleNodeEvent::Failed {
            failure: CapsuleNodeFailure::ExecutionFailed
        })
    ));
    assert!(stream.recv().await.is_none());
    assert!(endpoint.state.lock().await.active.is_empty());
    assert!(!*endpoint.loss.borrow());
}

struct DetailedFailure {
    during_start: bool,
    await_cancel: bool,
    cleanup_failure: bool,
    started: Arc<Notify>,
}

#[async_trait]
impl NodeRunner for DetailedFailure {
    async fn start(&self, request: NodeRunRequest) -> Result<NodeHandle, NodeRunnerError> {
        let error = NodeRunnerError::DriverDetail("upstream rejected model [REDACTED]".to_owned());
        if self.during_start {
            return Err(error);
        }
        let (handle, mut bridge) = remote_node_handle(request.invocation.reference);
        let await_cancel = self.await_cancel;
        let cleanup_failure = self.cleanup_failure;
        tokio::spawn(async move {
            if await_cancel {
                bridge.cancelled().await;
            }
            if cleanup_failure {
                bridge
                    .emit(
                        crate::native_v2_runner::LiveOutput::new(
                            LiveOutputStream::Error,
                            "upstream rejected model [REDACTED]",
                        )
                        .assert_value(),
                    )
                    .await
                    .assert_value();
                bridge.finish(Err(NodeRunnerError::CleanupUnconfirmed));
            } else {
                bridge.finish(Err(error));
            }
        });
        self.started.notify_one();
        Ok(handle)
    }
    async fn close_run(&self, _run_id: &RunId) {}
}

#[tokio::test]
async fn returned_details_survive_startup_and_completion_capsule_transport_once() {
    for (during_start, await_cancel, cleanup_failure) in [
        (true, false, false),
        (false, false, false),
        (false, true, false),
        (false, true, true),
    ] {
        let started = Arc::new(Notify::new());
        let endpoint = Arc::new(NativeCapsuleNodeEndpoint::new(Arc::new(DetailedFailure {
            during_start,
            await_cancel,
            cleanup_failure,
            started: started.clone(),
        })));
        let proxy = RemoteCapsuleNodeRunner::new(endpoint);
        let mut handle = proxy
            .start(crate::native_v2_capsule::tests::request(
                "detailed-failure",
                1,
            ))
            .await
            .assert_value();
        let mut output = handle.take_initial_output().assert_value();
        if await_cancel {
            started.notified().await;
            handle.cancel();
        }
        let (event, completion) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(output.recv_output(), handle.completion())
        })
        .await
        .assert_value();
        let event = event.assert_value();
        assert_eq!(event.stream, LiveOutputStream::Error);
        assert_eq!(
            event.text,
            if cleanup_failure {
                "upstream rejected model [REDACTED]"
            } else {
                "node execution failed: upstream rejected model [REDACTED]"
            }
        );
        assert_eq!(
            completion,
            Err(if cleanup_failure {
                NodeRunnerError::CleanupUnconfirmed
            } else {
                NodeRunnerError::Driver
            })
        );
        assert!(output.recv().await.is_err());
    }
}
