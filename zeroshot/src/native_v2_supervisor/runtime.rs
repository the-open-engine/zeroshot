use super::*;

impl NativeV2Supervisor {
    pub(super) async fn require_finished_cleanup(
        &self,
        finished: &FinishedDispatch,
    ) -> Result<(), NativeV2SupervisorError> {
        if !finished.result.cleanup_unconfirmed() {
            return Ok(());
        }
        // Logging cannot replace the cleanup failure or wait indefinitely for broken storage.
        // Leave the execution active: an error log is not proof that its processes stopped.
        if let Ok(Some(log)) = finished.failure_log() {
            self.record_failure_logs(vec![log]).await;
        }
        Err(NativeV2SupervisorError::CleanupUnconfirmed)
    }

    pub(super) async fn preserve_terminalization_logs(
        &self,
        logs: Vec<RunEvent>,
        result: Result<TerminalResult, NativeV2SupervisorError>,
    ) -> Result<TerminalResult, NativeV2SupervisorError> {
        if result.is_err() {
            self.record_failure_logs(logs).await;
        }
        result
    }

    async fn record_failure_logs(&self, logs: Vec<RunEvent>) {
        if !logs.is_empty() {
            let _ = tokio::time::timeout(
                Duration::from_secs(1),
                self.ledger.append(&self.run_id, logs),
            )
            .await;
        }
    }

    pub(super) async fn drain_failed_tasks(&self, tasks: &mut JoinSet<FinishedDispatch>) {
        let mut logs = Vec::new();
        // Every peer still drains, including after task panics and failed output persistence.
        while let Some(finished) = tasks.join_next().await {
            if let Ok(finished) = finished {
                if let Ok(Some(log)) = finished.failure_log() {
                    logs.push(log);
                }
            }
        }
        self.record_failure_logs(logs).await;
    }

    pub(super) async fn drain_terminalizing_tasks(
        &self,
        tasks: &mut JoinSet<FinishedDispatch>,
    ) -> Result<Vec<RunEvent>, NativeV2SupervisorError> {
        let mut logs = Vec::new();
        while let Some(finished) = tasks.join_next().await {
            let result = match finished {
                Ok(finished) => {
                    if let Some(log) = finished.failure_log()? {
                        logs.push(log);
                    }
                    terminal_drain_result(finished.result)
                }
                Err(error) => Err(supervisor_task_error(error)),
            };
            if let Err(error) = result {
                self.record_failure_logs(logs).await;
                return Err(error);
            }
        }
        Ok(logs)
    }
}

fn terminal_drain_result(result: DispatchResult) -> Result<(), NativeV2SupervisorError> {
    if result.cleanup_unconfirmed() {
        return Err(NativeV2SupervisorError::CleanupUnconfirmed);
    }
    match result {
        DispatchResult::DurableEventFailure(error) => Err(error.into()),
        DispatchResult::StartFailure(error) => Err(error),
        _ => Ok(()),
    }
}

pub(super) enum Initialization {
    Terminal(TerminalResult),
    Program(Box<RunProgram>),
}

pub(super) struct RunProgram {
    pub(super) admitted: AdmittedRun,
    pub(super) timeouts: BTreeMap<NodeName, Option<Duration>>,
    pub(super) instructions: BTreeMap<NodeName, Option<NodeInstructions>>,
}

#[derive(Default)]
pub(super) struct ActiveDispatches {
    pub(super) tasks: JoinSet<FinishedDispatch>,
    pub(super) cancellations: BTreeMap<ExecutionId, oneshot::Sender<ExecutionInterrupt>>,
    pub(super) pending_voids: BTreeMap<ExecutionId, ExecutionVoidReason>,
}

impl ActiveDispatches {
    pub(super) fn is_quiescent(&self, snapshot: &RunSnapshot) -> bool {
        self.tasks.is_empty() && snapshot.active_executions().next().is_none()
    }
}

pub(super) struct Dispatch {
    pub(super) reference: ExecutionRef,
    pub(super) occurrence: crate::full_v1_reducer::StructuralOccurrence,
    pub(super) attempt: openengine_cluster_protocol::PositiveInteger,
    pub(super) worker: openengine_cluster_protocol::WorkerRef,
    pub(super) input: serde_json::Value,
}

pub(super) fn void_decisions(decisions: &[Decision]) -> Vec<(ExecutionId, ExecutionVoidReason)> {
    decisions
        .iter()
        .filter_map(|decision| match decision {
            Decision::VoidLoser { execution, reason } => Some((*execution, *reason)),
            _ => None,
        })
        .collect()
}

pub(super) fn dispatch_decisions(run_id: &RunId, decisions: Vec<Decision>) -> Vec<Dispatch> {
    decisions
        .into_iter()
        .filter_map(|decision| match decision {
            Decision::Dispatch {
                node_instance,
                execution,
                occurrence,
                attempt,
                worker,
                input,
            } => Some(Dispatch {
                reference: ExecutionRef {
                    run_id: run_id.clone(),
                    node: occurrence.node.clone(),
                    node_instance,
                    execution,
                },
                occurrence,
                attempt,
                worker,
                input,
            }),
            Decision::VoidLoser { .. }
            | Decision::Continue { .. }
            | Decision::Promote { .. }
            | Decision::Terminal { .. } => None,
        })
        .collect()
}

#[derive(Clone, Copy)]
pub(super) enum ExecutionInterrupt {
    Void,
}

pub(super) enum DispatchResult {
    Completed(Result<NodeCompletion, NodeRunnerError>),
    TimedOut,
    Interrupted,
    DurableEventFailure(RunLedgerError),
    StartFailure(NativeV2SupervisorError),
}

impl DispatchResult {
    pub(super) fn cleanup_unconfirmed(&self) -> bool {
        matches!(
            self,
            Self::Completed(Err(NodeRunnerError::CleanupUnconfirmed))
        )
    }
}

pub(super) struct FinishedDispatch {
    pub(super) execution: ExecutionId,
    pub(super) reference: ExecutionRef,
    pub(super) result: DispatchResult,
    pub(super) elapsed: Duration,
}

pub(super) struct DispatchTask {
    pub(super) handle: NodeHandle,
    pub(super) timeout: Option<Duration>,
    pub(super) cancel: oneshot::Receiver<ExecutionInterrupt>,
    pub(super) ledger: Arc<dyn RunLedger>,
    pub(super) run_id: RunId,
    pub(super) registration: Option<Box<dyn LiveOutputRegistration>>,
    pub(super) output: crate::native_v2_runner::DurableOutput,
}

pub(super) async fn run_dispatch(task: DispatchTask) -> FinishedDispatch {
    let DispatchTask {
        mut handle,
        timeout,
        cancel,
        ledger,
        run_id,
        registration,
        output,
    } = task;
    let started = tokio::time::Instant::now();
    let reference = handle.reference().clone();
    let execution = reference.execution;
    let events = bridge_durable_events(ledger, run_id, execution, output);
    let result = observe_dispatch(&mut handle, events, timeout, cancel).await;
    if let Some(registration) = registration {
        registration.close().await;
    }
    FinishedDispatch {
        elapsed: started.elapsed(),
        execution,
        reference,
        result,
    }
}

async fn observe_dispatch(
    handle: &mut NodeHandle,
    events: impl std::future::Future<Output = Result<(), RunLedgerError>>,
    timeout: Option<Duration>,
    cancel: oneshot::Receiver<ExecutionInterrupt>,
) -> DispatchResult {
    let interrupt = async {
        tokio::select! {
            _ = crate::execution::process::wait_for_deadline(
                timeout.map(|duration| tokio::time::Instant::now() + duration),
            ) => DispatchResult::TimedOut,
            _ = cancel => DispatchResult::Interrupted,
        }
    };
    tokio::pin!(interrupt, events);
    let mut interrupted = None;
    let mut output_result = None;
    let result = loop {
        tokio::select! {
            completion = handle.completion() => break DispatchResult::Completed(completion),
            interrupt = &mut interrupt, if interrupted.is_none() => {
                handle.cancel();
                interrupted = Some(interrupt);
            }
            persisted = &mut events, if output_result.is_none() => {
                let failed = persisted.is_err();
                output_result = Some(persisted);
                if failed {
                    // Persistence owns failure now. A silent provider may never emit again, so
                    // cancel immediately. Keep polling completion and output together so cancellation
                    // cannot deadlock a provider that emits final usage through a bounded queue.
                    handle.cancel();
                }
            }
        }
    };
    let output_result = match output_result {
        Some(result) => result,
        None => events.await,
    };
    if result.cleanup_unconfirmed() {
        return result;
    }
    match output_result {
        Ok(()) => super::failure::preserve_interrupted_failure(interrupted, result),
        Err(error) => DispatchResult::DurableEventFailure(error),
    }
}

pub(super) async fn bridge_durable_events(
    ledger: Arc<dyn RunLedger>,
    run_id: RunId,
    execution: ExecutionId,
    mut output: crate::native_v2_runner::DurableOutput,
) -> Result<(), RunLedgerError> {
    const BATCH_SIZE: usize = 64;

    let mut queued = Vec::with_capacity(BATCH_SIZE);
    loop {
        queued.clear();
        if output.recv_many(&mut queued, BATCH_SIZE).await == 0 {
            break;
        }
        let mut events = Vec::with_capacity(queued.len());
        for event in queued.drain(..) {
            events.push(match event {
                DurableNodeEvent::Output { output, timestamp } => RunEvent::SafeLog {
                    execution: Some(execution),
                    timestamp,
                    stream: safe_log_stream(output.stream),
                    line: SafeLogLine::new(output.text)?,
                },
                DurableNodeEvent::TokenUsage(usage) => {
                    RunEvent::TokenUsageObserved { execution, usage }
                }
            });
        }
        ledger.append(&run_id, events).await?;
    }
    Ok(())
}

pub(super) const fn safe_log_stream(stream: LiveOutputStream) -> SafeLogStream {
    match stream {
        LiveOutputStream::Output => SafeLogStream::Output,
        LiveOutputStream::Error => SafeLogStream::Error,
        LiveOutputStream::System => SafeLogStream::System,
    }
}

pub(super) fn reduce(
    admitted: &AdmittedRun,
    snapshot: &RunSnapshot,
) -> Result<crate::full_v1_reducer::Reduction, NativeV2SupervisorError> {
    let executions = durable_history(snapshot)?;
    let verified = VerifiedGraph {
        compiled_ir: admitted.graph.clone(),
        diagnostics: Vec::new(),
    };
    Ok(FullV1Reducer::native_v2(&verified).reduce(ReductionInput {
        initial_input: &admitted.initial_input,
        executions: &executions,
        next_node_instance: next_node_instance(&executions)?,
        next_execution: next_execution(&executions)?,
    })?)
}

pub(crate) fn durable_history(
    snapshot: &RunSnapshot,
) -> Result<Vec<DurableExecution>, NativeV2SupervisorError> {
    let offset = snapshot
        .execution_seed
        .iter()
        .fold(0, |maximum, execution| {
            let end = match &execution.state {
                DurableExecutionState::Settled { position, .. }
                | DurableExecutionState::Voided { position, .. } => position.get(),
                DurableExecutionState::Active => execution.dispatch_position.get(),
            };
            maximum.max(end).max(execution.dispatch_position.get())
        });
    let mut executions = snapshot
        .executions
        .values()
        .map(|node| {
            let mut execution = durable_execution(node)?;
            execution.dispatch_position = offset_position(execution.dispatch_position, offset)?;
            match &mut execution.state {
                DurableExecutionState::Settled { position, .. }
                | DurableExecutionState::Voided { position, .. } => {
                    *position = offset_position(*position, offset)?;
                }
                DurableExecutionState::Active => {}
            }
            Ok(execution)
        })
        .collect::<Result<Vec<_>, NativeV2SupervisorError>>()?;
    executions.extend(snapshot.execution_seed.iter().cloned());
    executions.sort_by_key(|execution| (execution.dispatch_position, execution.execution));
    Ok(executions)
}

fn offset_position(
    position: HistoryPosition,
    offset: u64,
) -> Result<HistoryPosition, NativeV2SupervisorError> {
    position
        .get()
        .checked_add(offset)
        .and_then(|value| HistoryPosition::new(value).ok())
        .ok_or(NativeV2SupervisorError::InvalidState)
}

pub(super) fn durable_execution(
    node: &NodeSnapshot,
) -> Result<DurableExecution, NativeV2SupervisorError> {
    let state = match &node.state {
        NodeState::Active => DurableExecutionState::Active,
        NodeState::Completed { at, outcome } => DurableExecutionState::Settled {
            position: history_position(at)?,
            outcome: outcome.clone(),
        },
        NodeState::Voided { at, reason } => DurableExecutionState::Voided {
            position: history_position(at)?,
            reason: *reason,
        },
    };
    Ok(DurableExecution {
        dispatch_position: history_position(&node.started_at)?,
        node_instance: node.reference.node_instance,
        execution: node.reference.execution,
        occurrence: node.occurrence.clone(),
        attempt: node.attempt,
        input: node.input.clone(),
        state,
    })
}

pub(super) fn history_position(
    cursor: &openengine_cluster_protocol::Cursor,
) -> Result<HistoryPosition, NativeV2SupervisorError> {
    HistoryPosition::new(cursor_sequence(cursor)?)
        .map_err(|_| NativeV2SupervisorError::InvalidState)
}

pub(crate) fn next_node_instance(
    executions: &[DurableExecution],
) -> Result<u64, NativeV2SupervisorError> {
    executions
        .iter()
        .map(|execution| execution.node_instance.get())
        .max()
        .unwrap_or(FIRST_IDENTITY - 1)
        .checked_add(1)
        .ok_or(NativeV2SupervisorError::InvalidState)
}

pub(crate) fn next_execution(
    executions: &[DurableExecution],
) -> Result<u64, NativeV2SupervisorError> {
    executions
        .iter()
        .map(|execution| execution.execution.get())
        .max()
        .unwrap_or(FIRST_IDENTITY - 1)
        .checked_add(1)
        .ok_or(NativeV2SupervisorError::InvalidState)
}

pub(super) struct ExecutionCatalog {
    pub(super) timeouts: BTreeMap<NodeName, Option<Duration>>,
    pub(super) instructions: BTreeMap<NodeName, Option<NodeInstructions>>,
}

pub(super) fn execution_catalog(root: &GraphNode) -> ExecutionCatalog {
    let mut catalog = ExecutionCatalog {
        timeouts: BTreeMap::new(),
        instructions: BTreeMap::new(),
    };
    collect_execution_metadata(root, &mut catalog);
    catalog
}

fn collect_execution_metadata(node: &GraphNode, catalog: &mut ExecutionCatalog) {
    match node {
        GraphNode::Step(node) => record_execution_metadata(
            catalog,
            &node.name,
            node.timeout_ms.map(|value| value.get()),
            &node.instructions,
        ),
        GraphNode::Verifier(node) => record_execution_metadata(
            catalog,
            &node.name,
            node.timeout_ms.map(|value| value.get()),
            &node.instructions,
        ),
        GraphNode::Seq(node) => node
            .children
            .as_slice()
            .iter()
            .for_each(|child| collect_execution_metadata(child, catalog)),
        GraphNode::Choice(node) => {
            node.branches
                .as_slice()
                .iter()
                .for_each(|branch| collect_execution_metadata(&branch.node, catalog));
            if let Some(otherwise) = &node.otherwise {
                collect_execution_metadata(otherwise, catalog);
            }
        }
        GraphNode::Par(node) => node
            .branches
            .as_slice()
            .iter()
            .for_each(|branch| collect_execution_metadata(branch, catalog)),
        GraphNode::Loop(node) => collect_execution_metadata(&node.body, catalog),
        GraphNode::Map(node) => collect_execution_metadata(&node.body, catalog),
        GraphNode::Succeed(_) | GraphNode::Fail(_) => {}
    }
}

fn record_execution_metadata(
    catalog: &mut ExecutionCatalog,
    name: &NodeName,
    timeout_ms: Option<u64>,
    instructions: &Option<NodeInstructions>,
) {
    catalog
        .timeouts
        .insert(name.clone(), timeout_ms.map(Duration::from_millis));
    catalog
        .instructions
        .insert(name.clone(), instructions.clone());
}

pub(super) fn runner_failure(error: &NodeRunnerError) -> WorkerOutcome {
    let code = match error {
        NodeRunnerError::Cancelled | NodeRunnerError::RunClosed => WorkerErrorCode::Refusal,
        NodeRunnerError::InvalidRole
        | NodeRunnerError::SessionOpen
        | NodeRunnerError::SessionLost
        | NodeRunnerError::Driver
        | NodeRunnerError::DriverDetail(_)
        | NodeRunnerError::ConnectionLost
        | NodeRunnerError::CleanupUnconfirmed
        | NodeRunnerError::UnsafeOutput
        | NodeRunnerError::DurableOutputClosed
        | NodeRunnerError::CompletionClosed
        | NodeRunnerError::ExecutionActive => WorkerErrorCode::Crash,
    };
    WorkerOutcome::declared_failure(code)
}

pub(super) fn settled_outcome(
    reference: &ExecutionRef,
    result: DispatchResult,
    force: bool,
) -> Result<WorkerOutcome, NativeV2SupervisorError> {
    if force {
        return Ok(WorkerOutcome::declared_failure(WorkerErrorCode::Refusal));
    }
    match result {
        DispatchResult::Completed(Ok(completion)) if completion.reference == *reference => {
            Ok(completion.outcome)
        }
        DispatchResult::Completed(Ok(_)) => Err(NativeV2SupervisorError::InvalidState),
        DispatchResult::Completed(Err(error)) => Ok(runner_failure(&error)),
        DispatchResult::TimedOut => Ok(WorkerOutcome::declared_failure(WorkerErrorCode::Timeout)),
        DispatchResult::Interrupted => Ok(WorkerOutcome::declared_failure(WorkerErrorCode::Crash)),
        DispatchResult::DurableEventFailure(error) => Err(error.into()),
        DispatchResult::StartFailure(error) => Err(error),
    }
}
pub(super) fn settled_failure_cause(result: &DispatchResult, force: bool) -> Option<String> {
    if force {
        return None;
    }
    match result {
        DispatchResult::Completed(Err(error)) => Some(error.to_string()),
        _ => None,
    }
}
