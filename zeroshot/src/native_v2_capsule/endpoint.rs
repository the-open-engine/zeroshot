use super::*;

#[path = "endpoint/execution.rs"]
mod execution;
use execution::{LocalExecutionTask, remove_endpoint_execution, serve_local_execution};

#[derive(Clone)]
pub struct NativeCapsuleNodeEndpoint {
    runner: Arc<dyn NodeRunner>,
    loss: watch::Sender<bool>,
    state: Arc<Mutex<EndpointState>>,
}

#[derive(Default)]
struct EndpointState {
    active: Vec<EndpointExecution>,
    closed_runs: std::collections::BTreeSet<RunId>,
}

struct EndpointExecution {
    reference: ExecutionRef,
    cancel: watch::Sender<bool>,
    done: watch::Receiver<bool>,
}

impl NativeCapsuleNodeEndpoint {
    #[must_use]
    pub fn new(runner: Arc<dyn NodeRunner>) -> Self {
        let (loss, _) = watch::channel(false);
        Self {
            runner,
            loss,
            state: Arc::default(),
        }
    }

    /// Breaks the private connection and waits for all capsule-side provider cleanup.
    pub async fn disconnect(&self) {
        let _ = self.loss.send(true);
        let run_ids = {
            let state = self.state.lock().await;
            state
                .active
                .iter()
                .map(|entry| entry.reference.run_id.clone())
                .collect::<std::collections::BTreeSet<_>>()
        };
        for run_id in run_ids {
            self.close_local_run(&run_id).await;
        }
    }

    async fn settle_unusable_handle(&self, mut handle: NodeHandle) -> bool {
        let mut durable = handle.take_initial_output();
        handle.cancel();
        let settled = tokio::time::timeout(CLOSE_RPC_TIMEOUT, async {
            let completion = handle.completion();
            let drain = async {
                if let Some(output) = durable.as_mut() {
                    while output.recv().await.is_ok() {}
                }
            };
            let _ = tokio::join!(completion, drain);
        })
        .await
        .is_ok();
        if !settled {
            self.loss.send_replace(true);
        }
        settled
    }

    async fn finish_reservation(&self, reference: &ExecutionRef, done: &watch::Sender<bool>) {
        remove_endpoint_execution(&self.state, reference, done).await;
        done.send_replace(true);
    }

    fn ensure_run_open(
        &self,
        state: &EndpointState,
        run_id: &RunId,
    ) -> Result<(), CapsuleConnectionError> {
        if *self.loss.borrow() {
            return Err(CapsuleConnectionError::Lost);
        }
        if state.closed_runs.contains(run_id) {
            return Err(CapsuleConnectionError::Rejected(
                CapsuleNodeFailure::RunClosed,
            ));
        }
        Ok(())
    }

    async fn start_reserved(self, request: NodeRunRequest, mut reserved: ReservedLocalStart) {
        let result = if *reserved.commands.borrow() || reserved.events.is_closed() {
            Err(NodeRunnerError::Cancelled)
        } else {
            tokio::select! {
                biased;
                result = self.runner.start(request) => result,
                () = wait_for_signal(&mut reserved.commands) => Err(NodeRunnerError::Cancelled),
                () = reserved.events.closed() => Err(NodeRunnerError::Cancelled),
            }
        };
        let mut handle = match result {
            Ok(handle) => handle,
            Err(error) => {
                self.finish_failed_start(reserved, error).await;
                return;
            }
        };
        let Some(durable) = handle.take_initial_output() else {
            self.settle_unusable_handle(handle).await;
            self.finish_failed_start(reserved, NodeRunnerError::Driver)
                .await;
            return;
        };
        serve_local_execution(LocalExecutionTask {
            handle,
            durable,
            commands: reserved.commands,
            events: reserved.events,
            terminal: reserved.terminal,
            state: self.state.clone(),
            reference: reserved.reference,
            done: reserved.done,
        })
        .await;
    }

    async fn finish_failed_start(&self, reserved: ReservedLocalStart, error: NodeRunnerError) {
        drop(reserved.events);
        let _ = reserved.terminal.send(vec![CapsuleNodeEvent::Failed {
            failure: CapsuleNodeFailure::from_runner(&error),
        }]);
        self.finish_reservation(&reserved.reference, &reserved.done)
            .await;
    }

    async fn close_local_run(&self, run_id: &RunId) {
        let mut completions = {
            let mut state = self.state.lock().await;
            state.closed_runs.insert(run_id.clone());
            for entry in state
                .active
                .iter()
                .filter(|entry| &entry.reference.run_id == run_id)
            {
                entry.cancel.send_replace(true);
            }
            state
                .active
                .iter()
                .filter(|entry| &entry.reference.run_id == run_id)
                .map(|entry| entry.done.clone())
                .collect::<Vec<_>>()
        };
        self.runner.close_run(run_id).await;
        for completion in &mut completions {
            while !*completion.borrow_and_update() {
                if completion.changed().await.is_err() {
                    break;
                }
            }
        }
    }
}

#[async_trait]
impl CapsuleNodeChannel for NativeCapsuleNodeEndpoint {
    async fn start(
        &self,
        request: NodeRunRequest,
    ) -> Result<CapsuleExecutionStream, CapsuleConnectionError> {
        if *self.loss.borrow() {
            return Err(CapsuleConnectionError::Lost);
        }
        let reference = request.invocation.reference.clone();
        let (events, receiver) = mpsc::channel(DURABLE_OUTPUT_CAPACITY);
        let (terminal, terminal_receiver) = oneshot::channel();
        let (cancel, commands) = watch::channel(false);
        let (done_sender, done) = watch::channel(false);
        {
            let mut state = self.state.lock().await;
            self.ensure_run_open(&state, &reference.run_id)?;
            if state
                .active
                .iter()
                .any(|entry| entry.reference == reference)
            {
                return Err(CapsuleConnectionError::Rejected(
                    CapsuleNodeFailure::ExecutionActive,
                ));
            }
            state.active.push(EndpointExecution {
                reference: reference.clone(),
                cancel,
                done,
            });
        }
        let endpoint = self.clone();
        let reserved_reference = reference.clone();
        tokio::spawn(endpoint.start_reserved(
            request,
            ReservedLocalStart {
                reference: reserved_reference,
                done: done_sender,
                commands,
                events,
                terminal,
            },
        ));
        Ok(CapsuleExecutionStream::from_bounded_receiver(
            receiver,
            terminal_receiver,
        ))
    }

    async fn cancel(&self, reference: &ExecutionRef) -> Result<(), CapsuleConnectionError> {
        if *self.loss.borrow() {
            return Err(CapsuleConnectionError::Lost);
        }
        let state = self.state.lock().await;
        if let Some(entry) = state
            .active
            .iter()
            .find(|entry| &entry.reference == reference)
        {
            entry.cancel.send_replace(true);
        }
        Ok(())
    }

    async fn close_run(&self, run_id: &RunId) -> Result<(), CapsuleConnectionError> {
        if *self.loss.borrow() {
            return Err(CapsuleConnectionError::Lost);
        }
        self.close_local_run(run_id).await;
        Ok(())
    }

    fn connection_loss(&self) -> watch::Receiver<bool> {
        self.loss.subscribe()
    }
}

struct ReservedLocalStart {
    reference: ExecutionRef,
    done: watch::Sender<bool>,
    commands: watch::Receiver<bool>,
    events: mpsc::Sender<CapsuleNodeEvent>,
    terminal: oneshot::Sender<Vec<CapsuleNodeEvent>>,
}

#[cfg(test)]
#[path = "endpoint/tests.rs"]
mod tests;
