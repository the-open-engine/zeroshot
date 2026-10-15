use super::{SourceReviewReceipt, valid_revision};

pub(crate) fn valid_head_update(
    previous: &SourceReviewReceipt,
    updated: &SourceReviewReceipt,
) -> bool {
    updated.review_id == previous.review_id
        && updated.repository == previous.repository
        && updated.target_branch == previous.target_branch
        && updated.head_branch == previous.head_branch
        && valid_revision(&updated.head_revision)
        && updated.head_revision != previous.head_revision
}
