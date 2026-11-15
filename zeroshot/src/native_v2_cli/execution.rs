use std::future::Future;
use std::io::Write;
use std::pin::Pin;

use openengine_cluster_protocol::{
    Cursor, RunAttachParams, RunCheckpointsParams, RunDiscardWorkspaceParams, RunForceParams,
    RunId, RunListParams, RunLogEventNotification, RunLogsParams, RunResumeParams, RunStatus,
    RunStatusParams, RunWatchParams, SubscriptionCloseReason, TerminalResult,
};
use serde::Serialize;

use super::{
    CliOutcome, CliRunStatus, CliRunWatchEventNotification, CliSubscription, CliSubscriptionItem,
    DetachSignal, NativeV2CliBackend, NativeV2CliCommand, NativeV2CliError, RunCommand,
    RunCheckpointsCommand, RunResumeCommand, RunLogsCommand, RunSelector, RunWatchCommand,
};
#[path = "execution/attach.rs"]
mod attach;
#[path = "execution/connections.rs"]
mod connections;
#[path = "execution/context.rs"]
mod context;
#[path = "execution/export_workspace.rs"]
mod export_workspace;
#[path = "execution/failure.rs"]
mod failure;
#[path = "graph_tree.rs"]
mod graph_tree;
#[path = "execution/profiles.rs"]
mod profiles;
#[path = "execution/reconnect.rs"]
mod reconnect;
#[path = "execution/sizes.rs"]
mod sizes;
#[path = "execution/stacks.rs"]
mod stacks;
#[path = "execution/status.rs"]
mod status;
#[path = "execution/submission.rs"]
mod submission;
use attach::{RoutedAttach, follow_attach};
pub(crate) use context::CliExecutionContext;
pub use failure::RunFailure;
pub use submission::{
    try_execute_native_v2_preflight, try_execute_native_v2_static,
    try_execute_native_v2_static_with_terminal,
};
use submission::{
    GitHubTokenScope, RunSubmission, read_github_token, select_connection_requirements, submit_run,
};
use status::outcome_for_status;
use reconnect::Reconnect;

pub async fn execute_native_v2_cli<B, S, W>(
    command: NativeV2CliCommand,
    backend: &B,
    signal: &mut S,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    S: DetachSignal,
    W: Write,
{
    if let Some(outcome) = try_execute_native_v2_preflight(&command, output).await? {
        return Ok(outcome);
    }
    let environment = |name: &str| std::env::var_os(name);
    let context = CliExecutionContext::new(backend, &environment);
    execute_native_v2_cli_with_context(command, &context, signal, output).await
}

pub(crate) async fn execute_native_v2_cli_with_context<B, S, W>(
    command: NativeV2CliCommand,
    context: &CliExecutionContext<'_, B>,
    signal: &mut S,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    S: DetachSignal,
    W: Write,
{
    if let Some(outcome) = try_execute_native_v2_static(&command, output)? {
        return Ok(outcome);
    }
    if command.is_stack_operation() {
        return stacks::execute(command, context, signal, output).await;
    }
    if command.is_management_operation() {
        return execute_management(command, context.backend, output).await;
    }
    match command {
        NativeV2CliCommand::Run(run) => execute_run(*run, context, signal, output).await,
        NativeV2CliCommand::Update
        | NativeV2CliCommand::TargetServe(_)
        | NativeV2CliCommand::Ui { .. } => Err(NativeV2CliError::ProcessCommand),
        command => execute_run_operation(command, context, signal, output).await,
    }
}

async fn execute_management<B: NativeV2CliBackend, W: Write>(
    command: NativeV2CliCommand,
    backend: &B,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError> {
    if command.is_connection_operation() {
        return connections::execute_connection(command, backend, output).await;
    }
    if command.is_profile_operation() {
        return profiles::execute_profile(command, backend, output).await;
    }
    match command {
        NativeV2CliCommand::Delegation(command) => {
            write_json(output, &backend.delegation(command).await?)?;
            Ok(CliOutcome::Completed)
        }
        NativeV2CliCommand::TargetSizes { name, json } => {
            sizes::execute(backend, &name, json, output).await
        }
        command => execute_target(command, backend).await,
    }
}

async fn execute_target<B>(
    command: NativeV2CliCommand,
    backend: &B,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
{
    match command {
        NativeV2CliCommand::TargetAdd(request) => backend.target_add(request).await?,
        NativeV2CliCommand::TargetLogin { name } => backend.target_login(&name).await?,
        _ => {
            return Err(NativeV2CliError::Usage(
                "expected a target operation".to_owned(),
            ));
        }
    }
    Ok(CliOutcome::Completed)
}

async fn execute_run_operation<B, S, W>(
    command: NativeV2CliCommand,
    context: &CliExecutionContext<'_, B>,
    signal: &mut S,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    S: DetachSignal,
    W: Write,
{
    let backend = context.backend;
    let command = match command {
        NativeV2CliCommand::Suspend(run) => {
            write_json(
                output,
                &backend
                    .run_suspend(
                        run.target.as_deref(),
                        openengine_cluster_protocol::RunSuspendParams { run_id: run.run_id },
                    )
                    .await?,
            )?;
            return Ok(CliOutcome::Completed);
        }
        NativeV2CliCommand::Vectors(command) => {
            write_json(
                output,
                &backend
                    .run_vectors(
                        command.run.target.as_deref(),
                        openengine_cluster_protocol::RunVectorsParams {
                            run_id: command.run.run_id,
                            after: command.after,
                            limit: command.limit,
                        },
                    )
                    .await?,
            )?;
            return Ok(CliOutcome::Completed);
        }
        NativeV2CliCommand::Vector { run, id } => {
            write_json(
                output,
                &backend
                    .run_vector(
                        run.target.as_deref(),
                        openengine_cluster_protocol::RunVectorParams {
                            run_id: run.run_id,
                            id,
                        },
                    )
                    .await?,
            )?;
            return Ok(CliOutcome::Completed);
        }
        NativeV2CliCommand::ForceStop(run) => {
            return execute_force_stop(run, backend, output).await;
        }
        NativeV2CliCommand::Answer {
            run,
            input,
            forward_github_token,
        } => {
            return execute_answer((run, input, forward_github_token), context, output).await;
        }
        NativeV2CliCommand::InboxList { target, params } => {
            write_json(
                output,
                &context
                    .backend
                    .inbox_list(target.as_deref(), params)
                    .await?,
            )?;
            return Ok(CliOutcome::Completed);
        }
        NativeV2CliCommand::InboxShow(run) => {
            write_json(
                output,
                &context
                    .backend
                    .inbox_show(
                        run.target.as_deref(),
                        openengine_cluster_protocol::InboxShowParams { run_id: run.run_id },
                    )
                    .await?,
            )?;
            return Ok(CliOutcome::Completed);
        }
        NativeV2CliCommand::Resume(run) => {
            return execute_resume(run, context, output).await;
        }
        NativeV2CliCommand::ExportWorkspace(command) => {
            return export_workspace::execute(command, backend, signal, output).await;
        }
        NativeV2CliCommand::Checkpoints(command) => {
            return execute_checkpoints(command, backend, output).await;
        }
        NativeV2CliCommand::DiscardWorkspace(run) => {
            return execute_discard_workspace(run, backend, output).await;
        }
        command => command,
    };
    if matches!(
        command,
        NativeV2CliCommand::List { .. } | NativeV2CliCommand::Status(_)
    ) {
        return execute_run_unary(command, backend, output).await;
    }
    execute_run_subscription(command, backend, signal, output).await
}

async fn execute_resume<B, W>(
    command: RunResumeCommand,
    context: &CliExecutionContext<'_, B>,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    W: Write,
{
    let run = &command.run;
    let status = context
        .backend
        .run_resume_status(
            run.target.as_deref(),
            RunStatusParams {
                run_id: run.run_id.clone(),
            },
        )
        .await?;
    let answering = matches!(
        command.from,
        Some(
            openengine_cluster_protocol::RunResumeFrom::Answer { .. }
                | openengine_cluster_protocol::RunResumeFrom::Expiry {}
        )
    );
    if let CliRunStatus::Target(RunStatus::Suspended {
        suspension: openengine_cluster_protocol::RunSuspension::Request { .. },
        ..
    }) = &status.status
    {
        if !answering {
            return Err(request_error(format!(
                "run requires zeroshot answer {} --input FILE",
                run.run_id.as_str()
            )));
        }
    }
    if let CliRunStatus::Target(RunStatus::Suspended { suspension, .. }) = &status.status {
        if !answering && (command.from.is_some() || command.discard_workspace_changes) {
            return Err(NativeV2CliError::Usage(
                "suspended runs accept only default resume".to_owned(),
            ));
        }
        if let Some(due) = suspension.due_at()
            && crate::native_v2_runner::current_timestamp() < due
        {
            return Err(NativeV2CliError::Usage(format!(
                "wait is due at {}",
                due.get()
            )));
        }
    }
    if let Some(successor) = &status.workspace_recovery.successor_run_id {
        return Err(NativeV2CliError::Usage(format!(
            "run already has successor {}",
            successor.as_str()
        )));
    }
    if !status.workspace_recovery.recoverable && !answering {
        return Err(NativeV2CliError::Usage(
            "run does not have a recoverable workspace".to_owned(),
        ));
    }
    context.backend.validate_resume_selection(&command).await?;
    let run = command.run;
    let requirements = context
        .backend
        .authorize_resume_connection_requirements(
            run.target.as_deref(),
            &run.run_id,
            status.workspace_recovery.connection_requirements,
        )
        .await?;
    let scope = GitHubTokenScope::new(run.target.is_some(), command.forward_github_token);
    // Only direct targets return retained requirements, and they need exactly the original values,
    // so a withheld GH_TOKEN would fail later without naming the flag.
    if scope == GitHubTokenScope::Withheld
        && requirements
            .values()
            .flatten()
            .any(|field| field.as_str() == "GH_TOKEN")
    {
        return Err(NativeV2CliError::Usage(
            "this run's retained connections need GH_TOKEN; pass --forward-gh-token to send it"
                .to_owned(),
        ));
    }
    let connections =
        select_connection_requirements(requirements, scope.outbound(context.environment))?;
    let github_token = if scope == GitHubTokenScope::Local {
        read_github_token(context.environment)?
    } else {
        let github_com = status
            .source
            .as_ref()
            .is_some_and(crate::native_v2_candidate::is_github_com_source);
        // The opt-in always requires a valid GH_TOKEN; only GitHub.com sources carry it in the
        // legacy request field.
        scope
            .forwarded_token(context.environment)?
            .filter(|_| github_com)
    };
    let result = context
        .backend
        .run_resume(
            run.target.as_deref(),
            RunResumeParams {
                run_id: run.run_id,
                successor_run_id: RunId::new(uuid::Uuid::now_v7().to_string()),
                from: command.from,
                connections,
                connection_resolver: None,
                github_token,
                git_credentials: None,
                commit_identity: None,
                commit_co_author: None,
            },
            command.discard_workspace_changes,
        )
        .await?;
    write_json(output, &result)?;
    Ok(CliOutcome::Completed)
}

async fn execute_checkpoints<B, W>(
    command: RunCheckpointsCommand,
    backend: &B,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    W: Write,
{
    let result = backend
        .run_checkpoints(
            command.run.target.as_deref(),
            RunCheckpointsParams {
                run_id: command.run.run_id,
                after: command.after,
                limit: command.limit,
            },
        )
        .await?;
    write_json(output, &result)?;
    Ok(CliOutcome::Completed)
}

async fn execute_discard_workspace<B, W>(
    run: RunSelector,
    backend: &B,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    W: Write,
{
    let result = backend
        .run_discard_workspace(
            run.target.as_deref(),
            RunDiscardWorkspaceParams { run_id: run.run_id },
        )
        .await?;
    write_json(output, &result)?;
    Ok(CliOutcome::Completed)
}

async fn execute_run_unary<B, W>(
    command: NativeV2CliCommand,
    backend: &B,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    W: Write,
{
    match command {
        NativeV2CliCommand::List { target } => {
            let result = backend
                .run_list(target.as_deref(), RunListParams::default())
                .await?;
            write_json(output, &result)?;
            Ok(CliOutcome::Completed)
        }
        NativeV2CliCommand::Status(run) => {
            let result = backend
                .run_status(
                    run.target.as_deref(),
                    RunStatusParams { run_id: run.run_id },
                )
                .await?;
            write_json(output, &result)?;
            Ok(outcome_for_status(&result.status))
        }
        _ => Err(NativeV2CliError::Usage(
            "expected a unary run operation".to_owned(),
        )),
    }
}

async fn execute_force_stop<B, W>(
    run: RunSelector,
    backend: &B,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    W: Write,
{
    let result = backend
        .run_force(run.target.as_deref(), RunForceParams { run_id: run.run_id })
        .await?;
    write_json(output, &result)?;
    Ok(outcome_for_status(&result.status))
}

async fn execute_run_subscription<B, S, W>(
    command: NativeV2CliCommand,
    backend: &B,
    signal: &mut S,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    S: DetachSignal,
    W: Write,
{
    match command {
        NativeV2CliCommand::Watch(RunWatchCommand { run, after }) => {
            let detach = signal.wait();
            tokio::pin!(detach);
            follow_run_result(
                DurableFollow {
                    backend,
                    target: run.target.as_deref(),
                    run_id: run.run_id,
                    kind: DurableFollowKind::Watch,
                    initial_cursor: after,
                    execution: None,
                },
                detach,
                output,
            )
            .await
        }
        NativeV2CliCommand::Logs(RunLogsCommand {
            run,
            after,
            execution,
        }) => {
            follow_durable(
                DurableFollow {
                    backend,
                    target: run.target.as_deref(),
                    run_id: run.run_id,
                    kind: DurableFollowKind::Logs,
                    initial_cursor: after,
                    execution,
                },
                signal,
                output,
            )
            .await
        }
        NativeV2CliCommand::Attach { run, execution } => {
            follow_attach(
                backend,
                RoutedAttach {
                    target: run.target.as_deref(),
                    params: RunAttachParams {
                        run_id: run.run_id,
                        execution,
                    },
                },
                signal,
                output,
            )
            .await
        }
        _ => Err(NativeV2CliError::Usage(
            "expected a subscription run operation".to_owned(),
        )),
    }
}

async fn execute_run<B, S, W>(
    run: RunCommand,
    context: &CliExecutionContext<'_, B>,
    signal: &mut S,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    S: DetachSignal,
    W: Write,
{
    let detach = signal.wait();
    tokio::pin!(detach);
    let (receipt, interrupted) = match submit_run(&run, context, detach.as_mut(), output).await? {
        RunSubmission::Interrupted => return Ok(CliOutcome::Detached),
        RunSubmission::Validated => {
            write_json(output, &serde_json::json!({ "valid": true }))?;
            return Ok(CliOutcome::Completed);
        }
        RunSubmission::Submitted {
            receipt,
            interrupted,
        } => (receipt, interrupted),
    };
    write_json(output, &receipt)?;
    if run.detach || interrupted {
        return Ok(CliOutcome::Detached);
    }
    follow_run_result(
        DurableFollow {
            backend: context.backend,
            target: run.target.as_deref(),
            run_id: receipt.run_id,
            kind: DurableFollowKind::Watch,
            initial_cursor: None,
            execution: None,
        },
        detach,
        output,
    )
    .await
}

/// Follows a run's watch and settles the run's result.
///
/// Foreground `run` and `watch` share this, so a failed run ends both as `RunFailed` after its
/// status events are written.
async fn follow_run_result<B, D, W>(
    follow: DurableFollow<'_, B>,
    mut detach: Pin<&mut D>,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    D: Future<Output = ()> + ?Sized,
    W: Write,
{
    let (backend, target, run_id) = (follow.backend, follow.target, follow.run_id.clone());
    let mut observed = failure::FailureOutput::new(output);
    let outcome = match follow_durable_with_detach(follow, detach.as_mut(), &mut observed).await? {
        // A runtime failure can close the watch before its terminal snapshot is durable, or
        // without one when persistence is unavailable, and a watch resumed after the terminal
        // cursor has no new event. Run status reports the result, and Ctrl-C still detaches
        // while it is read.
        CliOutcome::Completed => {
            let params = RunStatusParams {
                run_id: run_id.clone(),
            };
            tokio::select! {
                biased;
                () = detach.as_mut() => CliOutcome::Detached,
                result = backend.run_status(target, params) => {
                    let result = result?;
                    observed.observe(&result.status);
                    outcome_for_status(&result.status)
                }
            }
        }
        outcome => outcome,
    };
    if outcome == CliOutcome::Failed {
        let failure = if let Some(failure) = observed.context {
            RunFailure::from_context(run_id, target, failure)
        } else {
            RunFailure::read(backend, target, run_id, detach).await
        };
        return Err(NativeV2CliError::RunFailed(Box::new(failure)));
    }
    Ok(outcome)
}

#[derive(Clone, Copy)]
enum DurableFollowKind {
    Watch,
    Logs,
}

struct DurableFollow<'a, B> {
    backend: &'a B,
    target: Option<&'a str>,
    run_id: RunId,
    kind: DurableFollowKind,
    initial_cursor: Option<Cursor>,
    execution: Option<openengine_cluster_protocol::ExecutionRef>,
}

impl<B> DurableFollow<'_, B>
where
    B: NativeV2CliBackend,
{
    async fn open(
        &self,
        from_cursor: Option<Cursor>,
    ) -> Result<DurableSubscription<B::Watch, B::Logs>, NativeV2CliError> {
        match self.kind {
            DurableFollowKind::Watch => self
                .backend
                .run_watch(
                    self.target,
                    RunWatchParams {
                        run_id: self.run_id.clone(),
                        from_cursor,
                    },
                )
                .await
                .map(DurableSubscription::Watch),
            DurableFollowKind::Logs => self
                .backend
                .run_logs(
                    self.target,
                    RunLogsParams {
                        run_id: self.run_id.clone(),
                        from_cursor,
                        execution: self.execution.clone(),
                    },
                )
                .await
                .map(DurableSubscription::Logs),
        }
    }
}

enum DurableSubscription<W, L> {
    Watch(W),
    Logs(L),
}

impl<W, L> DurableSubscription<W, L>
where
    W: CliSubscription<CliRunWatchEventNotification>,
    L: CliSubscription<RunLogEventNotification>,
{
    async fn next(&mut self) -> Result<Option<DurableItem>, NativeV2CliError> {
        match self {
            Self::Watch(subscription) => subscription.next().await.map(|item| {
                item.map(|item| match item {
                    CliSubscriptionItem::Event(event) => DurableItem::Watch(Box::new(event)),
                    CliSubscriptionItem::Closed { reason } => DurableItem::Closed(reason),
                })
            }),
            Self::Logs(subscription) => subscription.next().await.map(|item| {
                item.map(|item| match item {
                    CliSubscriptionItem::Event(event) => DurableItem::Log(event),
                    CliSubscriptionItem::Closed { reason } => DurableItem::Closed(reason),
                })
            }),
        }
    }
}

enum DurableItem {
    Watch(Box<CliRunWatchEventNotification>),
    Log(RunLogEventNotification),
    Closed(SubscriptionCloseReason),
}

enum SubscriptionStep<E> {
    Detached,
    Reconnect(NativeV2CliError),
    Item(Option<E>),
}

async fn next_or_detach<E, N, D>(
    next: N,
    mut detach: Pin<&mut D>,
) -> Result<SubscriptionStep<E>, NativeV2CliError>
where
    N: Future<Output = Result<Option<E>, NativeV2CliError>>,
    D: Future<Output = ()> + ?Sized,
{
    tokio::select! {
        biased;
        () = detach.as_mut() => Ok(SubscriptionStep::Detached),
        item = next => match item {
            Ok(item) => Ok(SubscriptionStep::Item(item)),
            Err(error) if error.is_disconnected() => Ok(SubscriptionStep::Reconnect(error)),
            Err(error) => Err(error),
        },
    }
}

impl DurableItem {
    fn cursor(&self) -> Option<&Cursor> {
        match self {
            Self::Watch(event) => Some(&event.cursor),
            Self::Log(event) => Some(&event.cursor),
            Self::Closed(_) => None,
        }
    }
}

async fn follow_durable<B, S>(
    follow: DurableFollow<'_, B>,
    signal: &mut S,
    output: &mut impl Write,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    S: DetachSignal,
{
    let detach = signal.wait();
    tokio::pin!(detach);
    follow_durable_with_detach(follow, detach.as_mut(), output).await
}

async fn follow_durable_with_detach<B, D, W>(
    follow: DurableFollow<'_, B>,
    mut detach: Pin<&mut D>,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    D: Future<Output = ()> + ?Sized,
    W: DurableOutput,
{
    let mut from_cursor = follow.initial_cursor.clone();
    let mut opened = false;
    let mut reconnect = Reconnect::new(follow.target, &follow.run_id);
    loop {
        let subscription = tokio::select! {
            biased;
            () = detach.as_mut() => return Ok(CliOutcome::Detached),
            error = reconnect.expired() => return Err(error),
            result = follow.open(from_cursor.clone()) => match result {
                Ok(subscription) => {
                    opened = true;
                    reconnect.connected();
                    subscription
                }
                Err(error) if !opened => return Err(error),
                Err(error) => {
                    if reconnect.wait(error, detach.as_mut()).await? {
                        return Ok(CliOutcome::Detached);
                    }
                    continue;
                }
            },
        };
        let mut subscription = subscription;
        let error = loop {
            match next_or_detach(subscription.next(), detach.as_mut()).await? {
                SubscriptionStep::Detached => return Ok(CliOutcome::Detached),
                SubscriptionStep::Item(Some(DurableItem::Closed(
                    SubscriptionCloseReason::Done,
                ))) => {
                    return Ok(CliOutcome::Completed);
                }
                SubscriptionStep::Item(Some(DurableItem::Closed(
                    SubscriptionCloseReason::SlowConsumer,
                )))
                | SubscriptionStep::Item(None) => break NativeV2CliError::Disconnected,
                SubscriptionStep::Reconnect(error) => break error,
                SubscriptionStep::Item(Some(event)) => {
                    if let Some(outcome) = write_durable_event(event, &mut from_cursor, output)? {
                        return Ok(outcome);
                    }
                }
            }
        };
        if reconnect.wait(error, detach.as_mut()).await? {
            return Ok(CliOutcome::Detached);
        }
    }
}

/// Receives each new event of a durable follow once, in cursor order.
trait DurableOutput {
    fn watch_event(&mut self, event: CliRunWatchEventNotification) -> Result<(), NativeV2CliError>;
    fn log_event(&mut self, event: RunLogEventNotification) -> Result<(), NativeV2CliError>;
}

impl<W: Write> DurableOutput for W {
    fn watch_event(&mut self, event: CliRunWatchEventNotification) -> Result<(), NativeV2CliError> {
        write_json(self, &event)
    }

    fn log_event(&mut self, event: RunLogEventNotification) -> Result<(), NativeV2CliError> {
        write_json(self, &event)
    }
}

fn write_durable_event(
    event: DurableItem,
    from_cursor: &mut Option<Cursor>,
    output: &mut impl DurableOutput,
) -> Result<Option<CliOutcome>, NativeV2CliError> {
    let cursor = event.cursor().cloned();
    if cursor.is_some() && cursor == *from_cursor {
        return Ok(None);
    }
    let outcome = match event {
        DurableItem::Watch(event) => {
            let outcome = watched_outcome(&event.status);
            output.watch_event(*event)?;
            outcome
        }
        DurableItem::Log(event) => {
            output.log_event(event)?;
            None
        }
        DurableItem::Closed(SubscriptionCloseReason::SourceUnavailable) => {
            return Err(NativeV2CliError::Protocol(
                "observation source is unavailable; the stream is incomplete".to_owned(),
            ));
        }
        DurableItem::Closed(_) => None,
    };
    *from_cursor = cursor;
    Ok(outcome)
}

fn watched_outcome(status: &CliRunStatus) -> Option<CliOutcome> {
    match status {
        CliRunStatus::Target(RunStatus::Suspended { .. }) => Some(CliOutcome::Finished),
        CliRunStatus::Target(RunStatus::Finished {
            terminal_result: TerminalResult::Succeeded { .. },
            ..
        }) => Some(CliOutcome::Finished),
        CliRunStatus::Target(RunStatus::Finished {
            terminal_result: TerminalResult::Failed { .. },
            ..
        }) => Some(CliOutcome::Failed),
        _ => None,
    }
}

fn write_json(output: &mut impl Write, value: &impl Serialize) -> Result<(), NativeV2CliError> {
    serde_json::to_writer(&mut *output, value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
#[path = "execution/incomplete_stream_tests.rs"]
mod incomplete_stream_tests;

#[cfg(test)]
#[path = "execution/exact_miss_tests.rs"]
mod exact_miss_tests;

#[cfg(test)]
#[path = "execution/failure_tests.rs"]
mod failure_tests;

fn request_error(message: impl Into<String>) -> NativeV2CliError {
    NativeV2CliError::Remote {
        code: "invalid_request".to_owned(),
        message: message.into(),
        details: None,
    }
}

async fn execute_answer<B: NativeV2CliBackend, W: Write>(
    command: (RunSelector, std::path::PathBuf, bool),
    context: &CliExecutionContext<'_, B>,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError> {
    let (run, input, forward_github_token) = command;
    context
        .backend
        .validate_request_ask(run.target.as_deref())
        .await?;
    let status = context
        .backend
        .run_status(
            run.target.as_deref(),
            RunStatusParams {
                run_id: run.run_id.clone(),
            },
        )
        .await?;
    if let Some(successor) = status.workspace_recovery.successor_run_id {
        return Err(request_error(format!(
            "run already has successor {}",
            successor.as_str()
        )));
    }
    let CliRunStatus::Target(RunStatus::Suspended { suspension, .. }) = status.status else {
        return Err(request_error("answer requires a suspended run"));
    };
    let openengine_cluster_protocol::RunSuspension::Request { .. } = &suspension else {
        return Err(request_error(format!(
            "answer requires a request; suspension reason is {}",
            if suspension.due_at().is_some() {
                "wait"
            } else {
                "command"
            }
        )));
    };
    let (from, input_error) = match read_request_answer(&input) {
        Ok(from) => (from, None),
        Err(error) => (
            // Only the owning controller knows whether the deadline passed. An invalid
            // result lets it apply expiry even when the file cannot be parsed, and can
            // never answer a pending request because the output must be a record.
            openengine_cluster_protocol::RunResumeFrom::Answer {
                signals: Default::default(),
                output: serde_json::Value::Null,
            },
            Some(error),
        ),
    };
    let mut result = execute_resume(
        RunResumeCommand {
            run,
            from: Some(from),
            discard_workspace_changes: false,
            forward_github_token,
        },
        context,
        output,
    )
    .await;
    if let (Some(input_error), Err(NativeV2CliError::Remote { message, .. })) =
        (input_error, &mut result)
    {
        // Keep the controller's refusal (in particular expiry and its successor) first.
        message.push_str("; answer file: ");
        message.push_str(&input_error);
    }
    result
}

fn read_request_answer(
    input: &std::path::Path,
) -> Result<openengine_cluster_protocol::RunResumeFrom, String> {
    use std::io::Read;
    let file = std::fs::File::open(input)
        .map_err(|error| format!("$: cannot read answer file: {error}"))?;
    let mut bytes = Vec::new();
    file.take(openengine_cluster_protocol::MAX_VECTOR_RESPONSE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("$: cannot read answer file: {error}"))?;
    if bytes.len() > openengine_cluster_protocol::MAX_VECTOR_RESPONSE_BYTES {
        return Err("$: answer exceeds node result size limit".to_owned());
    }
    let answer = crate::native_v2_request::parse_answer(&bytes)?;
    let from = openengine_cluster_protocol::RunResumeFrom::Answer {
        signals: answer.signals,
        output: answer.output,
    };
    // A legal node completion includes this result plus its execution envelope. Refuse
    // necessarily oversized results without sending them through a smaller transport bound.
    if serde_json::to_vec(&from)
        .map_err(|error| format!("$: {error}"))?
        .len()
        > crate::v2_run_ledger::MAX_EVENT_BYTES
    {
        return Err("$: answer exceeds node result size limit".to_owned());
    }
    Ok(from)
}

#[cfg(test)]
#[path = "execution/request_tests.rs"]
mod request_tests;
