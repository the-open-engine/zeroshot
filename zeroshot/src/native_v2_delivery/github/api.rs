use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::timeout;

use super::wire::{GitReferenceWire, reference_revision, require_review_head};
use super::*;

const MAX_API_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_API_ERROR_BYTES: usize = 64 * 1024;
const MAX_CHECK_LOG_TAIL_BYTES: usize = 64 * 1024;

impl GhCliDeliveryAuthority {
    pub(super) async fn api(
        &self,
        arguments: &[String],
        credential: GitHubCredential<'_>,
    ) -> Result<Value, GitHubAuthorityError> {
        let output = self.api_output(arguments, credential).await?;
        serde_json::from_slice(&output).map_err(|_| GitHubAuthorityError::Rejected)
    }

    pub(super) async fn api_output(
        &self,
        arguments: &[String],
        credential: GitHubCredential<'_>,
    ) -> Result<Vec<u8>, GitHubAuthorityError> {
        let mut command = clean_command(&self.config, &self.config.gh_program, credential);
        command.arg("api").args(arguments).stdout(Stdio::piped());
        let context = format!("command: {:?} api {arguments:?}", self.config.gh_program);
        bounded_output(command, self.config.api_deadline, credential)
            .await
            .map_err(|error| {
                let diagnostic = format!("{context}\n{error}")
                    .replace(credential.expose(), "[REDACTED]")
                    .replace(&encode_basic_credential(credential.expose()), "[REDACTED]");
                GitHubAuthorityError::api(error.api_status(), diagnostic)
            })
    }

    pub(super) async fn confirm_review_head(
        &self,
        request: &GitHubReviewRequest,
        credential: GitHubCredential<'_>,
    ) -> Result<(), GitHubAuthorityError> {
        let reference = self
            .read_reference(&request.target.repository, &request.head_branch, credential)
            .await?;
        require_review_head(reference, request)
    }

    pub(super) async fn confirm_pushed_head(
        &self,
        request: &GitHubPushRequest,
        credential: GitHubCredential<'_>,
    ) -> Result<(), GitHubAuthorityError> {
        let reference = self
            .read_reference(&request.target.repository, &request.head_branch, credential)
            .await?;
        let revision = reference_revision(reference, &request.head_branch)?;
        (revision == request.head_revision)
            .then_some(())
            .ok_or(GitHubAuthorityError::Rejected)
    }

    pub(super) async fn target_revision(
        &self,
        review: &GitHubReviewReceipt,
        credential: GitHubCredential<'_>,
    ) -> Result<String, GitHubAuthorityError> {
        let reference = self
            .read_reference(&review.repository, &review.target_branch, credential)
            .await?;
        reference_revision(reference, &review.target_branch)
    }
    async fn read_reference(
        &self,
        repository: &str,
        branch: &str,
        credential: GitHubCredential<'_>,
    ) -> Result<GitReferenceWire, GitHubAuthorityError> {
        let value = self
            .api(
                &[format!("repos/{repository}/git/ref/heads/{branch}")],
                credential,
            )
            .await?;
        serde_json::from_value(value).map_err(|_| GitHubAuthorityError::Rejected)
    }
}

async fn bounded_output(
    mut command: Command,
    deadline: Duration,
    credential: GitHubCredential<'_>,
) -> Result<Vec<u8>, GitHubAuthorityError> {
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|_| GitHubAuthorityError::Unavailable)?;
    let stdout = child
        .stdout
        .take()
        .ok_or(GitHubAuthorityError::Unavailable)?;
    let stderr = child
        .stderr
        .take()
        .ok_or(GitHubAuthorityError::Unavailable)?;
    let mut output = Vec::new();
    let mut error_output = Vec::new();
    let (status, (), ()) = timeout(deadline, async {
        tokio::try_join!(
            child.wait(),
            collect_bounded(stdout, &mut output, MAX_API_OUTPUT_BYTES),
            collect_bounded(stderr, &mut error_output, MAX_API_ERROR_BYTES),
        )
    })
    .await
    .map_err(|_| GitHubAuthorityError::Unavailable)?
    .map_err(|_| GitHubAuthorityError::Unavailable)?;
    if !status.success() {
        let error = github_api_error(&error_output);
        let stdout_truncated = output.len() > MAX_API_OUTPUT_BYTES;
        let stderr_truncated = error_output.len() > MAX_API_ERROR_BYTES;
        let (stdout, stdout_truncated) =
            super::super::command::diagnostic_text(output, stdout_truncated, credential.expose());
        let (stderr, stderr_truncated) = super::super::command::diagnostic_text(
            error_output,
            stderr_truncated,
            credential.expose(),
        );
        let diagnostic = format!(
            "exitStatus: {:?}\nstdout (truncated={stdout_truncated}):\n{stdout}\n\
            stderr (truncated={stderr_truncated}):\n{stderr}",
            status.code()
        );
        return Err(GitHubAuthorityError::api(error.api_status(), diagnostic));
    }
    validate_api_output(output)
}

async fn collect_bounded<R>(
    mut reader: R,
    output: &mut Vec<u8>,
    maximum_bytes: usize,
) -> Result<(), std::io::Error>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        let remaining = maximum_bytes.saturating_add(1).saturating_sub(output.len());
        let retained = remaining.min(read);
        output.extend_from_slice(buffer.get(..retained).unwrap_or_default());
    }
}

fn github_api_error(output: &[u8]) -> GitHubAuthorityError {
    let text = String::from_utf8_lossy(output);
    let value = github_api_error_value(&text);
    let status = value
        .as_ref()
        .and_then(github_api_status)
        .or_else(|| github_api_status_from_text(&text));
    GitHubAuthorityError::api(status, text.into_owned())
}

fn github_api_error_value(text: &str) -> Option<Value> {
    text.lines()
        .rev()
        .find_map(|line| serde_json::from_str(line).ok())
        .or_else(|| {
            text.char_indices()
                .filter(|(_, character)| *character == '{')
                .find_map(|(index, _)| {
                    text.get(index..)
                        .and_then(|suffix| serde_json::from_str(suffix).ok())
                })
        })
}

fn github_api_status(value: &Value) -> Option<u16> {
    value.get("status").and_then(|status| {
        status
            .as_u64()
            .and_then(|status| u16::try_from(status).ok())
            .or_else(|| status.as_str()?.parse().ok())
    })
}

fn github_api_status_from_text(text: &str) -> Option<u16> {
    let marker = "(HTTP ";
    let start = text.rfind(marker)? + marker.len();
    let digits = text.get(start..)?.split(')').next()?;
    digits.parse().ok()
}

fn validate_api_output(output: Vec<u8>) -> Result<Vec<u8>, GitHubAuthorityError> {
    if output.is_empty() || output.len() > MAX_API_OUTPUT_BYTES {
        return Err(GitHubAuthorityError::Rejected);
    }
    Ok(output)
}

pub(super) fn check_log_tail(output: &[u8]) -> String {
    let start = output.len().saturating_sub(MAX_CHECK_LOG_TAIL_BYTES);
    String::from_utf8_lossy(output.get(start..).unwrap_or_default()).into_owned()
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;
