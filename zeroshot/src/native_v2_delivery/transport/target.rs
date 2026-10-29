use std::time::Duration;
use tokio::process::Command;
use crate::source_code_provider::*;
use crate::native_v2_delivery::command::{capture, capture_sensitive};
use super::GitTransport;

pub(super) async fn bounded_status(
    mut command: Command,
    deadline: Duration,
) -> Result<(), SourceDeliveryError> {
    capture(&mut command, deadline).await?.require_success()?;
    Ok(())
}

pub(super) async fn bounded_git_output(
    command: &mut Command,
    deadline: Duration,
    maximum: usize,
) -> Result<String, SourceDeliveryError> {
    let output = capture_sensitive(command, deadline, &[])
        .await?
        .require_success()?;
    if output.stdout_truncated() || output.stdout().len() > maximum {
        return Err(SourceDeliveryError::identity(
            "Git inspection exceeded its byte bound",
        ));
    }
    Ok(output.stdout().to_owned())
}

fn git_error(error: crate::native_v2_delivery::git::GitError) -> SourceDeliveryError {
    match error {
        crate::native_v2_delivery::git::GitError::Command(failure) => {
            SourceDeliveryError::Command(failure)
        }
        error => SourceDeliveryError::repairable(error.to_string()),
    }
}

#[derive(Clone, Copy)]
struct TargetContext<'a> {
    transport: &'a GitTransport,
    request: SourceTargetReconciliation<'a>,
}

pub(crate) async fn reconcile_target(
    transport: &GitTransport,
    request: SourceTargetReconciliation<'_>,
    target_revision: &str,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceTargetIntegration, SourceDeliveryError> {
    if !valid_git_revision(target_revision) {
        return Err(SourceDeliveryError::Rejected);
    }
    let context = TargetContext { transport, request };
    transport.fetch_exact(target_revision, credential).await?;
    let outcome = integrate(context, target_revision).await?;
    Ok(SourceTargetIntegration {
        target_revision: target_revision.to_owned(),
        outcome,
    })
}

async fn integrate(
    context: TargetContext<'_>,
    target_revision: &str,
) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
    let git = context.transport.workspace_git();
    let (head, _) = git
        .workspace_state(context.request.workspace)
        .await
        .map_err(git_error)?;
    let source_revision = &context.request.target.base_revision;
    let mut provenance = local_command(context);
    provenance.args(["merge-base", "--is-ancestor", source_revision, &head]);
    bounded_status(provenance, context.transport.config().deadline).await?;
    if is_ancestor(context, target_revision, &head).await? {
        return Ok(SourceReconciliationOutcome::Unchanged);
    }
    let candidate = git
        .prepare_revision(
            context.request.workspace,
            source_revision,
            context.request.commit_message,
        )
        .await
        .map_err(git_error)?;
    configure_delivery_identity(context.transport).await?;
    merge_target(context, target_revision, &candidate).await
}

async fn merge_target(
    context: TargetContext<'_>,
    target_revision: &str,
    candidate: &str,
) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
    let mut command = local_command(context);
    command.args([
        "-c",
        "rerere.enabled=false",
        "merge",
        "--ff",
        "--no-squash",
        "--commit",
        "--no-edit",
        "--no-verify",
        target_revision,
    ]);
    let output = capture(&mut command, context.transport.config().deadline).await?;
    match output.exit_status {
        Some(0) => require_completed_integration(context, target_revision, candidate).await?,
        Some(1) if has_exact_conflict(context, target_revision, candidate).await? => {}
        _ => return Err(output.into()),
    }
    Ok(SourceReconciliationOutcome::NeedsWork(format!(
        "trusted delivery fetched and integrated captured target revision {target_revision}; \
         inspect and test the resulting workspace, resolving any conflicts, before another delivery\n{output}",
    )))
}

async fn require_completed_integration(
    context: TargetContext<'_>,
    target_revision: &str,
    candidate: &str,
) -> Result<(), SourceDeliveryError> {
    let git = context.transport.workspace_git();
    let (head, dirty) = git
        .workspace_state(context.request.workspace)
        .await
        .map_err(git_error)?;
    if dirty
        || !is_ancestor(context, candidate, &head).await?
        || !is_ancestor(context, target_revision, &head).await?
    {
        return Err(SourceDeliveryError::repairable(
            "Git merge reported success without a clean, completed integration preserving both \
             the candidate and captured target ancestry; inspect the preserved workspace",
        ));
    }
    Ok(())
}

async fn has_exact_conflict(
    context: TargetContext<'_>,
    target_revision: &str,
    candidate: &str,
) -> Result<bool, SourceDeliveryError> {
    for (reference, expected) in [("HEAD", candidate), ("MERGE_HEAD", target_revision)] {
        let value = bounded_git_output(
            local_command(context).args(["rev-parse", "--verify", reference]),
            context.transport.config().deadline,
            128,
        )
        .await?;
        if value.trim() != expected {
            return Ok(false);
        }
    }
    let output = capture(
        local_command(context).args(["ls-files", "--unmerged", "-z"]),
        context.transport.config().deadline,
    )
    .await?
    .require_success()?;
    Ok(!output.stdout.is_empty() || output.stdout_truncated)
}

async fn is_ancestor(
    context: TargetContext<'_>,
    ancestor: &str,
    descendant: &str,
) -> Result<bool, SourceDeliveryError> {
    let output = capture(
        local_command(context).args(["merge-base", "--is-ancestor", ancestor, descendant]),
        context.transport.config().deadline,
    )
    .await?;
    if output.exit_status == Some(1) {
        return Ok(false);
    }
    output.require_success()?;
    Ok(true)
}

fn local_command(context: TargetContext<'_>) -> Command {
    context.transport.command()
}

pub(super) async fn configure_delivery_identity(
    transport: &GitTransport,
) -> Result<(), SourceDeliveryError> {
    for (key, value) in [
        ("user.name", "Zeroshot"),
        ("user.email", "delivery@zeroshot.invalid"),
    ] {
        let mut command = transport.command();
        command.args(["config", "--local", "--replace-all", key, value]);
        bounded_status(command, transport.config().deadline).await?;
    }
    Ok(())
}
