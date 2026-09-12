use super::*;

pub(super) struct RepairFailure {
    diagnostic: String,
    review: Option<Box<GitHubReviewReceipt>>,
}

pub(super) fn repair(diagnostic: impl Into<String>) -> DeliveryStop {
    DeliveryStop::Repair(RepairFailure {
        diagnostic: diagnostic.into(),
        review: None,
    })
}

impl From<GitHubAuthorityError> for DeliveryStop {
    fn from(error: GitHubAuthorityError) -> Self {
        if error.authentication_failed() {
            Self::Outcome(WorkerOutcome::authentication_refusal())
        } else {
            repair(error.to_string())
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
                head_branch: delivery_branch(invocation.node.reference.run_id.as_str()),
                head_revision: String::new(),
            });
        let diagnostic = format!(
            "repository: {}\ntargetBranch: {}\nbaseRevision: {}\nrunBranch: {}\nheadRevision: {}\n\
             pullRequestId: {}\n{}",
            review.repository,
            review.target_branch,
            self.config.target.base_revision,
            review.head_branch,
            review.head_revision,
            review.review_id,
            failure.diagnostic,
        );
        let diagnostic = self.redact_feedback(invocation, diagnostic);
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

impl DeliveryCredentials<'_> {
    pub(super) async fn refresh_after(
        &mut self,
        error: GitHubAuthorityError,
        control: &DriverControl,
    ) -> Result<(), DeliveryStop> {
        if !error.authentication_failed() || !self.can_refresh() {
            return Err(error.into());
        }
        emit(control, "delivery: refreshing GitHub credential").await?;
        self.refresh().await
    }
}
