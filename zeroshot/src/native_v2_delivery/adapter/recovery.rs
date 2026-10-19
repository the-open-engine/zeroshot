use super::*;

pub(super) struct DeliveryFeedback<'a, 'environment> {
    pub(super) invocation: &'a DriverInvocation,
    pub(super) credentials: &'a DeliveryCredentials<'environment>,
}

pub(super) struct RepairFailure {
    pub(super) diagnostic: String,
    pub(super) retryable: bool,
    review: Option<Box<SourceReviewReceipt>>,
    authority: Option<SourceDeliveryError>,
}

pub(super) fn repair(diagnostic: impl Into<String>) -> DeliveryStop {
    DeliveryStop::Repair(RepairFailure {
        diagnostic: diagnostic.into(),
        retryable: false,
        review: None,
        authority: None,
    })
}

pub(super) fn uncertain_authority(error: SourceDeliveryError) -> DeliveryStop {
    DeliveryStop::Repair(RepairFailure {
        diagnostic: error.to_string(),
        retryable: error.retryable_operation(),
        review: None,
        authority: Some(error),
    })
}

pub(super) fn finish_uncertain_authority(mut failure: RepairFailure) -> DeliveryStop {
    match failure.authority.take() {
        Some(error) => error.into(),
        None => DeliveryStop::Repair(failure),
    }
}

impl From<SourceDeliveryError> for DeliveryStop {
    fn from(error: SourceDeliveryError) -> Self {
        let diagnostic = error.to_string();
        if error.cleanup_unconfirmed() {
            Self::Explained {
                result: Box::new(Err(NodeRunnerError::CleanupUnconfirmed)),
                diagnostic,
            }
        } else if matches!(error, SourceDeliveryError::Identity(_)) {
            Self::failure(
                WorkerOutcome::declared_failure(WorkerErrorCode::Refusal),
                diagnostic,
            )
        } else if error.authentication_failed() {
            Self::failure(WorkerOutcome::authentication_refusal(), diagnostic)
        } else if error.retryable_operation() {
            Self::Retry(error)
        } else if matches!(
            error,
            SourceDeliveryError::Repairable(_) | SourceDeliveryError::Command(_)
        ) {
            repair(diagnostic)
        } else {
            Self::failure(
                WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
                diagnostic,
            )
        }
    }
}

impl DeliveryStop {
    pub(super) fn with_review(mut self, review: &SourceReviewReceipt) -> Self {
        if let Self::Repair(failure) = &mut self {
            failure.review = Some(Box::new(review.clone()));
        }
        self
    }
}

impl NativeV2DeliveryAdapter {
    pub(super) async fn repair_outcome(
        &self,
        feedback: &DeliveryFeedback<'_, '_>,
        mode: DeliveryMode,
        failure: RepairFailure,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        if !contract::supports_repair(&feedback.invocation.response) {
            return Ok(WorkerOutcome::declared_failure(WorkerErrorCode::Crash));
        }
        let review = failure
            .review
            .map(|review| *review)
            .unwrap_or_else(|| SourceReviewReceipt {
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
        let diagnostic =
            self.redact_feedback(feedback.credentials, self.review_context(&diagnostic));
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

    pub(super) fn redact_feedback(
        &self,
        credentials: &DeliveryCredentials<'_>,
        diagnostic: impl AsRef<str>,
    ) -> String {
        let mut secrets = credentials.feedback_secrets();
        for token in [self.trusted_github_token.as_deref()].into_iter().flatten() {
            secrets.extend([token.to_owned(), git_auth::encode_basic_credential(token)]);
        }
        for credentials in [self.trusted_git_credentials.as_ref()]
            .into_iter()
            .flatten()
        {
            source_secrets(credentials, &mut secrets);
        }
        super::super::command::redact(diagnostic.as_ref(), &secrets)
    }
}

impl DeliveryCredentials<'_> {
    fn feedback_secrets(&self) -> Vec<String> {
        let mut secrets = Vec::new();
        for token in [&self.token, &self.github_git_token] {
            secrets.extend([token.clone(), git_auth::encode_basic_credential(token)]);
        }
        if let Some(git) = &self.git {
            source_secrets(git, &mut secrets);
        }
        if let Some(environment) = self.environment {
            for (_, value) in environment.iter() {
                secrets.push(value.to_owned());
            }
            for field in [self.forge_field, Some(GITHUB_TOKEN_ENV)]
                .into_iter()
                .flatten()
            {
                if let Some(token) = forge_credential(environment, field) {
                    secrets.extend([token.to_owned(), git_auth::encode_basic_credential(token)]);
                }
            }
            for git in [
                environment.source_git_credentials().cloned(),
                git_credentials(environment).ok().flatten(),
            ]
            .into_iter()
            .flatten()
            {
                source_secrets(&git, &mut secrets);
            }
        }
        secrets
    }
}

fn source_secrets(
    credentials: &openengine_cluster_protocol::GitTransportCredentials,
    secrets: &mut Vec<String>,
) {
    use openengine_cluster_protocol::GitTransportCredentials;
    match credentials {
        GitTransportCredentials::Https { username, password } => secrets.extend([
            username.clone(),
            password.clone(),
            git_auth::encode_user_password(username, password),
        ]),
        GitTransportCredentials::Ssh {
            private_key,
            known_hosts,
        } => {
            secrets.extend([private_key.clone(), known_hosts.clone()]);
            secrets.extend(private_key.lines().map(str::to_owned));
        }
    }
}

pub(super) fn git_failure(error: super::super::git::GitError) -> DeliveryStop {
    match error {
        super::super::git::GitError::Command(failure) if failure.cleanup_unconfirmed => {
            DeliveryStop::Explained {
                result: Box::new(Err(NodeRunnerError::CleanupUnconfirmed)),
                diagnostic: failure.to_string(),
            }
        }
        error => repair(error.to_string()),
    }
}
