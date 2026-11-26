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
#[path = "execution/resume_selection.rs"]
mod resume_selection;
#[path = "execution/sizes.rs"]
mod sizes;
#[path = "execution/stacks.rs"]
mod stacks;
#[path = "execution/status.rs"]
mod status;
#[path = "execution/steering.rs"]
mod steering;
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
        NativeV2CliCommand::File(_)
        | NativeV2CliCommand::Update
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
        NativeV2CliCommand::TargetList { json } => {
            let targets = backend.target_list().await?;
            if json {
                write_json(output, &targets)?;
            } else {
                writeln!(output, "Name\tKind\tOrigin")?;
                for target in targets {
                    let kind = match target.kind {
                        super::NamedTargetKind::Hosted => "hosted",
                        super::NamedTargetKind::Direct => "direct",
                    };
                    writeln!(output, "{}\t{kind}\t{}", target.name, target.origin)?;
                }
            }
            Ok(CliOutcome::Completed)
        }
        NativeV2CliCommand::TargetRemove { name, json } => {
            let receipt = backend.target_remove(&name).await?;
            if json {
                write_json(output, &receipt)?;
            } else {
                writeln!(output, "Removed target {:?}.", receipt.name)?;
            }
            Ok(CliOutcome::Completed)
        }
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
        command @ NativeV2CliCommand::Steer { .. } => {
            return steering::execute(command, context, output).await;
        }
        NativeV2CliCommand::Suspend(run) => {
            write_json(
                output,
                &backend
                    .run_suspend(
                        run.target.as_deref(),
                        openengine_cluster_protocol::RunSuspendParams {
                            part_id: run.part_id,
                            run_id: run.run_id,
                        },
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
                            part_id: command.run.part_id.clone(),
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
                            part_id: run.part_id.clone(),
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
            return execute_answer((run, input, forward_github_token), context, signal, output)
                .await;
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
            let request = context
                .backend
                .inbox_show(
                    run.target.as_deref(),
                    openengine_cluster_protocol::InboxShowParams {
                        part_id: run.part_id,
                        run_id: run.run_id,
                    },
                )
                .await?;
            write_json(output, &request)?;
            return Ok(CliOutcome::Completed);
        }
        NativeV2CliCommand::Resume(run) => {
            return execute_resume(run, context, signal, output).await;
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

fn prepare_resume_runtime(
    command: &RunResumeCommand,
) -> Result<Option<openengine_cluster_protocol::RunResumeRuntime>, NativeV2CliError> {
    let mut runtime = command
        .runtime
        .as_ref()
        .map(|source| match source {
            super::RunRuntime::Exact(path) => submission::read_json("runtime config", path)
                .map(|runtime| openengine_cluster_protocol::RunResumeRuntime::Exact { runtime }),
            super::RunRuntime::Uniform(path) => {
                submission::read_json("uniform runtime config", path).map(|runtime| {
                    openengine_cluster_protocol::RunResumeRuntime::Uniform { runtime }
                })
            }
        })
        .transpose()?;
    if let Some(runtime) = runtime.as_mut() {
        let placement = if command.run.target.is_some() {
            crate::native_v2_candidate::ProviderAccessPlacement::Contained
        } else {
            crate::native_v2_candidate::ProviderAccessPlacement::Local
        };
        crate::native_v2_runtime_config::materialize_access(runtime, placement)
            .map_err(|error| NativeV2CliError::Usage(error.to_string()))?;
    }
    Ok(runtime)
}

fn check_suspended_resume(
    command: &RunResumeCommand,
    suspension: &openengine_cluster_protocol::RunSuspension,
    answering: bool,
) -> Result<(), NativeV2CliError> {
    if !answering {
        if matches!(
            suspension,
            openengine_cluster_protocol::RunSuspension::Request { .. }
        ) {
            return Err(request_error(format!(
                "run requires zeroshot answer {} --input FILE",
                command.run.run_id.as_str()
            )));
        }
        if command.from.is_some() || command.discard_workspace_changes {
            return Err(NativeV2CliError::Usage(
                "suspended runs accept only default resume".to_owned(),
            ));
        }
    }
    if let Some(due) = suspension.due_at()
        && crate::native_v2_runner::current_timestamp() < due
    {
        return Err(NativeV2CliError::Usage(format!(
            "wait is due at {}",
            due.get()
        )));
    }
    Ok(())
}

async fn execute_resume<B, S, W>(
    mut command: RunResumeCommand,
    context: &CliExecutionContext<'_, B>,
    signal: &mut S,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    S: DetachSignal,
    W: Write,
{
    let runtime = prepare_resume_runtime(&command)?;
    if let Some(runtime) = &runtime {
        context
            .backend
            .validate_resume_runtime(
                &command.run,
                runtime,
                command.run.target.is_none() && read_github_token(context.environment)?.is_some(),
            )
            .await?;
    }
    let run = &command.run;
    let status = context
        .backend
        .run_resume_status(
            run.target.as_deref(),
            RunStatusParams {
                part_id: run.part_id.clone(),
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
    if run
        .part_id
        .as_ref()
        .is_some_and(|part| part != &status.current_part_id)
    {
        return Err(stale_part(&status.current_part_id));
    }
    if let Some(successor) = &status.workspace_recovery.successor_run_id {
        return Err(NativeV2CliError::Usage(format!(
            "run already has successor {}",
            successor.as_str()
        )));
    }
    if let CliRunStatus::Target(RunStatus::Suspended { suspension, .. }) = &status.status {
        check_suspended_resume(&command, suspension, answering)?;
    }
    if !status.workspace_recovery.recoverable && !answering {
        return Err(NativeV2CliError::Usage(
            "run does not have a recoverable workspace".to_owned(),
        ));
    }
    let root = status.run_id.clone();
    command.run.part_id = Some(status.current_part_id.clone());
    resume_selection::apply_default(&mut command, &status.status, context.backend).await?;
    command.run.run_id = status.current_part_id.clone();
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
    let requirements = if let Some(runtime) = runtime.as_ref() {
        context
            .backend
            .replacement_resume_requirements(
                &run,
                runtime,
                run.target.is_none() && read_github_token(context.environment)?.is_some(),
            )
            .await?
    } else {
        requirements
    };
    let scope = GitHubTokenScope::new(run.target.is_some(), command.forward_github_token);
    // Direct targets require the effective authorized declarations. Name the forwarding
    // flag before selecting credentials when those declarations include GH_TOKEN.
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
    let selected_checkpoint = command.from.as_ref().and_then(|from| match from {
        openengine_cluster_protocol::RunResumeFrom::Checkpoint { checkpoint_id } => {
            Some(checkpoint_id.clone())
        }
        _ => None,
    });
    let result = context
        .backend
        .run_resume(
            run.target.as_deref(),
            RunResumeParams {
                runtime,
                part_id: status.current_part_id,
                run_id: root,
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
    if let Some(checkpoint) = selected_checkpoint {
        eprintln!("Selected checkpoint {checkpoint}. The selected node can run again.");
    }
    write_json(output, &result)?;
    let detach = signal.wait();
    tokio::pin!(detach);
    RunFailure::read(
        context.backend,
        run.target.as_deref(),
        RunStatusParams {
            run_id: result.run_id,
            part_id: Some(result.current_part_id),
        },
        detach.as_mut(),
    )
    .await
    .report_memory(false);
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
                part_id: command.run.part_id.clone(),
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
            RunDiscardWorkspaceParams {
                part_id: run.part_id,
                run_id: run.run_id,
            },
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
                    RunStatusParams {
                        part_id: run.part_id,
                        run_id: run.run_id,
                    },
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
        .run_force(
            run.target.as_deref(),
            RunForceParams {
                part_id: run.part_id,
                run_id: run.run_id,
            },
        )
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
        NativeV2CliCommand::Watch(RunWatchCommand { run, after, follow }) => {
            let detach = signal.wait();
            tokio::pin!(detach);
            follow_run_result(
                DurableFollow {
                    part_id: run.part_id,
                    backend,
                    target: run.target.as_deref(),
                    run_id: run.run_id,
                    kind: if follow {
                        DurableFollowKind::ContinuousWatch
                    } else {
                        DurableFollowKind::Watch
                    },
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
                    part_id: run.part_id,
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
                        part_id: run.part_id.clone(),
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
            part_id: None,
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
    let part_id = follow.part_id.clone();
    let mut observed = failure::FailureOutput::new(output);
    let outcome = match follow_durable_with_detach(follow, detach.as_mut(), &mut observed).await? {
        // A runtime failure can close the watch before its terminal snapshot is durable, or
        // without one when persistence is unavailable, and a watch resumed after the terminal
        // cursor has no new event. Run status reports the result, and Ctrl-C still detaches
        // while it is read.
        CliOutcome::Completed => {
            let params = RunStatusParams {
                part_id: part_id.clone(),
                run_id: run_id.clone(),
            };
            tokio::select! {
                biased;
                () = detach.as_mut() => CliOutcome::Detached,
                result = backend.run_status(target, params) => {
                    let result = result?;
                    observed.origin = Some((result.run_id, result.part_id));
                    observed.observe(&result.status);
                    outcome_for_status(&result.status)
                }
            }
        }
        outcome => outcome,
    };
    // Auxiliary memory errors never populate the primary failure context. Read their existing
    // log stream for successful runs too, including recall-only runs and resumed controllers.
    let (run_id, part_id) = observed
        .origin
        .take()
        .map_or((run_id, part_id), |(run, part)| (run, Some(part)));
    let logs = if matches!(outcome, CliOutcome::Finished | CliOutcome::Failed) {
        let logs = RunFailure::read(
            backend,
            target,
            RunStatusParams {
                run_id: run_id.clone(),
                part_id,
            },
            detach.as_mut(),
        )
        .await;
        logs.report_memory(true);
        Some(logs)
    } else {
        None
    };
    if outcome == CliOutcome::Failed {
        let failure = if let Some(failure) = observed.context {
            RunFailure::from_context(run_id, target, failure)
        } else {
            logs.expect("finished run logs were read")
        };
        return Err(NativeV2CliError::RunFailed(Box::new(failure)));
    }
    Ok(outcome)
}

#[derive(Clone, Copy)]
enum DurableFollowKind {
    Watch,
    ContinuousWatch,
    Logs,
}

struct DurableFollow<'a, B> {
    part_id: Option<RunId>,
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
    /// A pinned stream closes when its ledger has been drained. Use that for a
    /// foreground reconnect into the currently suspended part, even when its
    /// last status notification precedes the final ledger cursor.
    async fn suspended_replay(&self) -> Result<Option<RunId>, NativeV2CliError> {
        if !matches!(self.kind, DurableFollowKind::Watch) || self.part_id.is_some() {
            return Ok(None);
        }
        let Some((part, _)) = self
            .initial_cursor
            .as_ref()
            .and_then(crate::native_v2_observability::lineage::split_cursor)
        else {
            return Ok(None);
        };
        let head = self
            .backend
            .run_status(self.target, RunStatusParams::new(self.run_id.clone()))
            .await?;
        Ok((head.current_part_id == part
            && matches!(
                head.status,
                CliRunStatus::Target(RunStatus::Suspended { .. })
            ))
        .then_some(part))
    }

    async fn boundary(
        &self,
        outcome: Option<CliOutcome>,
        cursor: Option<&Cursor>,
    ) -> Result<Option<CliOutcome>, NativeV2CliError> {
        if outcome.is_none() || matches!(self.kind, DurableFollowKind::ContinuousWatch) {
            return Ok(None);
        }
        let Some((part, _)) =
            cursor.and_then(crate::native_v2_observability::lineage::split_cursor)
        else {
            return Ok(outcome);
        };
        if self.part_id.is_some() {
            return Ok(outcome);
        }
        let head = self
            .backend
            .run_status(self.target, RunStatusParams::new(self.run_id.clone()))
            .await?;
        Ok(outcome.filter(|_| head.current_part_id == part))
    }

    async fn open(
        &self,
        from_cursor: Option<Cursor>,
    ) -> Result<DurableSubscription<B::Watch, B::Logs>, NativeV2CliError> {
        match self.kind {
            DurableFollowKind::Watch | DurableFollowKind::ContinuousWatch => self
                .backend
                .run_watch(
                    self.target,
                    RunWatchParams {
                        part_id: self.part_id.clone(),
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
                        part_id: self.part_id.clone(),
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
    mut follow: DurableFollow<'_, B>,
    mut detach: Pin<&mut D>,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    D: Future<Output = ()> + ?Sized,
    W: DurableOutput,
{
    let mut from_cursor = follow.initial_cursor.clone();
    let suspended = tokio::select! {
        biased;
        () = detach.as_mut() => return Ok(CliOutcome::Detached),
        part = follow.suspended_replay() => part?,
    };
    if let Some(part) = &suspended {
        follow.part_id = Some(part.clone());
    }
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
                SubscriptionStep::Item(Some(mut event)) => {
                    if let (Some(part), DurableItem::Watch(notification)) = (&suspended, &mut event)
                    {
                        notification.cursor = crate::native_v2_observability::lineage::part_cursor(
                            part,
                            &notification.cursor,
                        );
                    }
                    let outcome = write_durable_event(event, &mut from_cursor, output)?;
                    let boundary = tokio::select! {
                        biased;
                        () = detach.as_mut() => return Ok(CliOutcome::Detached),
                        result = follow.boundary(outcome, from_cursor.as_ref()) => result?,
                    };
                    if let Some(outcome) = boundary {
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

pub(super) fn stale_part(current: &RunId) -> NativeV2CliError {
    NativeV2CliError::Remote {
        code: openengine_cluster_protocol::RUN_CONFLICT.into(),
        message: format!("the current run part is {}", current.as_str()),
        details: Some(serde_json::json!({"currentPartId":current})),
    }
}

async fn execute_answer<B: NativeV2CliBackend, S: DetachSignal, W: Write>(
    command: (RunSelector, std::path::PathBuf, bool),
    context: &CliExecutionContext<'_, B>,
    signal: &mut S,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError> {
    let (mut run, input, forward_github_token) = command;
    let part = answer_part(context.backend, &run).await;
    let answer = read_request_answer(&input);
    run.part_id = Some(match part {
        Ok(part) => part,
        Err(error) => {
            if let Ok(openengine_cluster_protocol::RunResumeFrom::Answer { output, .. }) = &answer {
                let _ = context
                    .backend
                    .release_rejected_answer_uploads(&run, output)
                    .await;
            }
            return Err(error);
        }
    });
    let (from, input_error) = match answer {
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
            runtime: None,
            run,
            from: Some(from),
            api_default: false,
            discard_workspace_changes: false,
            forward_github_token,
        },
        context,
        signal,
        output,
    )
    .await;
    if let (Some(input_error), Err(NativeV2CliError::Remote { message, .. })) =
        (input_error, &mut result)
    {
        // A path names only the placeholder's shape; any other refusal (in particular expiry and
        // its successor) stays first.
        if message.starts_with('$') {
            *message = format!("answer file: {input_error}");
        } else {
            message.push_str("; answer file: ");
            message.push_str(&input_error);
        }
    }
    result
}

async fn answer_part<B: NativeV2CliBackend>(
    backend: &B,
    run: &RunSelector,
) -> Result<RunId, NativeV2CliError> {
    backend.validate_request_ask(run.target.as_deref()).await?;
    let status = backend
        .run_status(
            run.target.as_deref(),
            RunStatusParams {
                part_id: run.part_id.clone(),
                run_id: run.run_id.clone(),
            },
        )
        .await?;
    if status.part_id != status.current_part_id {
        return Err(stale_part(&status.current_part_id));
    }
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
    Ok(status.part_id)
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

#[cfg(test)]
#[path = "execution/steer_tests.rs"]
mod steer_tests;

fn read_steer_text(source: super::SteerText) -> Result<String, NativeV2CliError> {
    use std::io::Read;
    let limit = openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES;
    let text = match source {
        super::SteerText::Text(text) => text,
        source => {
            let reader: Box<dyn Read> = match source {
                super::SteerText::File(path) => {
                    Box::new(std::fs::File::open(path).map_err(|error| {
                        NativeV2CliError::Usage(format!("cannot read steer text file: {error}"))
                    })?)
                }
                _ => Box::new(std::io::stdin()),
            };
            let mut bytes = Vec::new();
            reader
                .take(limit as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| {
                    NativeV2CliError::Usage(format!("cannot read steer text: {error}"))
                })?;
            if bytes.len() > limit {
                return Err(NativeV2CliError::Usage(format!(
                    "steer exceeds {limit} UTF-8 bytes"
                )));
            }
            String::from_utf8(bytes)
                .map_err(|_| NativeV2CliError::Usage("steer must be UTF-8".into()))?
        }
    };
    openengine_cluster_protocol::validate_steer(&text).map_err(NativeV2CliError::Usage)?;
    Ok(text)
}
