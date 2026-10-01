use super::*;

mod resolution;
mod start;

impl NativeV2Supervisor {
    pub(super) async fn dispatch(
        &self,
        program: &RunProgram,
        dispatches: Vec<Dispatch>,
        active: &mut ActiveDispatches,
    ) -> Result<bool, NativeV2SupervisorError> {
        if self.force_requested.load(Ordering::Acquire)
            || !self.record_dispatches(&dispatches).await?
        {
            return Ok(false);
        }
        for dispatch in dispatches {
            self.start_dispatch(program, dispatch, active).await?;
        }
        Ok(true)
    }

    pub(super) async fn record_dispatches(
        &self,
        dispatches: &[Dispatch],
    ) -> Result<bool, NativeV2SupervisorError> {
        let events = dispatches
            .iter()
            .map(|dispatch| RunEvent::NodeStarted {
                reference: dispatch.reference.clone(),
                occurrence: dispatch.occurrence.clone(),
                attempt: dispatch.attempt,
                input: dispatch.input.clone(),
            })
            .collect();
        if let Err(error) = self.ledger.append(&self.run_id, events).await {
            if let Ok(snapshot) = self.snapshot().await
                && (snapshot.force_stop_requested || snapshot.terminal.is_some())
            {
                return Ok(false);
            }
            return Err(error.into());
        }
        Ok(true)
    }

    pub(super) async fn start_dispatch(
        &self,
        program: &RunProgram,
        dispatch: Dispatch,
        active: &mut ActiveDispatches,
    ) -> Result<(), NativeV2SupervisorError> {
        let binding = program
            .admitted
            .runtime
            .nodes()
            .get(&dispatch.reference.node)
            .cloned()
            .ok_or(NativeV2SupervisorError::InvalidState)?;
        let instructions = program
            .instructions
            .get(&dispatch.reference.node)
            .cloned()
            .ok_or(NativeV2SupervisorError::InvalidState)?;
        let timeout = *program
            .timeouts
            .get(&dispatch.reference.node)
            .ok_or(NativeV2SupervisorError::InvalidState)?;
        let invocation = NodeInvocation {
            reference: dispatch.reference,
            worker: dispatch.worker,
            instructions,
            input: dispatch.input,
            binding,
        };
        let (cancel, receiver) = oneshot::channel();
        active
            .cancellations
            .insert(invocation.reference.execution, cancel);
        active
            .tasks
            .spawn(self.clone().run_pending_node(invocation, timeout, receiver));
        Ok(())
    }

    pub(super) async fn register_live(
        &self,
        reference: &ExecutionRef,
        handle: &mut NodeHandle,
    ) -> Result<Option<Box<dyn LiveOutputRegistration>>, LiveOutputUnavailable> {
        let Some(registrar) = &self.live_output else {
            return Ok(None);
        };
        let source = handle.live_output_source().ok_or(LiveOutputUnavailable)?;
        registrar.register(reference, source).await.map(Some)
    }

    pub(super) async fn cancel_voids(
        &self,
        voids: Vec<(ExecutionId, ExecutionVoidReason)>,
        active: &mut ActiveDispatches,
    ) -> Result<(), NativeV2SupervisorError> {
        let targets = voids
            .into_iter()
            .map(|(execution, reason)| {
                active.pending_voids.insert(execution, reason);
                let cancel = active
                    .cancellations
                    .remove(&execution)
                    .ok_or(NativeV2SupervisorError::InvalidState)?;
                let _ = cancel.send(ExecutionInterrupt::Void);
                Ok(execution)
            })
            .collect::<Result<BTreeSet<_>, NativeV2SupervisorError>>()?;
        while targets
            .iter()
            .any(|execution| active.pending_voids.contains_key(execution))
        {
            let finished = Self::next_finished(active).await?;
            self.settle(finished, &mut active.pending_voids).await?;
        }
        Ok(())
    }

    pub(super) async fn settle(
        &self,
        finished: FinishedDispatch,
        pending_voids: &mut BTreeMap<ExecutionId, ExecutionVoidReason>,
    ) -> Result<(), NativeV2SupervisorError> {
        self.require_finished_cleanup(&finished).await?;
        if let Some(reason) = pending_voids.remove(&finished.execution) {
            let mut events = finished.failure_log()?.into_iter().collect::<Vec<_>>();
            events.push(RunEvent::ExecutionVoided {
                reference: finished.reference,
                reason,
            });
            self.ledger.append(&self.run_id, events).await?;
            return Ok(());
        }
        let snapshot = self.snapshot().await?;
        let force = self.force_requested.load(Ordering::Acquire) || snapshot.force_stop_requested;
        let events = finished.completion_events(force)?;
        self.ledger.append(&self.run_id, events).await?;
        Ok(())
    }

    pub(super) async fn append_terminal(
        &self,
        terminal: TerminalResult,
    ) -> Result<TerminalResult, NativeV2SupervisorError> {
        self.ledger
            .append(
                &self.run_id,
                super::logging::terminal_events(terminal.clone())?,
            )
            .await?;
        Ok(terminal)
    }

    async fn stop_runtime_tasks(
        &self,
        tasks: &mut JoinSet<FinishedDispatch>,
    ) -> Result<Vec<RunEvent>, NativeV2SupervisorError> {
        self.resolution_stop.send_replace(true);
        self.runner.close_run(&self.run_id).await;
        self.drain_terminalizing_tasks(tasks).await
    }

    pub(super) async fn terminalize_force(
        &self,
        tasks: &mut JoinSet<FinishedDispatch>,
    ) -> Result<TerminalResult, NativeV2SupervisorError> {
        let logs = self.stop_runtime_tasks(tasks).await?;
        let result = async {
            let snapshot = self.snapshot().await?;
            if let Some(terminal) = snapshot.terminal {
                return Ok(terminal);
            }
            let terminal = TerminalResult::Failed {
                reason: EnumLabel::new("force_stopped")
                    .map_err(|_| NativeV2SupervisorError::InvalidState)?,
            };
            self.cleanup_runtime(RunRuntimeExit::ForceStopped).await?;
            let mut events = logs.clone();
            events.extend(super::logging::terminal_failure_events(
                &snapshot,
                "force_stopped",
                WorkerErrorCode::Refusal,
            )?);
            self.ledger.append(&self.run_id, events).await?;
            Ok(terminal)
        }
        .await;
        self.preserve_terminalization_logs(logs, result).await
    }

    pub(super) async fn terminalize_lost(
        &self,
        tasks: &mut JoinSet<FinishedDispatch>,
    ) -> Result<TerminalResult, NativeV2SupervisorError> {
        let logs = self.stop_runtime_tasks(tasks).await?;
        let result = async {
            self.cleanup_runtime(RunRuntimeExit::RuntimeLost).await?;
            self.append_runtime_failure("runtime_lost", logs.clone())
                .await
        }
        .await;
        self.preserve_terminalization_logs(logs, result).await
    }

    pub(super) async fn append_runtime_failure(
        &self,
        reason: &str,
        mut events: Vec<RunEvent>,
    ) -> Result<TerminalResult, NativeV2SupervisorError> {
        let snapshot = self.snapshot().await?;
        if let Some(terminal) = snapshot.terminal {
            return Ok(terminal);
        }
        let terminal = TerminalResult::Failed {
            reason: EnumLabel::new(reason).map_err(|_| NativeV2SupervisorError::InvalidState)?,
        };
        events.extend(super::logging::terminal_failure_events(
            &snapshot,
            reason,
            WorkerErrorCode::Crash,
        )?);
        self.ledger.append(&self.run_id, events).await?;
        Ok(terminal)
    }
}
