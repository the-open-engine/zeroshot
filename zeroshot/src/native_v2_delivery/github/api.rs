use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};

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
        bounded_output(command, self.config.api_deadline).await
    }

    pub(super) async fn confirm_review_head(
        &self,
        request: &GitHubReviewRequest,
        credential: GitHubCredential<'_>,
    ) -> Result<(), GitHubAuthorityError> {
        let value = self
            .api(
                &[format!(
                    "repos/{}/git/ref/heads/{}",
                    request.target.repository, request.head_branch
                )],
                credential,
            )
            .await?;
        let reference: GitReferenceWire =
            serde_json::from_value(value).map_err(|_| GitHubAuthorityError::Rejected)?;
        require_review_head(reference, request)
    }

    pub(super) async fn target_revision(
        &self,
        review: &GitHubReviewReceipt,
        credential: GitHubCredential<'_>,
    ) -> Result<String, GitHubAuthorityError> {
        let value = self
            .api(
                &[format!(
                    "repos/{}/git/ref/heads/{}",
                    review.repository, review.target_branch
                )],
                credential,
            )
            .await?;
        let reference: GitReferenceWire =
            serde_json::from_value(value).map_err(|_| GitHubAuthorityError::Rejected)?;
        reference_revision(reference, &review.target_branch)
    }
}

async fn bounded_output(
    mut command: Command,
    deadline: Duration,
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
        return Err(github_api_error(&error_output));
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
    let mut parts = Vec::new();
    if let Some(message) = value
        .as_ref()
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .and_then(github_api_message)
    {
        parts.push(message);
    }
    if let Some(errors) = value
        .as_ref()
        .and_then(|value| value.get("errors"))
        .and_then(Value::as_array)
    {
        parts.extend(errors.iter().take(3).filter_map(github_api_error_detail));
    }
    if parts.is_empty() {
        parts.push("GitHub CLI returned a non-structured API error".to_owned());
    }
    let diagnostic = status.map_or_else(
        || parts.join("; "),
        |status| format!("HTTP {status}: {}", parts.join("; ")),
    );
    GitHubAuthorityError::api(status, diagnostic)
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

fn github_api_error_detail(value: &Value) -> Option<String> {
    if let Some(message) = value.as_str() {
        return github_api_message(message);
    }
    let mut fields = ["resource", "field", "code"]
        .into_iter()
        .filter_map(|field| value.get(field).and_then(Value::as_str))
        .filter_map(safe_api_identifier)
        .collect::<Vec<_>>();
    if let Some(message) = value
        .get("message")
        .and_then(Value::as_str)
        .and_then(github_api_message)
    {
        fields.push(message);
    }
    (!fields.is_empty()).then(|| fields.join(" "))
}

fn safe_api_identifier(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    .then(|| value.to_owned())
}

fn github_api_message(value: &str) -> Option<String> {
    let normalized = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    exact_github_api_message(&normalized)
        .or_else(|| classified_github_api_message(&normalized))
        .map(str::to_owned)
}

fn exact_github_api_message(value: &str) -> Option<&'static str> {
    Some(match value {
        "validation failed" => "validation failed",
        "not found" => "not found",
        "bad credentials" => "bad credentials",
        "server error" => "server error",
        "service unavailable" => "service unavailable",
        _ => return None,
    })
}

fn classified_github_api_message(value: &str) -> Option<&'static str> {
    if value.contains("head sha can't be blank") || value.contains("head is invalid") {
        return Some("pull request head revision is not visible");
    }
    if value.contains("no commits between") {
        return Some("no commits are visible between base and head");
    }
    if value.contains("pull request already exists") {
        return Some("pull request already exists");
    }
    if value.contains("rate limit") {
        return Some("rate limited");
    }
    if value.contains("resource not accessible by integration") {
        return Some("resource not accessible by integration");
    }
    None
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
