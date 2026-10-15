//! Preserve candidate work while integrating an exact target before first publication.
use super::*;
use super::wire::{GitReferenceWire, reference_revision};

#[derive(Clone, Copy)]
struct TargetContext<'a> {
    authority: &'a GhCliDeliveryAuthority,
    request: SourceTargetReconciliation<'a>,
}

pub(super) async fn reconcile(
    authority: &GhCliDeliveryAuthority,
    request: SourceTargetReconciliation<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceTargetIntegration, SourceDeliveryError> {
    let context = TargetContext { authority, request };
    let target_revision = observe_target(context, credential).await?;
    fetch_target(context, &target_revision, credential).await?;
    let outcome = integrate(context, &target_revision).await?;
    Ok(SourceTargetIntegration {
        target_revision,
        outcome,
    })
}

async fn observe_target(
    context: TargetContext<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<String, SourceDeliveryError> {
    let target = context.request.target;
    let value = context
        .authority
        .api(
            &[format!(
                "repos/{}/git/ref/heads/{}",
                target.repository, target.target_branch
            )],
            credential,
        )
        .await?;
    let reference: GitReferenceWire = super::api::decode_response(value, credential)?;
    reference_revision(reference, &target.target_branch)
}

async fn fetch_target(
    context: TargetContext<'_>,
    target_revision: &str,
    credential: SourceDeliveryCredential<'_>,
) -> Result<(), SourceDeliveryError> {
    context
        .authority
        .git_transport(
            context.request.workspace,
            &context.request.target.repository,
        )
        .fetch_exact(
            target_revision,
            credential.with_github_git_token(credential.expose()),
        )
        .await
}

async fn integrate(
    context: TargetContext<'_>,
    target_revision: &str,
) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
    let git = context.authority.workspace_git();
    let (head, _) = git
        .workspace_state(context.request.workspace)
        .await
        .map_err(git_error)?;
    let source_revision = &context.request.target.base_revision;
    let mut provenance = local_command(context);
    provenance.args(["merge-base", "--is-ancestor", source_revision, &head]);
    bounded_status(provenance, context.authority.config.api_deadline).await?;
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
    configure_delivery_identity(&context.authority.config, context.request.workspace).await?;
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
    let output = capture(&mut command, context.authority.config.api_deadline).await?;
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
    let git = context.authority.workspace_git();
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
            context.authority.config.api_deadline,
            128,
        )
        .await?;
        if value.trim() != expected {
            return Ok(false);
        }
    }
    let output = capture(
        local_command(context).args(["ls-files", "--unmerged", "-z"]),
        context.authority.config.api_deadline,
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
        context.authority.config.api_deadline,
    )
    .await?;
    if output.exit_status == Some(1) {
        return Ok(false);
    }
    output.require_success()?;
    Ok(true)
}

fn local_command(context: TargetContext<'_>) -> Command {
    local_git_command(&context.authority.config, context.request.workspace)
}

#[cfg(all(test, unix))]
#[path = "target_tests.rs"]
mod tests;
