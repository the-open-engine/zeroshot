//! Merge, withdrawal and conflict materialization for the contributed Forgejo adapter.
//!
//! A merge is one write for one identity-validated review, pinned to the reviewed head. The intent is
//! recorded before the write and released only when a write settles or a read shows the merge, so a
//! withdrawal never reports an attempt whose outcome is unknown as withdrawn. A later write for the
//! same review is pinned to the same head. Nothing here asks the forge to force a merge, to merge
//! when checks succeed, or to delete the head branch.

use std::collections::BTreeSet;
use std::sync::MutexGuard;

use reqwest::Method;
use serde_json::json;

use crate::native_v2_delivery::transport;
use crate::source_code_provider::{
    SourceChecks, SourceConflictOutcome, SourceConflictRequest, SourceDeliveryCredential,
    SourceDeliveryError, SourceMergeRequestOutcome, SourceMergeWithdrawal, SourceReviewReceipt,
    SourceReviewState,
};

use super::api::ApiRequest;
use super::authority::{ForgejoDeliveryAuthority, split_repository};
use super::checks;
use super::review;
use super::wire;

/// Merge intent key on the authority's effect set. The `merge` prefix keeps it apart from a
/// review-creation key for the same repository.
pub(super) fn merge_key(repository: &str, review_id: &str) -> String {
    format!("merge\0{repository}\0{review_id}")
}

pub(super) fn merge_style(
    repository: &wire::Repository,
) -> Result<&'static str, SourceDeliveryError> {
    let styles = [
        ("merge", repository.allow_merge_commits),
        ("rebase", repository.allow_rebase),
        ("rebase-merge", repository.allow_rebase_explicit),
        ("squash", repository.allow_squash_merge),
        (
            "fast-forward-only",
            repository.allow_fast_forward_only_merge,
        ),
    ];
    let default = styles
        .iter()
        .find(|(style, _)| *style == repository.default_merge_style);
    if let Some((style, allowed)) = default
        && *allowed
    {
        return Ok(style);
    }
    styles
        .into_iter()
        .find(|(_, allowed)| *allowed)
        .map(|(style, _)| style)
        .ok_or_else(|| {
            SourceDeliveryError::identity(format!(
                "Forgejo repository {} allows no merge style, so its default merge style {} cannot be used",
                wire::repository_identity(&repository.full_name),
                wire::observed(&repository.default_merge_style),
            ))
        })
}

pub(super) async fn request(
    authority: &ForgejoDeliveryAuthority,
    receipt: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
    let context = Merge::new(authority, receipt);
    authority.api.require_supported(credential).await?;
    let repository = authority
        .repository(&receipt.repository, credential)
        .await?;
    let style = merge_style(&repository)?;

    let pull = context.pull(credential).await?;
    review::require_bound_head(&pull, receipt)?;
    if pull.merged {
        review::merged_revision(&pull)?;
        context.release()?;
        return Ok(SourceMergeRequestOutcome::Accepted);
    }
    if pull.state == "closed" {
        return Ok(SourceMergeRequestOutcome::Pending);
    }

    let observed = checks::observe_open(authority, &receipt.repository, &pull, credential).await?;
    if observed.state == SourceReviewState::Conflict {
        return Ok(SourceMergeRequestOutcome::Conflict);
    }
    // A merge is requested only for an open pull whose checks have settled and that does not still
    // wait for an approval.
    let ready = matches!(
        observed,
        checks::OpenReview {
            state: SourceReviewState::Open {
                checks: SourceChecks::Passed | SourceChecks::NotRequired,
            },
            pull_request_ready: false,
        }
    );
    if !ready {
        return Ok(SourceMergeRequestOutcome::Pending);
    }

    // An earlier unconfirmed attempt keeps its record: the new write is pinned to the same reviewed
    // head, and only a settled write or a merged read releases it.
    context.remember()?;
    let (owner, name) = split_repository(&receipt.repository)?;
    let segments = ["repos", owner, name, "pulls", &receipt.review_id, "merge"];
    let body = json!({
        "Do": style,
        "head_commit_id": receipt.head_revision,
        "delete_branch_after_merge": false,
    });
    let result = authority
        .api
        .empty(ApiRequest::write(&segments, Method::POST, body), credential)
        .await;
    context.settle(result, credential).await
}

pub(super) async fn withdraw(
    authority: &ForgejoDeliveryAuthority,
    receipt: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceMergeWithdrawal, SourceDeliveryError> {
    let context = Merge::new(authority, receipt);
    authority.api.require_supported(credential).await?;
    let pull = context.pull(credential).await?;
    if pull.merged {
        review::require_bound_head(&pull, receipt)?;
        return Ok(SourceMergeWithdrawal::Merged {
            merge_revision: review::merged_revision(&pull)?,
        });
    }
    // An unconfirmed attempt may still be running on the server, so only a later merged read or a
    // settled write can release it.
    if context.record_exists()? {
        return Err(SourceDeliveryError::api(
            None,
            format!(
                "Forgejo merge settlement remains uncertain for {}; no repair handoff is authorized",
                context.identity()
            ),
        ));
    }
    Ok(SourceMergeWithdrawal::Withdrawn)
}

pub(super) async fn materialize_conflict(
    authority: &ForgejoDeliveryAuthority,
    request: &SourceConflictRequest,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceConflictOutcome, SourceDeliveryError> {
    let context = Merge::new(authority, &request.review);
    authority.api.require_supported(credential).await?;
    context.pull(credential).await?;
    let revision = authority
        .branch_revision(&request.review.repository, &request.review.target_branch, credential)
        .await?
        .ok_or_else(|| {
            SourceDeliveryError::identity(format!(
                "Forgejo target branch is absent: branch {} in repository {}. Restore the target branch before resuming.",
                wire::branch_identity(&request.review.target_branch),
                wire::repository_identity(&request.review.repository),
            ))
        })?;
    transport::materialize_conflict(authority.git()?, request, &revision, credential).await
}

/// One review, its merge record key, and the authority that reads it.
struct Merge<'a> {
    authority: &'a ForgejoDeliveryAuthority,
    receipt: &'a SourceReviewReceipt,
    key: String,
}

impl<'a> Merge<'a> {
    fn new(authority: &'a ForgejoDeliveryAuthority, receipt: &'a SourceReviewReceipt) -> Self {
        Self {
            authority,
            receipt,
            key: merge_key(&receipt.repository, &receipt.review_id),
        }
    }

    /// The identity the permission message names: the project, the review, both branches and the
    /// head revision.
    fn identity(&self) -> String {
        format!(
            "{} pull request {} (head branch {}, base branch {}, head revision {})",
            wire::repository_identity(&self.receipt.repository),
            self.receipt.review_id,
            wire::branch_identity(&self.receipt.head_branch),
            wire::branch_identity(&self.receipt.target_branch),
            wire::revision_identity(&self.receipt.head_revision),
        )
    }

    fn permission_error(&self, status: Option<u16>) -> SourceDeliveryError {
        SourceDeliveryError::api(
            status,
            format!(
                "Forgejo merge permission is missing for {}. Grant the token permission to merge into target branch {}, then resume.",
                self.identity(),
                wire::branch_identity(&self.receipt.target_branch),
            ),
        )
        .permission_denied()
    }

    async fn pull(
        &self,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<wire::PullRequest, SourceDeliveryError> {
        let (owner, name) = split_repository(&self.receipt.repository)?;
        let pull = self
            .authority
            .api
            .json::<wire::PullRequest>(
                ApiRequest::get(&["repos", owner, name, "pulls", &self.receipt.review_id]),
                credential,
            )
            .await?;
        self.require_identity(&pull)?;
        Ok(pull)
    }

    /// The read is accepted only when it is the review the receipt names. The number is the one the
    /// receipt selects; the rest of the identity is the check `review.rs` already has.
    fn require_identity(&self, pull: &wire::PullRequest) -> Result<(), SourceDeliveryError> {
        if pull.number.to_string() != self.receipt.review_id {
            return Err(SourceDeliveryError::identity(format!(
                "Forgejo pull request read returned {} instead of {}",
                pull.number, self.receipt.review_id
            )));
        }
        review::require_pull(
            pull,
            &self.receipt.repository,
            &self.receipt.target_branch,
            &self.receipt.head_branch,
        )
    }

    fn record_exists(&self) -> Result<bool, SourceDeliveryError> {
        Ok(self.effects()?.contains(&self.key))
    }

    fn remember(&self) -> Result<(), SourceDeliveryError> {
        self.effects()?.insert(self.key.clone());
        Ok(())
    }

    fn release(&self) -> Result<(), SourceDeliveryError> {
        self.effects()?.remove(&self.key);
        Ok(())
    }

    fn effects(&self) -> Result<MutexGuard<'_, BTreeSet<String>>, SourceDeliveryError> {
        self.authority
            .effects
            .lock()
            .map_err(|_| SourceDeliveryError::Unavailable)
    }

    /// What the write answered. A confirmed write releases the record; a write whose outcome is
    /// uncertain keeps it, because only a later merged read can confirm it.
    async fn settle(
        &self,
        result: Result<(), SourceDeliveryError>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
        let error = match result {
            Ok(()) => {
                let pull = self.pull(credential).await?;
                if pull.merged {
                    review::require_bound_head(&pull, self.receipt)?;
                    review::merged_revision(&pull)?;
                    self.release()?;
                }
                return Ok(SourceMergeRequestOutcome::Accepted);
            }
            Err(error) => error,
        };
        let status = error.api_status();
        // A permission refusal is decided from the write response itself, before any settle read.
        if matches!(status, Some(401 | 403)) {
            self.release()?;
            // Forgejo also uses 401 for a valid token without merge rights, so a fresh
            // authenticated read separates that from a rejected token.
            if status == Some(401) {
                let user = self
                    .authority
                    .api
                    .json::<wire::User>(ApiRequest::get(&["user"]), credential)
                    .await;
                if !matches!(&user, Ok(user) if !user.login.is_empty()) {
                    return Err(error);
                }
            }
            return Err(self.permission_error(status));
        }
        match status {
            Some(405) => {
                self.release()?;
                Ok(SourceMergeRequestOutcome::Pending)
            }
            Some(409) => {
                self.release()?;
                let pull = self.pull(credential).await?;
                if pull.head.sha != self.receipt.head_revision {
                    return Ok(SourceMergeRequestOutcome::Pending);
                }
                Ok(SourceMergeRequestOutcome::Conflict)
            }
            Some(400..=499) => {
                self.release()?;
                Err(error)
            }
            // A transport failure or a server error leaves the write uncertain: it may have
            // completed, so the read decides, and an unconfirmed record stays recorded.
            _ => {
                let pull = self.pull(credential).await?;
                if pull.merged {
                    review::require_bound_head(&pull, self.receipt)?;
                    review::merged_revision(&pull)?;
                    self.release()?;
                    return Ok(SourceMergeRequestOutcome::Accepted);
                }
                Err(error)
            }
        }
    }
}
