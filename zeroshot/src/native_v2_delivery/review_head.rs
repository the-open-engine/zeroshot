use super::{GitHubReviewObservation, GitHubReviewReceipt, GitHubReviewState, valid_revision};

#[derive(Clone, Copy)]
pub struct GitHubHeadSynchronization<'a> {
    pub workspace: &'a std::path::Path,
    pub previous: &'a GitHubReviewReceipt,
    pub updated: &'a GitHubReviewReceipt,
}

impl GitHubReviewReceipt {
    pub(crate) fn observation(&self, state: GitHubReviewState) -> GitHubReviewObservation {
        GitHubReviewObservation {
            review_id: self.review_id.clone(),
            repository: self.repository.clone(),
            target_branch: self.target_branch.clone(),
            head_branch: self.head_branch.clone(),
            head_revision: self.head_revision.clone(),
            state,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitHubMergeRequestOutcome {
    Accepted,
    Pending,
    HeadUpdateRequired,
    Conflict,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitHubHeadUpdateOutcome {
    Updated(GitHubReviewReceipt),
    Pending,
    Conflict,
}

pub(super) fn valid_head_update(
    previous: &GitHubReviewReceipt,
    updated: &GitHubReviewReceipt,
) -> bool {
    updated.review_id == previous.review_id
        && updated.repository == previous.repository
        && updated.target_branch == previous.target_branch
        && updated.head_branch == previous.head_branch
        && valid_revision(&updated.head_revision)
        && updated.head_revision != previous.head_revision
}
