use super::*;

pub(super) struct LocalExecutionTask {
    pub(super) handle: NodeHandle,
    pub(super) durable: crate::native_v2_runner::DurableOutput,
    pub(super) commands: watch::Receiver<bool>,
    pub(super) events: mpsc::Sender<CapsuleNodeEvent>,
    pub(super) terminal: oneshot::Sender<Vec<CapsuleNodeEvent>>,
    pub(super) state: Arc<Mutex<EndpointState>>,
    pub(super) reference: ExecutionRef,
    pub(super) done: watch::Sender<bool>,
}

enum LocalInput {
    Completion(Result<NodeCompletion, NodeRunnerError>),
    Output(Result<DurableNodeEvent, crate::native_v2_runner::AttachReceiveError>),
    Cancel,
    ConsumerGone,
}

struct LocalAwait {
    completion: bool,
    output: bool,
    command: bool,
}

struct LocalInputContext<'a> {
    handle: &'a mut NodeHandle,
    durable: &'a mut crate::native_v2_runner::DurableOutput,
    commands: &'a mut watch::Receiver<bool>,
    events: &'a mpsc::Sender<CapsuleNodeEvent>,
    awaiting: LocalAwait,
}

struct LocalOutputContext<'a> {
    events: &'a mpsc::Sender<CapsuleNodeEvent>,
    commands: &'a mut watch::Receiver<bool>,
    handle: &'a mut NodeHandle,
    output_closed: &'a mut bool,
    consumer_gone: &'a mut bool,
    cancelled: &'a mut bool,
    terminal_metadata: &'a mut TerminalMetadata,
}

#[derive(Default)]
pub(super) struct TerminalMetadata {
    known_usage: Option<TokenUsageDelta>,
    errors: Vec<CapsuleNodeEvent>,
    incomplete: bool,
    overflowed: bool,
    usage_overflowed: bool,
    diagnostics_incomplete: bool,
    last_failure_message: Option<String>,
}

impl TerminalMetadata {
    fn observe(&mut self, event: &CapsuleNodeEvent) {
        if let CapsuleNodeEvent::Output { output, .. } = event {
            if output.diagnostic == Some(FailureDiagnosticKind::Failure) {
                self.last_failure_message = Some(output.text.clone());
            }
        }
    }

    fn retain(&mut self, event: CapsuleNodeEvent) {
        let usage = match event {
            CapsuleNodeEvent::TokenUsage { usage } => usage,
            event @ CapsuleNodeEvent::Output {
                output:
                    CapsuleOutput {
                        stream: CapsuleOutputStream::Error,
                        ..
                    },
                ..
            } => {
                if self.errors.len() < crate::native_v2_runner::DURABLE_OUTPUT_CAPACITY {
                    self.observe(&event);
                    self.errors.push(event);
                } else {
                    self.diagnostics_incomplete = true;
                    self.overflowed = true;
                }
                return;
            }
            _ => return,
        };
        self.retain_usage(usage);
    }

    fn retain_usage(&mut self, usage: Option<TokenUsageDelta>) {
        let Some(usage) = usage else {
            self.incomplete = true;
            return;
        };
        if self.usage_overflowed {
            return;
        }
        self.known_usage = match self.known_usage {
            None => Some(usage),
            Some(total) => match TokenUsageDelta::checked_add(total, usage) {
                Some(total) => Some(total),
                None => {
                    self.incomplete = true;
                    self.usage_overflowed = true;
                    self.overflowed = true;
                    Some(total)
                }
            },
        };
    }

    pub(super) fn into_events(
        self,
        completion: Result<NodeCompletion, NodeRunnerError>,
    ) -> Vec<CapsuleNodeEvent> {
        let mut events = self.errors;
        events.reserve(4);
        if let Some(usage) = self.known_usage {
            events.push(CapsuleNodeEvent::TokenUsage { usage: Some(usage) });
        }
        if self.incomplete {
            events.push(CapsuleNodeEvent::TokenUsage { usage: None });
        }
        if self.diagnostics_incomplete {
            events.push(CapsuleNodeEvent::Output {
                output: LiveOutput::diagnostic(
                    FailureDiagnosticKind::Incomplete,
                    "Some diagnostic output was lost because the capsule terminal queue overflowed",
                )
                .into(),
                timestamp: crate::native_v2_runner::current_timestamp(),
            });
        }
        if let Err(error @ NodeRunnerError::DriverDetail(_)) = &completion {
            let diagnostic =
                LiveOutput::diagnostic(FailureDiagnosticKind::Failure, error.to_string());
            if self.last_failure_message.as_deref() != Some(diagnostic.text.as_str()) {
                events.push(CapsuleNodeEvent::Output {
                    output: diagnostic.into(),
                    timestamp: crate::native_v2_runner::current_timestamp(),
                });
            }
        }
        let terminal = if matches!(completion, Err(NodeRunnerError::CleanupUnconfirmed)) {
            CapsuleNodeEvent::Failed {
                failure: CapsuleNodeFailure::CleanupUnconfirmed,
            }
        } else if self.overflowed {
            CapsuleNodeEvent::Failed {
                failure: CapsuleNodeFailure::ExecutionFailed,
            }
        } else {
            match completion {
                Ok(completion) => CapsuleNodeEvent::Completed { completion },
                Err(error) => CapsuleNodeEvent::Failed {
                    failure: CapsuleNodeFailure::from_runner(&error),
                },
            }
        };
        events.push(terminal);
        events
    }
}

pub(super) async fn serve_local_execution(task: LocalExecutionTask) {
    let LocalExecutionTask {
        mut handle,
        mut durable,
        mut commands,
        events,
        terminal,
        state,
        reference,
        done,
    } = task;
    let mut completion = None;
    let mut output_closed = false;
    let mut consumer_gone = false;
    let mut cancelled = false;
    let mut terminal_metadata = TerminalMetadata::default();
    while completion.is_none() || !output_closed {
        let next = next_local_input(LocalInputContext {
            handle: &mut handle,
            durable: &mut durable,
            commands: &mut commands,
            events: &events,
            awaiting: LocalAwait {
                completion: completion.is_none(),
                output: !output_closed,
                command: !consumer_gone && !cancelled,
            },
        })
        .await;
        match next {
            LocalInput::Completion(result) => completion = Some(result),
            LocalInput::Output(output) => {
                apply_local_output(
                    output,
                    LocalOutputContext {
                        events: &events,
                        commands: &mut commands,
                        handle: &mut handle,
                        output_closed: &mut output_closed,
                        consumer_gone: &mut consumer_gone,
                        cancelled: &mut cancelled,
                        terminal_metadata: &mut terminal_metadata,
                    },
                )
                .await;
            }
            LocalInput::Cancel => {
                cancelled = true;
                handle.cancel();
            }
            LocalInput::ConsumerGone => {
                consumer_gone = true;
                cancelled = true;
                handle.cancel();
            }
        }
    }
    drop(events);
    send_local_completion(terminal, completion, consumer_gone, terminal_metadata);
    remove_endpoint_execution(&state, &reference, &done).await;
    let _ = done.send(true);
}

async fn next_local_input(context: LocalInputContext<'_>) -> LocalInput {
    tokio::select! {
        result = context.handle.completion(), if context.awaiting.completion => LocalInput::Completion(result),
        output = context.durable.recv(), if context.awaiting.output => LocalInput::Output(output),
        () = wait_for_signal(context.commands), if context.awaiting.command => LocalInput::Cancel,
        () = context.events.closed(), if context.awaiting.command => LocalInput::ConsumerGone,
    }
}

async fn apply_local_output(
    output: Result<DurableNodeEvent, crate::native_v2_runner::AttachReceiveError>,
    context: LocalOutputContext<'_>,
) {
    let Ok(output) = output else {
        *context.output_closed = true;
        return;
    };
    let event = match output {
        DurableNodeEvent::Output { output, timestamp } => CapsuleNodeEvent::Output {
            output: output.into(),
            timestamp,
        },
        DurableNodeEvent::TokenUsage(usage) => CapsuleNodeEvent::TokenUsage { usage },
    };
    if *context.consumer_gone {
        return;
    }
    if *context.cancelled {
        context.terminal_metadata.retain(event);
        return;
    }
    tokio::select! {
        biased;
        () = wait_for_signal(context.commands) => {
            *context.cancelled = true;
            context.handle.cancel();
            context.terminal_metadata.retain(event);
        }
        permit = context.events.reserve() => match permit {
            Ok(permit) => {
                context.terminal_metadata.observe(&event);
                permit.send(event);
            },
            Err(_) => {
                *context.consumer_gone = true;
                context.handle.cancel();
            }
        }
    }
}

fn send_local_completion(
    terminal: oneshot::Sender<Vec<CapsuleNodeEvent>>,
    completion: Option<Result<NodeCompletion, NodeRunnerError>>,
    consumer_gone: bool,
    metadata: TerminalMetadata,
) {
    if consumer_gone {
        return;
    }
    let Some(completion) = completion else {
        return;
    };
    let _ = terminal.send(metadata.into_events(completion));
}

pub(super) async fn remove_endpoint_execution(
    state: &Mutex<EndpointState>,
    reference: &ExecutionRef,
    done: &watch::Sender<bool>,
) {
    let expected = done.subscribe();
    let mut state = state.lock().await;
    if let Some(index) = state
        .active
        .iter()
        .position(|entry| &entry.reference == reference && entry.done.same_channel(&expected))
    {
        state.active.swap_remove(index);
    }
}

#[cfg(test)]
#[path = "execution/tests.rs"]
mod tests;
