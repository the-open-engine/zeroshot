use crate::source_code_provider::*;
use crate::native_v2_delivery::command::{ConfinedCommand, delivery_identity, identity_configuration};
use super::GitTransport;

pub(super) async fn bounded_status(
    mut command: ConfinedCommand,
) -> Result<(), SourceDeliveryError> {
    command.capture().await?.require_success()?;
    Ok(())
}

pub(super) async fn bounded_git_output(
    mut command: ConfinedCommand,
    maximum: usize,
) -> Result<String, SourceDeliveryError> {
    let output = command.capture_sensitive().await?.require_success()?;
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
    Ok(SourceTargetIntegration::Integrated {
        target_revision: target_revision.to_owned(),
        outcome,
    })
}

/// Integrates the captured target into a published review. The workspace must still descend from
/// the review head, and the review's fork point from the target bounds the deliverable mutation,
/// so a clean workspace at the review head is integrated instead of reporting no mutation.
pub(crate) async fn integrate_review_target(
    transport: &GitTransport,
    request: &SourceConflictRequest,
    target_revision: &str,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
    let review_head = &request.review.head_revision;
    if !valid_git_revision(target_revision) || !valid_git_revision(review_head) {
        return Err(SourceDeliveryError::Rejected);
    }
    transport.fetch_exact(target_revision, credential).await?;
    let mut provenance = transport.workspace_command(&request.workspace).await?;
    provenance.args(["merge-base", "--is-ancestor", review_head, "HEAD"]);
    bounded_status(provenance).await?;
    let mut fork_point = transport.workspace_command(&request.workspace).await?;
    fork_point.args(["merge-base", review_head, target_revision]);
    let fork_point = bounded_git_output(fork_point, 128).await?;
    let target = DeliveryTarget::new(
        &request.review.repository,
        &request.review.target_branch,
        fork_point.trim(),
    )
    .map_err(|_| SourceDeliveryError::Rejected)?;
    let request = SourceTargetReconciliation {
        workspace: &request.workspace,
        target: &target,
        commit_message: "Integrate the captured target before another review",
    };
    integrate(TargetContext { transport, request }, target_revision).await
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
    let mut provenance = local_command(context).await?;
    provenance.args(["merge-base", "--is-ancestor", source_revision, &head]);
    bounded_status(provenance).await?;
    if is_ancestor(context, target_revision, &head).await? {
        return Ok(SourceReconciliationOutcome::Unchanged);
    }
    let prepared = git
        .prepare_revision(
            context.request.workspace,
            source_revision,
            context.request.commit_message,
        )
        .await;
    let candidate = match prepared {
        Err(crate::native_v2_delivery::git::GitError::NoMutation)
            if context.transport.config().local_environment.is_some() =>
        {
            git.descendant_revision(context.request.workspace, source_revision)
                .await
                .map_err(git_error)?
        }
        result => result.map_err(git_error)?,
    };
    configure_delivery_identity(context.transport).await?;
    merge_target(context, target_revision, &candidate).await
}

async fn merge_target(
    context: TargetContext<'_>,
    target_revision: &str,
    candidate: &str,
) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
    let mut command = local_command(context).await?;
    let [name, email] = identity_configuration(context.transport.commit_identity());
    command.args([
        "-c",
        "rerere.enabled=false",
        "-c",
        &name,
        "-c",
        &email,
        "merge",
        "--ff",
        "--no-squash",
        "--commit",
        "--no-edit",
        "--no-verify",
        target_revision,
    ]);
    let output = command.capture().await?;
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
        let mut value = local_command(context).await?;
        value.args(["rev-parse", "--verify", reference]);
        let value = bounded_git_output(value, 128).await?;
        if value.trim() != expected {
            return Ok(false);
        }
    }
    let mut unmerged = local_command(context).await?;
    unmerged.args(["ls-files", "--unmerged", "-z"]);
    let output = unmerged.capture_sensitive().await?.require_success()?;
    Ok(!output.stdout().is_empty() || output.stdout_truncated())
}

async fn is_ancestor(
    context: TargetContext<'_>,
    ancestor: &str,
    descendant: &str,
) -> Result<bool, SourceDeliveryError> {
    let mut command = local_command(context).await?;
    command.args(["merge-base", "--is-ancestor", ancestor, descendant]);
    let output = command.capture().await?;
    if output.exit_status == Some(1) {
        return Ok(false);
    }
    output.require_success()?;
    Ok(true)
}

async fn local_command(context: TargetContext<'_>) -> Result<ConfinedCommand, GitCommandFailure> {
    context
        .transport
        .workspace_command(context.request.workspace)
        .await
}

pub(super) async fn configure_delivery_identity(
    transport: &GitTransport,
) -> Result<(), SourceDeliveryError> {
    // Local transport carries the invoking shell's context and uses a caller-owned checkout.
    // Its identity stays command-scoped, including in worktrees sharing that checkout's config.
    // Contained checkouts retain the identity handoff for subsequent conflict-repair commits.
    if transport.config().local_environment.is_some() {
        return Ok(());
    }
    let (name, email) = delivery_identity(transport.commit_identity());
    for (key, value) in [("user.name", name), ("user.email", email)] {
        let mut command = transport
            .workspace_command(&transport.config().workspace)
            .await?;
        command.args(["config", "--local", "--replace-all", key, value]);
        bounded_status(command).await?;
    }
    Ok(())
}
