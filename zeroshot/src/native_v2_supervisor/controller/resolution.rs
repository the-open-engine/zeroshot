use super::*;
use crate::native_v2_runner::ResolvedEnvironment;
use tokio::time::Instant;

const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(1);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

pub(super) enum InitialEnvironment {
    Ready(ResolvedEnvironment),
    Stopped(DispatchResult),
}

impl NativeV2Supervisor {
    pub(super) async fn resolve_initial_environment(
        &self,
        invocation: &NodeInvocation,
        deadline: Option<Instant>,
        cancel: &mut oneshot::Receiver<ExecutionInterrupt>,
    ) -> Result<InitialEnvironment, NativeV2SupervisorError> {
        tokio::select! {
            biased;
            stopped = self.wait_startup_interrupt(deadline, cancel) => {
                Ok(InitialEnvironment::Stopped(stopped))
            }
            result = self.resolve_environment_with_retry(invocation) => result,
        }
    }

    pub(super) fn startup_interrupt(
        &self,
        deadline: Option<Instant>,
        cancel: &mut oneshot::Receiver<ExecutionInterrupt>,
    ) -> Option<DispatchResult> {
        if *self.resolution_stop.borrow()
            || !matches!(cancel.try_recv(), Err(oneshot::error::TryRecvError::Empty))
        {
            return Some(DispatchResult::Interrupted);
        }
        deadline
            .filter(|deadline| *deadline <= Instant::now())
            .map(|_| DispatchResult::TimedOut)
    }

    pub(super) async fn wait_startup_interrupt(
        &self,
        deadline: Option<Instant>,
        cancel: &mut oneshot::Receiver<ExecutionInterrupt>,
    ) -> DispatchResult {
        let mut stopping = self.resolution_stop.subscribe();
        if let Some(stopped) = self.startup_interrupt(deadline, cancel) {
            return stopped;
        }
        tokio::select! {
            biased;
            _ = stopping.changed() => DispatchResult::Interrupted,
            _ = cancel => DispatchResult::Interrupted,
            _ = crate::execution::process::wait_for_deadline(deadline) => DispatchResult::TimedOut,
        }
    }

    async fn resolve_environment_with_retry(
        &self,
        invocation: &NodeInvocation,
    ) -> Result<InitialEnvironment, NativeV2SupervisorError> {
        let mut delay = INITIAL_RETRY_DELAY;
        loop {
            match self.environment.resolve(&invocation.binding).await {
                Ok(environment) => return Ok(InitialEnvironment::Ready(environment)),
                Err(RunEnvironmentError::ResolutionUnavailable) => {
                    self.log_resolution_retry(&invocation.reference, delay)
                        .await?;
                    tokio::time::sleep(delay).await;
                    delay = delay.saturating_mul(2).min(MAX_RETRY_DELAY);
                }
                Err(error) => {
                    return Ok(InitialEnvironment::Stopped(DispatchResult::Completed(Ok(
                        NodeCompletion {
                            reference: invocation.reference.clone(),
                            outcome: resolution_failure(error),
                        },
                    ))));
                }
            }
        }
    }

    async fn log_resolution_retry(
        &self,
        reference: &ExecutionRef,
        delay: Duration,
    ) -> Result<(), NativeV2SupervisorError> {
        self.ledger
            .append(
                &self.run_id,
                vec![RunEvent::SafeLog {
                    execution: Some(reference.execution),
                    timestamp: crate::native_v2_runner::current_timestamp(),
                    stream: SafeLogStream::System,
                    line: SafeLogLine::new(format!(
                        "Node {} waiting for connection resolution; retrying in {}s",
                        reference.node.as_str(),
                        delay.as_secs(),
                    ))?,
                }],
            )
            .await?;
        Ok(())
    }
}

fn resolution_failure(error: RunEnvironmentError) -> WorkerOutcome {
    match error {
        RunEnvironmentError::ResolutionRefused => WorkerOutcome::authentication_refusal(),
        _ => WorkerOutcome::malformed(),
    }
}
