use serde::Deserialize;

use super::*;

#[derive(Deserialize)]
pub(super) struct PullRequestWire {
    number: u64,
    pub(super) title: Option<String>,
    pub(super) body: Option<String>,
    base: ReviewBranchWire,
    head: ReviewBranchWire,
}

#[derive(Deserialize)]
pub(super) struct IssueCommentWire {
    pub(super) body: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct IssueWire {
    pub(super) comments: u64,
}

#[derive(Deserialize)]
pub(super) struct GitReferenceWire {
    #[serde(rename = "ref")]
    reference: String,
    object: GitReferenceObjectWire,
}

#[derive(Deserialize)]
struct GitReferenceObjectWire {
    sha: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct ReviewBranchWire {
    #[serde(rename = "ref")]
    branch: String,
    pub(super) sha: String,
    repo: ReviewRepositoryWire,
}

#[derive(Deserialize)]
struct ReviewRepositoryWire {
    full_name: String,
}

pub(super) fn review_receipt(
    wire: PullRequestWire,
    request: &SourceReviewRequest,
) -> Result<SourceReviewReceipt, SourceDeliveryError> {
    let receipt = read_review_receipt(wire, &request.target, &request.head_branch)?;
    if receipt.head_revision != request.head_revision {
        return Err(SourceDeliveryError::review_head_not_visible());
    }
    Ok(receipt)
}

pub(super) fn read_review_receipt(
    wire: PullRequestWire,
    target: &super::super::DeliveryTarget,
    head_branch: &str,
) -> Result<SourceReviewReceipt, SourceDeliveryError> {
    let receipt = SourceReviewReceipt {
        review_id: wire.number.to_string(),
        repository: wire.base.repo.full_name,
        target_branch: wire.base.branch,
        head_branch: wire.head.branch,
        head_revision: wire.head.sha,
    };
    if receipt.repository != target.repository
        || receipt.target_branch != target.target_branch
        || receipt.head_branch != head_branch
        || wire.head.repo.full_name != target.repository
        || wire.number == 0
        || !valid_revision(&receipt.head_revision)
    {
        return Err(SourceDeliveryError::identity(format!(
            "expected repository {} base {} head {}; observed review {receipt:?}",
            target.repository, target.target_branch, head_branch,
        )));
    }
    Ok(receipt)
}

pub(super) fn require_review_identity(
    wire: &PullRequestWire,
    review: &SourceReviewReceipt,
) -> Result<(), SourceDeliveryError> {
    let valid = wire.number.to_string() == review.review_id
        && wire.base.repo.full_name == review.repository
        && wire.base.branch == review.target_branch
        && wire.head.repo.full_name == review.repository
        && wire.head.branch == review.head_branch
        && wire.head.sha == review.head_revision;
    valid.then_some(()).ok_or(SourceDeliveryError::Rejected)
}

pub(super) fn require_review_head(
    wire: GitReferenceWire,
    request: &SourceReviewRequest,
) -> Result<(), SourceDeliveryError> {
    let valid_identity = wire.reference == format!("refs/heads/{}", request.head_branch)
        && wire.object.kind == "commit";
    if !valid_identity {
        return Err(SourceDeliveryError::Rejected);
    }
    if wire.object.sha != request.head_revision {
        return Err(SourceDeliveryError::review_head_not_visible());
    }
    Ok(())
}

pub(super) fn reference_revision(
    wire: GitReferenceWire,
    branch: &str,
) -> Result<String, SourceDeliveryError> {
    let valid_identity = wire.reference == format!("refs/heads/{branch}")
        && wire.object.kind == "commit"
        && valid_revision(&wire.object.sha);
    valid_identity
        .then_some(wire.object.sha)
        .ok_or(SourceDeliveryError::Rejected)
}

#[cfg(test)]
#[path = "wire/tests.rs"]
mod tests;
