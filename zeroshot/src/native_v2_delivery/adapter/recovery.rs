use super::*;

pub(super) struct RepairFailure {
    pub(super) diagnostic: String,
    pub(super) retryable: bool,
    review: Option<Box<GitHubReviewReceipt>>,
}

pub(super) fn repair(diagnostic: impl Into<String>) -> DeliveryStop {
    DeliveryStop::Repair(RepairFailure {
        diagnostic: diagnostic.into(),
        retryable: false,
        review: None,
    })
}

impl From<GitHubAuthorityError> for DeliveryStop {
    fn from(error: GitHubAuthorityError) -> Self {
        if matches!(error, GitHubAuthorityError::Identity(_)) {
            Self::Outcome(WorkerOutcome::declared_failure(WorkerErrorCode::Refusal))
        } else if error.authentication_failed() {
            Self::Outcome(WorkerOutcome::authentication_refusal())
        } else {
            let mut stop = repair(error.to_string());
            if let Self::Repair(failure) = &mut stop {
                failure.retryable = error.retryable_operation();
            }
            stop
        }
    }
}

impl DeliveryStop {
    pub(super) fn with_review(mut self, review: &GitHubReviewReceipt) -> Self {
        if let Self::Repair(failure) = &mut self {
            failure.review = Some(Box::new(review.clone()));
        }
        self
    }
}

impl NativeV2DeliveryAdapter {
    pub(super) async fn repair_outcome(
        &self,
        invocation: &DriverInvocation,
        mode: DeliveryMode,
        failure: RepairFailure,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        if !contract::supports_repair(&invocation.response) {
            return Ok(WorkerOutcome::declared_failure(WorkerErrorCode::Crash));
        }
        let review = failure
            .review
            .map(|review| *review)
            .unwrap_or_else(|| GitHubReviewReceipt {
                review_id: String::new(),
                repository: self.config.target.repository.clone(),
                target_branch: self.config.target.target_branch.clone(),
                head_branch: delivery_branch(self.config.delivery_run_id.as_str()),
                head_revision: String::new(),
            });
        let diagnostic = format!(
            "repository: {}\ntargetBranch: {}\nrunBranch: {}\nheadRevision: {}\n\
             pullRequestId: {}\n{}",
            review.repository,
            review.target_branch,
            review.head_branch,
            review.head_revision,
            review.review_id,
            failure.diagnostic,
        );
        let diagnostic = self.redact_feedback(invocation, self.review_context(&diagnostic));
        delivery_outcome(
            DeliveryResult {
                mode,
                outcome: DELIVERY_REPAIR_REQUIRED_LABEL,
                review: &review,
                merge_revision: None,
            },
            &diagnostic,
        )
    }

    pub(super) fn review_context(&self, diagnostic: &str) -> String {
        let state = self.delivery_state();
        let baseline = match state.review_base_revision.as_deref() {
            Some(revision) => format!(
                "reviewBaseRevision: {revision}\nReview the candidate against this captured target \
                 revision. Preserve integrated upstream changes."
            ),
            None => {
                "reviewBaseRevision: unavailable\nThe current integrated target revision has not \
                     been confirmed; do not infer it from sourceRevision."
                    .to_owned()
            }
        };
        format!(
            "sourceRevision: {}\n{baseline}\nsourceRevision is the original admitted source, \
             retained for provenance; do not restore files merely to match it.\n{diagnostic}",
            self.config.target.base_revision,
        )
    }

    fn redact_feedback(&self, invocation: &DriverInvocation, mut diagnostic: String) -> String {
        let tokens = [
            self.trusted_github_token.as_deref(),
            github_credential(&invocation.environment).map(GitHubCredential::expose),
        ];
        for token in tokens.into_iter().flatten() {
            diagnostic = diagnostic
                .replace(token, "[REDACTED]")
                .replace(&git_auth::encode_basic_credential(token), "[REDACTED]");
        }
        diagnostic
    }
}
