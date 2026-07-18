use std::sync::Arc;

use tokio::sync::{oneshot, watch};

use crate::fault::{EvidenceClass, FaultModule};

use super::LocalExecutionRuntime;
use super::state::{LiveExecution, LiveState, fault_is_session_lost};
use crate::execution::driver::{
    DriverCancellation, DriverStartOutcome, ExecutionSiteResolution, ResolvedExecutionSite,
};
use crate::execution::types::{
    DispatchObservation, ExecutionCommand, ExecutionControl, ExecutionTargetRef, SessionScope,
};

pub(super) struct DispatchContext {
    pub live: Arc<LiveExecution>,
    pub cancel_rx: watch::Receiver<bool>,
    pub ready_tx: oneshot::Sender<DispatchObservation>,
}

pub(super) async fn run_dispatch(
    runtime: &LocalExecutionRuntime,
    command: ExecutionCommand,
    context: DispatchContext,
) {
    let control = command.control();
    if node_instance_scope_is_unsupported(&command) {
        let fault = runtime.fault(FaultModule::Worker, EvidenceClass::InvariantViolation);
        retain_terminal_state(
            runtime,
            &control,
            LiveState::DefinitelyNotStarted {
                fault: Some(fault.clone()),
            },
        )
        .await;
        let _ = context
            .ready_tx
            .send(DispatchObservation::DefinitelyNotStarted {
                execution: command.execution(),
                dispatch_fence: command.dispatch_fence(),
                fault: Some(fault),
            });
        return;
    }

    match runtime.resolver.resolve(&command).await {
        ExecutionSiteResolution::Resolved(resolved) => {
            let outcome = start_resolved_site(*resolved, context.cancel_rx.clone()).await;
            handle_driver_outcome(runtime, control, context, outcome).await;
        }
        ExecutionSiteResolution::DefinitelyNotStarted { fault } => {
            let observation = resolve_not_started(&command, &fault);
            retain_terminal_state(
                runtime,
                &control,
                live_state_for_observation(&observation, fault),
            )
            .await;
            let _ = context.ready_tx.send(observation);
        }
        ExecutionSiteResolution::Indeterminate { fault } => {
            retain_terminal_state(
                runtime,
                &control,
                LiveState::Indeterminate {
                    fault: Some(fault.clone()),
                },
            )
            .await;
            let _ = context.ready_tx.send(DispatchObservation::Indeterminate {
                execution: command.execution(),
                dispatch_fence: command.dispatch_fence(),
                fault: Some(fault),
            });
        }
    }
}

async fn start_resolved_site(
    resolved: ResolvedExecutionSite,
    cancel_rx: watch::Receiver<bool>,
) -> DriverStartOutcome {
    match resolved {
        ResolvedExecutionSite::Agent { driver, request } => {
            driver
                .start(request, DriverCancellation::new(cancel_rx))
                .await
        }
        ResolvedExecutionSite::Builtin { driver, request } => {
            driver
                .start(request, DriverCancellation::new(cancel_rx))
                .await
        }
    }
}

async fn handle_driver_outcome(
    runtime: &LocalExecutionRuntime,
    control: ExecutionControl,
    context: DispatchContext,
    outcome: DriverStartOutcome,
) {
    let execution = control.execution();
    let dispatch_fence = control.dispatch_fence();
    match outcome {
        DriverStartOutcome::DefinitelyNotStarted { fault } => {
            finish_not_started(runtime, &control, context, fault).await;
        }
        DriverStartOutcome::MayHaveStarted { fault, completion } => {
            set_live_state(
                &context.live,
                LiveState::MayHaveStarted {
                    fault: fault.clone(),
                },
            )
            .await;
            let _ = context.ready_tx.send(DispatchObservation::MayHaveStarted {
                execution,
                dispatch_fence,
                fault: fault.clone(),
            });
            retain_terminal_state(
                runtime,
                &control,
                LiveState::Completed(completion.await.result),
            )
            .await;
        }
        DriverStartOutcome::Running { completion } => {
            set_live_state(&context.live, LiveState::Running).await;
            let _ = context.ready_tx.send(DispatchObservation::Running {
                execution,
                dispatch_fence,
            });
            retain_terminal_state(
                runtime,
                &control,
                LiveState::Completed(completion.await.result),
            )
            .await;
        }
        DriverStartOutcome::Completed { completion } => {
            let result = completion.result;
            retain_terminal_state(runtime, &control, LiveState::Completed(result.clone())).await;
            let _ = context.ready_tx.send(DispatchObservation::Completed {
                execution,
                dispatch_fence,
                result,
            });
        }
        DriverStartOutcome::Indeterminate { fault } => {
            retain_terminal_state(
                runtime,
                &control,
                LiveState::Indeterminate {
                    fault: fault.clone(),
                },
            )
            .await;
            let _ = context.ready_tx.send(DispatchObservation::Indeterminate {
                execution,
                dispatch_fence,
                fault,
            });
        }
    }
}

fn node_instance_scope_is_unsupported(command: &ExecutionCommand) -> bool {
    matches!(command.session_scope(), SessionScope::NodeInstance)
        && matches!(
            command.target(),
            ExecutionTargetRef::Agent(binding) if !binding.supports_node_instance()
        )
}

fn resolve_not_started(
    command: &ExecutionCommand,
    fault: &crate::fault::EngineFault,
) -> DispatchObservation {
    if fault_is_session_lost(fault) {
        DispatchObservation::Indeterminate {
            execution: command.execution(),
            dispatch_fence: command.dispatch_fence(),
            fault: Some(fault.clone()),
        }
    } else {
        DispatchObservation::DefinitelyNotStarted {
            execution: command.execution(),
            dispatch_fence: command.dispatch_fence(),
            fault: Some(fault.clone()),
        }
    }
}

fn live_state_for_observation(
    observation: &DispatchObservation,
    fault: crate::fault::EngineFault,
) -> LiveState {
    match observation {
        DispatchObservation::DefinitelyNotStarted { .. } => {
            LiveState::DefinitelyNotStarted { fault: Some(fault) }
        }
        _ => LiveState::Indeterminate { fault: Some(fault) },
    }
}

async fn set_live_state(live: &Arc<LiveExecution>, next: LiveState) {
    let mut state = live.state.lock().await;
    *state = next;
}

async fn retain_terminal_state(
    runtime: &LocalExecutionRuntime,
    control: &ExecutionControl,
    next: LiveState,
) {
    runtime.retain_terminal(control, next).await;
}

async fn finish_not_started(
    runtime: &LocalExecutionRuntime,
    control: &ExecutionControl,
    context: DispatchContext,
    fault: Option<crate::fault::EngineFault>,
) {
    let execution = control.execution();
    let dispatch_fence = control.dispatch_fence();
    let original_fault = fault.clone();
    let final_fault = fault.filter(|value| !fault_is_session_lost(value));
    let observation = if final_fault.is_some() || original_fault.is_none() {
        DispatchObservation::DefinitelyNotStarted {
            execution,
            dispatch_fence,
            fault: final_fault.clone(),
        }
    } else {
        DispatchObservation::Indeterminate {
            execution,
            dispatch_fence,
            fault: original_fault,
        }
    };
    let next = match &observation {
        DispatchObservation::DefinitelyNotStarted { fault, .. } => {
            LiveState::DefinitelyNotStarted {
                fault: fault.clone(),
            }
        }
        DispatchObservation::Indeterminate { fault, .. } => LiveState::Indeterminate {
            fault: fault.clone(),
        },
        _ => unreachable!(),
    };
    retain_terminal_state(runtime, control, next).await;
    let _ = context.ready_tx.send(observation);
}
