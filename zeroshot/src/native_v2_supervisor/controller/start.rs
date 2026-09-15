use super::*;
use super::resolution::InitialEnvironment;
use tokio::time::Instant;

impl NativeV2Supervisor {
    pub(super) async fn run_pending_node(
        self,
        invocation: NodeInvocation,
        timeout: Option<Duration>,
        cancel: oneshot::Receiver<ExecutionInterrupt>,
    ) -> FinishedDispatch {
        let started = Instant::now();
        let reference = invocation.reference.clone();
        let deadline = timeout.map(|duration| started + duration);
        let result = self
            .prepare_and_run(invocation, deadline, cancel)
            .await
            .unwrap_or_else(DispatchResult::StartFailure);
        FinishedDispatch {
            execution: reference.execution,
            reference,
            result,
            elapsed: started.elapsed(),
        }
    }

    async fn prepare_and_run(
        &self,
        invocation: NodeInvocation,
        deadline: Option<Instant>,
        mut cancel: oneshot::Receiver<ExecutionInterrupt>,
    ) -> Result<DispatchResult, NativeV2SupervisorError> {
        let environment = match self
            .resolve_initial_environment(&invocation, deadline, &mut cancel)
            .await?
        {
            InitialEnvironment::Ready(environment) => environment,
            InitialEnvironment::Stopped(result) => return Ok(result),
        };
        if let Some(stopped) = self.startup_interrupt(deadline, &mut cancel) {
            return Ok(stopped);
        }
        let request = NodeRunRequest {
            invocation,
            environment,
        };
        let handle = tokio::select! {
            biased;
            result = self.runner.start(request) => match result {
                Ok(handle) => handle,
                Err(error) => return Ok(DispatchResult::Completed(Err(error))),
            },
            stopped = self.wait_startup_interrupt(deadline, &mut cancel) => return Ok(stopped),
        };
        self.run_started_node(handle, deadline, cancel).await
    }

    async fn run_started_node(
        &self,
        mut handle: NodeHandle,
        deadline: Option<Instant>,
        mut cancel: oneshot::Receiver<ExecutionInterrupt>,
    ) -> Result<DispatchResult, NativeV2SupervisorError> {
        let reference = handle.reference().clone();
        let Some(output) = handle.take_initial_output() else {
            handle.cancel();
            let _ = handle.completion().await;
            return Err(NativeV2SupervisorError::InvalidState);
        };
        let registration = tokio::select! {
            biased;
            stopped = self.wait_startup_interrupt(deadline, &mut cancel) => Err(stopped),
            registered = self.register_live(&reference, &mut handle) => {
                registered.map_err(|_| DispatchResult::Completed(Ok(NodeCompletion {
                    reference: reference.clone(),
                    outcome: WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
                })))
            }
        };
        let registration = match registration {
            Ok(registration) => registration,
            Err(stopped) => return Ok(self.drain_unregistered_node(handle, output, stopped).await),
        };
        let finished = run_dispatch(DispatchTask {
            handle,
            timeout: deadline.map(|deadline| deadline.saturating_duration_since(Instant::now())),
            cancel,
            ledger: self.ledger.clone(),
            run_id: self.run_id.clone(),
            registration,
            output,
        })
        .await;
        Ok(finished.result)
    }

    async fn drain_unregistered_node(
        &self,
        handle: NodeHandle,
        output: crate::native_v2_runner::DurableOutput,
        stopped: DispatchResult,
    ) -> DispatchResult {
        handle.cancel();
        let (sender, cancel) = oneshot::channel();
        drop(sender);
        let finished = run_dispatch(DispatchTask {
            handle,
            timeout: None,
            cancel,
            ledger: self.ledger.clone(),
            run_id: self.run_id.clone(),
            registration: None,
            output,
        })
        .await;
        if finished.result.cleanup_unconfirmed() {
            return finished.result;
        }
        match finished.result {
            DispatchResult::DurableEventFailure(error) => {
                DispatchResult::DurableEventFailure(error)
            }
            _ => stopped,
        }
    }
}
