use super::{GitTransport, SourceDeliveryCredential, SourceDeliveryError};
use crate::source_code_provider::{
    SourceHeadReconciliation, SourceReviewReceipt, SourceReconciliationOutcome,
};
use crate::native_v2_delivery::{command::GitCommandOutput, git::GitError};
use std::ffi::OsString;

#[derive(Clone, Copy)]
struct ReconciliationContext<'a> {
    transport: &'a GitTransport,
    credential: SourceDeliveryCredential<'a>,
}

impl ReconciliationContext<'_> {
    async fn capture(self, arguments: &[&str]) -> Result<GitCommandOutput, SourceDeliveryError> {
        let arguments = arguments.iter().map(OsString::from).collect::<Vec<_>>();
        self.transport
            .execute(
                &arguments,
                self.credential,
                self.transport.config().deadline,
            )
            .await
            .map_err(Into::into)
    }
}

fn git_error(error: GitError) -> SourceDeliveryError {
    match error {
        GitError::Command(failure) => SourceDeliveryError::Command(failure),
        error => SourceDeliveryError::repairable(error.to_string()),
    }
}

pub(crate) async fn reconcile(
    transport: &GitTransport,
    request: SourceHeadReconciliation<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
    require_identity(request.published, request.observed)?;
    let context = ReconciliationContext {
        transport,
        credential,
    };
    transport
        .fetch_exact(&request.observed.head_revision, credential)
        .await?;
    if !request.adopting_existing
        && !is_ancestor(
            context,
            &request.published.head_revision,
            &request.observed.head_revision,
        )
        .await?
    {
        return Ok(SourceReconciliationOutcome::Refused(format!(
            "remote history was rewritten: published head {}; observed head {}; local work was preserved",
            request.published.head_revision, request.observed.head_revision,
        )));
    }
    reconcile_workspace(context, request).await
}

async fn reconcile_adopted_delivery(
    context: ReconciliationContext<'_>,
    request: SourceHeadReconciliation<'_>,
    local_head: String,
) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
    if !is_ancestor(context, &local_head, &request.observed.head_revision).await? {
        return Ok(SourceReconciliationOutcome::Refused(format!(
            "existing remote head {} does not descend from retained local head {}; local work was preserved",
            request.observed.head_revision, local_head,
        )));
    }
    let mut anchor = request.observed.clone();
    anchor.head_revision = local_head;
    Box::pin(reconcile_workspace(
        context,
        SourceHeadReconciliation {
            published: &anchor,
            adopting_existing: false,
            ..request
        },
    ))
    .await
}

fn require_identity(
    published: &SourceReviewReceipt,
    observed: &SourceReviewReceipt,
) -> Result<(), SourceDeliveryError> {
    let bound_review_changed =
        !published.review_id.is_empty() && published.review_id != observed.review_id;
    if bound_review_changed
        || published.repository != observed.repository
        || published.target_branch != observed.target_branch
        || published.head_branch != observed.head_branch
    {
        return Err(SourceDeliveryError::identity(format!(
            "published identity {published:?}; observed identity {observed:?}",
        )));
    }
    Ok(())
}

fn authorizes_fast_forward(
    request: &SourceHeadReconciliation<'_>,
    head: &str,
    dirty: bool,
) -> bool {
    request.authorized_update && !dirty && head == request.published.head_revision
}

async fn reconcile_workspace(
    context: ReconciliationContext<'_>,
    request: SourceHeadReconciliation<'_>,
) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
    let git = context.transport.workspace_git();
    let (head, dirty) = git
        .workspace_state(context.transport.config().workspace.as_path())
        .await
        .map_err(git_error)?;
    if is_ancestor(context, &request.observed.head_revision, &head).await? {
        return Ok(SourceReconciliationOutcome::Unchanged);
    }
    if request.adopting_existing {
        return reconcile_adopted_delivery(context, request, head).await;
    }
    if !is_ancestor(context, &request.published.head_revision, &head).await? {
        return Ok(SourceReconciliationOutcome::Refused(format!(
            "local history no longer contains published head {}; local head {head}; \
             observed head {}; work was preserved",
            request.published.head_revision, request.observed.head_revision,
        )));
    }
    if dirty {
        git.prepare_revision(
            context.transport.config().workspace.as_path(),
            &request.published.head_revision,
            request.commit_message,
        )
        .await
        .map_err(git_error)?;
    }
    let authorized = authorizes_fast_forward(&request, &head, dirty);
    integrate(context, request, authorized).await
}

async fn integrate(
    context: ReconciliationContext<'_>,
    request: SourceHeadReconciliation<'_>,
    authorized: bool,
) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
    let mut arguments = vec![
        "-c",
        "user.name=Zeroshot",
        "-c",
        "user.email=delivery@zeroshot.invalid",
        "merge",
        "--no-edit",
    ];
    if authorized {
        arguments.push("--ff-only");
    }
    arguments.push(&request.observed.head_revision);
    let output = context.capture(&arguments).await?;
    let diagnostic = format!(
        "trusted delivery fetched remote head {} and reconciled published head {} with local work; \
         inspect the resulting workspace before another delivery\n{output}",
        request.observed.head_revision, request.published.head_revision,
    );
    if output.diagnostic().exit_status == Some(0) {
        return Ok(if authorized {
            SourceReconciliationOutcome::Adopted
        } else {
            SourceReconciliationOutcome::NeedsWork(diagnostic)
        });
    }
    if has_conflicts(context).await? {
        return Ok(SourceReconciliationOutcome::NeedsWork(diagnostic));
    }
    Err(output.into_diagnostic().into())
}

async fn has_conflicts(context: ReconciliationContext<'_>) -> Result<bool, SourceDeliveryError> {
    let output = context
        .capture(&["diff", "--name-only", "--diff-filter=U"])
        .await?
        .require_success()?;
    Ok(!output.stdout().is_empty() || output.stdout_truncated())
}

async fn is_ancestor(
    context: ReconciliationContext<'_>,
    ancestor: &str,
    descendant: &str,
) -> Result<bool, SourceDeliveryError> {
    let output = context
        .capture(&["merge-base", "--is-ancestor", ancestor, descendant])
        .await?;
    if output.diagnostic().exit_status == Some(1) {
        return Ok(false);
    }
    output.require_success()?;
    Ok(true)
}
