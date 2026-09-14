use super::*;

pub(super) async fn remote_start(
    runner: &RemoteCapsuleNodeRunner,
    request: NodeRunRequest,
) -> Result<NodeHandle, NodeRunnerError> {
    if connection_is_lost(&runner.connection_loss) {
        return Err(NodeRunnerError::ConnectionLost);
    }
    let reference = request.invocation.reference.clone();
    let (handle, bridge) = remote_node_handle(reference.clone());
    let registration = runner
        .activity
        .register(reference.clone(), bridge.cancellation_signal())
        .await?;
    tokio::spawn(establish_remote_execution(PendingRemoteStart {
        runner: runner.clone(),
        request,
        reference,
        bridge,
        registration,
    }));
    Ok(handle)
}

struct PendingRemoteStart {
    runner: RemoteCapsuleNodeRunner,
    request: NodeRunRequest,
    reference: ExecutionRef,
    bridge: RemoteNodeHandleBridge,
    registration: ProxyRegistration,
}

async fn establish_remote_execution(mut pending: PendingRemoteStart) {
    let stream = match await_remote_start(&mut pending).await {
        Ok(stream) => stream,
        Err(error) => {
            pending.bridge.finish(Err(error));
            pending
                .runner
                .activity
                .finish(&pending.reference, &pending.registration.done)
                .await;
            return;
        }
    };
    let runtime = ProxyRuntime {
        channel: pending.runner.channel.clone(),
        connection_loss: pending.runner.connection_loss.clone(),
        loss: pending.runner.loss.clone(),
        activity: pending.runner.activity.clone(),
        control_timeout: pending.runner.control_timeout,
    };
    let task = RemoteExecutionTask {
        runtime,
        reference: pending.reference,
        stream,
        bridge: pending.bridge,
        registration: pending.registration,
    };
    drive_remote_execution(task).await;
}

async fn await_remote_start(
    pending: &mut PendingRemoteStart,
) -> Result<CapsuleExecutionStream, NodeRunnerError> {
    let mut connection_loss = pending.runner.connection_loss.clone();
    let mut closed = pending.registration.closed.clone();
    let mut cancellation = pending.bridge.cancellation_signal().subscribe();
    if *cancellation.borrow() {
        return Err(pending_start_cancelled(pending).await);
    }
    let start = pending.runner.channel.start(pending.request.clone());
    tokio::pin!(start);
    tokio::select! {
        biased;
        result = &mut start => match result {
            Ok(stream) => Ok(stream),
            Err(CapsuleConnectionError::Lost) => {
                pending.runner.loss.promote();
                Err(NodeRunnerError::ConnectionLost)
            }
            Err(CapsuleConnectionError::Rejected(failure)) => Err(failure.into_runner()),
        },
        () = wait_for_signal(&mut connection_loss) => Err(NodeRunnerError::ConnectionLost),
        () = wait_for_signal(&mut closed) => Err(NodeRunnerError::RunClosed),
        () = wait_for_signal(&mut cancellation) => Err(pending_start_cancelled(pending).await),
    }
}

async fn pending_start_cancelled(pending: &PendingRemoteStart) -> NodeRunnerError {
    if *pending.registration.closing.borrow() {
        let mut loss = pending.runner.connection_loss.clone();
        let mut closed = pending.registration.closed.clone();
        tokio::select! {
            biased;
            () = wait_for_signal(&mut loss) => return NodeRunnerError::ConnectionLost,
            () = wait_for_signal(&mut closed) => {}
        }
    }
    NodeRunnerError::Cancelled
}
