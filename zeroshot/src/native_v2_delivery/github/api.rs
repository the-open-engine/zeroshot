use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::timeout;

use super::wire::{GitReferenceWire, reference_revision, require_review_head};
use super::*;

const MAX_API_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_API_ERROR_BYTES: usize = 64 * 1024;
const MAX_API_DIAGNOSTIC_STREAM_BYTES: usize = 8 * 1024;
const MAX_CHECK_LOG_TAIL_BYTES: usize = 64 * 1024;

impl GhCliDeliveryAuthority {
    pub(super) async fn api(
        &self,
        arguments: &[String],
        credential: GitHubCredential<'_>,
    ) -> Result<Value, GitHubAuthorityError> {
        let output = self.api_output(arguments, credential).await?;
        serde_json::from_slice(&output).map_err(|error| {
            let (text, truncated) = api_diagnostic_text(output, false, MAX_API_OUTPUT_BYTES, credential);
            redacted_api_error(None, format!(
                "GitHub returned invalid JSON: {error}\nresponse (truncated={truncated}):\n{text}"
            ), credential)
        })
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
                let retryable = error.retryable_operation();
                let wrapped = redacted_api_error(
                    error.api_status(),
                    format!("{context}\n{error}"),
                    credential,
                );
                if retryable {
                    wrapped.temporary()
                } else {
                    wrapped
                }
            })
    }

    pub(super) async fn job_log_output(
        &self,
        repository: &str,
        job: u64,
        credential: GitHubCredential<'_>,
    ) -> Result<Vec<u8>, GitHubAuthorityError> {
        let mut arguments = vec![
            format!("repos/{repository}/actions/jobs/{job}/logs"),
            "--method".to_owned(),
            "GET".to_owned(),
            "--allow-escape-sequences".to_owned(),
        ];
        let output = self.api_output(&arguments, credential).await;
        if matches!(&output, Err(error) if error.api_status().is_none()
            && error.to_string().lines().any(|line| line == "unknown flag: --allow-escape-sequences"))
        {
            // Older gh rejects this flag before sending the request.
            arguments.pop();
            return self.api_output(&arguments, credential).await;
        }
        output
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
        decode_response(value, credential)
    }
}

pub(super) fn decode_response<T: serde::de::DeserializeOwned>(
    value: Value,
    credential: GitHubCredential<'_>,
) -> Result<T, GitHubAuthorityError> {
    T::deserialize(&value).map_err(|error| {
        let (text, truncated) = api_diagnostic_text(
            value.to_string().into_bytes(),
            false,
            MAX_API_OUTPUT_BYTES,
            credential,
        );
        redacted_api_error(
            None,
            format!(
                "GitHub returned an invalid response: {error}\nresponse (truncated={truncated}):\n{text}"
            ),
            credential,
        )
    })
}

pub(super) async fn command_status(
    mut command: Command,
    deadline: Duration,
    credential: GitHubCredential<'_>,
) -> Result<(), GitHubAuthorityError> {
    capture(&mut command, deadline)
        .await
        .and_then(|output| output.require_success())
        .map(|_| ())
        .map_err(|failure| {
            // Capture owns the process tree; only a completed command supplies an HTTP status.
            let response = failure
                .exit_status
                .filter(|_| !failure.stderr_truncated)
                .map(|_| github_api_error(failure.stderr.as_bytes()));
            let status = response.as_ref().and_then(GitHubAuthorityError::api_status);
            let retryable = failure.retryable_transport()
                || response
                    .as_ref()
                    .is_some_and(GitHubAuthorityError::retryable_operation)
                || status.is_none() && github_transport_error(&failure.stderr);
            let error = redacted_api_error(status, failure.to_string(), credential);
            if retryable { error.temporary() } else { error }
        })
}

async fn bounded_output(
    mut command: Command,
    deadline: Duration,
    credential: GitHubCredential<'_>,
) -> Result<Vec<u8>, GitHubAuthorityError> {
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        redacted_api_error(
            None,
            format!("could not start command: {error}"),
            credential,
        )
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        redacted_api_error(None, "command stdout pipe is unavailable", credential)
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        redacted_api_error(None, "command stderr pipe is unavailable", credential)
    })?;
    let mut output = ApiOutput::default();
    let result = timeout(deadline, async {
        tokio::try_join!(
            async {
                child
                    .wait()
                    .await
                    .map_err(|error| format!("could not wait for command: {error}"))
            },
            collect_bounded(stdout, &mut output.stdout, MAX_API_OUTPUT_BYTES, "stdout"),
            collect_bounded(stderr, &mut output.stderr, MAX_API_ERROR_BYTES, "stderr"),
        )
    })
    .await;
    match result {
        Ok(Ok((status, (), ()))) if status.success() => validate_api_output(output.stdout),
        Ok(Ok((status, (), ()))) => Err(output.failure(
            Some(status),
            &format!("command exited unsuccessfully: {status}"),
            credential,
        )),
        Ok(Err(error)) => Err(output.failure(None, &error, credential)),
        Err(_) => Err(output
            .failure(
                None,
                &format!(
                    "command timed out after {} milliseconds",
                    deadline.as_millis()
                ),
                credential,
            )
            .temporary()),
    }
}

#[derive(Default)]
struct ApiOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl ApiOutput {
    fn failure(
        self,
        status: Option<std::process::ExitStatus>,
        context: &str,
        credential: GitHubCredential<'_>,
    ) -> GitHubAuthorityError {
        // Incomplete stderr is not an authoritative HTTP response.
        let api_error = status.map(|_| github_api_error(&self.stderr));
        let api_status = api_error
            .as_ref()
            .and_then(GitHubAuthorityError::api_status);
        let (stdout, stdout_truncated) = api_diagnostic_text(
            self.stdout,
            status.is_none(),
            MAX_API_OUTPUT_BYTES,
            credential,
        );
        let (stderr, stderr_truncated) = api_diagnostic_text(
            self.stderr,
            status.is_none(),
            MAX_API_ERROR_BYTES,
            credential,
        );
        let rate_limited = api_error
            .as_ref()
            .is_some_and(|error| error.api_status() == Some(403) && error.retryable_operation());
        let transient = rate_limited || api_status.is_none() && github_transport_error(&stderr);
        let failure = redacted_api_error(
            api_status,
            format!(
                "exitStatus: {:?}\n{context}\nstderr (truncated={stderr_truncated}):\n{stderr}\n\
                stdout (truncated={stdout_truncated}):\n{stdout}",
                status.and_then(|status| status.code())
            ),
            credential,
        );
        if transient {
            failure.temporary()
        } else {
            failure
        }
    }
}

fn redacted_api_error(
    status: Option<u16>,
    diagnostic: impl Into<String>,
    credential: GitHubCredential<'_>,
) -> GitHubAuthorityError {
    let diagnostic = diagnostic
        .into()
        .replace(credential.expose(), "[REDACTED]")
        .replace(&encode_basic_credential(credential.expose()), "[REDACTED]");
    GitHubAuthorityError::api(status, diagnostic)
}

fn api_diagnostic_text(
    bytes: Vec<u8>,
    incomplete: bool,
    capture_limit: usize,
    credential: GitHubCredential<'_>,
) -> (String, bool) {
    let truncated = incomplete || bytes.len() > capture_limit;
    let (mut text, truncated) =
        super::super::command::diagnostic_text(bytes, truncated, credential.expose());
    // Reserve space for both streams inside the combined API diagnostic budget.
    // Redact before truncating so a credential cannot be exposed as a partial suffix.
    let truncated = truncated || text.len() > MAX_API_DIAGNOSTIC_STREAM_BYTES;
    let mut boundary = text.len().min(MAX_API_DIAGNOSTIC_STREAM_BYTES);
    while !text.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    text.truncate(boundary);
    (text, truncated)
}

async fn collect_bounded<R>(
    mut reader: R,
    output: &mut Vec<u8>,
    maximum_bytes: usize,
    stream: &str,
) -> Result<(), String>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|error| format!("could not read command {stream}: {error}"))?;
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
    let rate_limited = status == Some(403)
        && (value
            .as_ref()
            .and_then(|value| value.get("message"))
            .and_then(Value::as_str)
            .is_some_and(github_rate_limit_message)
            || text.lines().any(|line| {
                line.strip_prefix("HTTP 403: ")
                    .or_else(|| {
                        line.strip_prefix("gh: ")
                            .and_then(|line| line.strip_suffix(" (HTTP 403)"))
                    })
                    .is_some_and(github_rate_limit_message)
            }));
    let failure = GitHubAuthorityError::api(status, text.into_owned());
    if rate_limited {
        failure.temporary()
    } else {
        failure
    }
}

fn github_transport_error(text: &str) -> bool {
    text.lines().any(|line| {
        line.starts_with("error connecting to ") || line.contains(": TLS handshake timeout")
    })
}

fn github_rate_limit_message(message: &str) -> bool {
    message.starts_with("API rate limit exceeded")
        || message.starts_with("You have exceeded a secondary rate limit")
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
    if let Some(status) = text.lines().find_map(|line| {
        let response = line.strip_prefix("HTTP ")?;
        let (status, _) = response
            .split_once(": ")
            .or_else(|| response.split_once(" ("))?;
        status.parse().ok()
    }) {
        return Some(status);
    }
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
    const ERROR_MARKER: &[u8] = b"##[error]";
    // GitHub appends checkout and service cleanup after the failed step. Keep
    // the raw context ending at its last error annotation instead of that noise.
    let end = output
        .windows(ERROR_MARKER.len())
        .rposition(|window| window == ERROR_MARKER)
        .map(|offset| {
            output[offset..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(output.len(), |length| offset + length + 1)
        })
        .unwrap_or(output.len());
    let start = end.saturating_sub(MAX_CHECK_LOG_TAIL_BYTES);
    String::from_utf8_lossy(&output[start..end]).into_owned()
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "api/command_status_tests.rs"]
mod command_status_tests;
