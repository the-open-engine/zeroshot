use serde_json::Value;

use super::http::FailureKind;
use super::wire::{GitReferenceWire, reference_revision, require_review_head};
use super::*;
use super::super::ci_diagnostics::{JobLog, MAX_JOB_LOG_BYTES, diagnostic_secrets, job_log_tail};

const MAX_API_DIAGNOSTIC_BYTES: usize = 8 * 1024;

impl GhCliDeliveryAuthority {
    pub(super) async fn job_log_output(
        &self,
        repository: &str,
        job: u64,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<JobLog, SourceDeliveryError> {
        self.ensure_instance(credential).await?;
        let http = self.http().await?;
        let (reply, download_secrets) = http
            .job_log(
                (repository, job),
                credential,
                (self.api_deadline(), MAX_JOB_LOG_BYTES),
            )
            .await
            .map_err(|failure| {
                let timed_out = failure.kind == FailureKind::Timeout;
                let error = self.http_failure(failure, credential);
                if timed_out { error.temporary() } else { error }
            })?;
        if !reply.head.status.is_success() {
            return Err(if download_secrets.is_empty() {
                self.status_error(&reply, credential, &[])
            } else {
                let error = self.body_error(
                    &reply,
                    ("GitHub Actions log download failed", None),
                    (credential, &download_secrets),
                );
                if super::http_api::transient_status(&reply) {
                    error.temporary()
                } else {
                    error
                }
            });
        }
        let mut secrets = self.network.secrets();
        secrets.extend(download_secrets);
        Ok(check_log_tail(&reply.body, credential, &secrets))
    }

    pub(super) async fn confirm_review_head(
        &self,
        request: &SourceReviewRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        let reference = self
            .read_reference(&request.target.repository, &request.head_branch, credential)
            .await?;
        require_review_head(reference, request)
    }

    pub(super) async fn target_revision(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<String, SourceDeliveryError> {
        let reference = self
            .read_reference(&review.repository, &review.target_branch, credential)
            .await?;
        reference_revision(reference, &review.target_branch)
    }

    pub(super) async fn read_reference(
        &self,
        repository: &str,
        branch: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<GitReferenceWire, SourceDeliveryError> {
        let mut path = repository_path(repository, &["git", "ref", "heads"])?;
        path.extend(branch.split('/'));
        let value = self.rest(Request::get(&path), credential).await?;
        self.decode_response(value, credential)
    }

    pub(super) fn decode_response<T: serde::de::DeserializeOwned>(
        &self,
        value: Value,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<T, SourceDeliveryError> {
        T::deserialize(&value).map_err(|error| {
            use crate::native_v2_delivery::command::{diagnostic_text_with_secrets, redact};

            let secrets = diagnostic_secrets(self.network.secrets(), credential);
            let bytes = value.to_string().into_bytes();
            let truncated = bytes.len() > MAX_JOB_LOG_BYTES;
            let (mut text, truncated) = diagnostic_text_with_secrets(bytes, truncated, &secrets);
            let truncated = truncated || text.len() > MAX_API_DIAGNOSTIC_BYTES;
            // Redact before truncating so a credential cannot be exposed as a partial suffix.
            let mut boundary = text.len().min(MAX_API_DIAGNOSTIC_BYTES);
            while !text.is_char_boundary(boundary) {
                boundary = boundary.saturating_sub(1);
            }
            text.truncate(boundary);
            let diagnostic = format!(
                "GitHub returned an invalid response: {error}\nresponse (truncated={truncated}):\n{text}"
            );
            SourceDeliveryError::api(None, redact(&diagnostic, &secrets))
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MutationRejection {
    NotSent,
    Policy,
    Conversation,
    RequiredCheckQueued,
}

#[derive(Clone, Debug)]
pub(super) struct MutationFailure {
    pub error: SourceDeliveryError,
    pub rejection: Option<MutationRejection>,
}

impl MutationFailure {
    pub(super) fn not_sent(error: SourceDeliveryError) -> Self {
        Self {
            error,
            rejection: Some(MutationRejection::NotSent),
        }
    }

    pub(super) fn unknown(error: SourceDeliveryError) -> Self {
        Self {
            error,
            rejection: None,
        }
    }
}

pub(super) fn check_log_tail(
    output: &[u8],
    credential: SourceDeliveryCredential<'_>,
    extra_secrets: &[String],
) -> JobLog {
    let output = String::from_utf8_lossy(output);
    let bytes = output.as_bytes();
    const ERROR_MARKER: &[u8] = b"##[error]";
    // GitHub appends checkout and service cleanup after the failed step. Keep
    // the failed step through its last error annotation instead of that noise;
    // the shared excerpt selector retains early failure context and the summary.
    let end = bytes
        .windows(ERROR_MARKER.len())
        .rposition(|window| window == ERROR_MARKER)
        .map(|offset| {
            bytes[offset..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(output.len(), |length| offset + length + 1)
        })
        .unwrap_or(output.len());
    job_log_tail(&output, end, credential, extra_secrets)
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;
