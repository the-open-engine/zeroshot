//! HTTP response handling and credential redaction shared by the token-based forge adapters.

use std::fmt::Write as _;

use reqwest::Url;
use url::Position;

use crate::native_v2_delivery::{command, git_auth};
use crate::source_code_provider::{
    DELIVERY_GENERATED_BODY_END, DELIVERY_GENERATED_BODY_START, SourceDeliveryCredential,
    SourceDeliveryError, SourceGitCredential, SourceReviewRequest,
};

/// A forge explains a rejection in a short body; a longer body keeps only its prefix.
pub(crate) const ERROR_BODY_LIMIT: usize = 4 * 1024;

/// The rejected response body, cut at `ERROR_BODY_LIMIT`; `true` when it was cut or unreadable.
pub(crate) async fn rejection_body(response: &mut reqwest::Response) -> (Vec<u8>, bool) {
    let mut body = Vec::new();
    let truncated = loop {
        match response.chunk().await {
            Ok(Some(chunk)) if body.len() + chunk.len() <= ERROR_BODY_LIMIT => {
                body.extend_from_slice(&chunk);
            }
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk[..ERROR_BODY_LIMIT - body.len()]);
                break true;
            }
            Ok(None) => break false,
            Err(_) => break true,
        }
    };
    (body, truncated)
}

/// `context` followed by every cause of `error`, without its URL. A cause from the operating
/// system also names its portable kind, because the message itself is localized.
pub(crate) fn transport_detail(context: String, error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut detail = context;
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    while let Some(current) = cause {
        let _ = write!(detail, ": {current}");
        if let Some(kind) = current
            .downcast_ref::<std::io::Error>()
            .filter(|error| error.raw_os_error().is_some())
            .map(std::io::Error::kind)
        {
            let _ = write!(detail, " [{kind}]");
        }
        cause = current.source();
    }
    detail
}

/// Why a bounded body could not be read.
pub(crate) enum BodyFailure {
    /// The declared or received length passed the bound.
    TooLarge,
    /// Reading a chunk failed. `source` is the response URL up to its path.
    Transport {
        source: String,
        error: reqwest::Error,
    },
}

/// Reads the whole body when it stays within `maximum_bytes`.
pub(crate) async fn bounded_body(
    mut response: reqwest::Response,
    maximum_bytes: usize,
) -> Result<Vec<u8>, BodyFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum_bytes as u64)
    {
        return Err(BodyFailure::TooLarge);
    }
    let source = response.url()[..Position::AfterPath].to_owned();
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if bytes.len().saturating_add(chunk.len()) > maximum_bytes {
                    return Err(BodyFailure::TooLarge);
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok(bytes),
            Err(error) => return Err(BodyFailure::Transport { source, error }),
        }
    }
}

/// Every value a printable diagnostic redacts: `extra` first, then the credential's own
/// diagnostic secrets, its encoded Git login, and each credential value in raw, URL-segment and
/// basic-auth form.
pub(crate) fn diagnostic_secrets(
    credential: SourceDeliveryCredential<'_>,
    extra: &[String],
) -> Vec<String> {
    let mut values = vec![credential.expose()];
    values.extend(credential.github_git_token());
    let mut secrets = extra.to_vec();
    secrets.extend_from_slice(credential.diagnostic_secrets());
    match credential.git() {
        Some(SourceGitCredential::Https { username, password }) => {
            values.extend([username, password]);
            secrets.push(git_auth::encode_user_password(username, password));
        }
        Some(SourceGitCredential::Ssh {
            private_key,
            known_hosts,
        }) => {
            values.extend([private_key, known_hosts]);
            values.extend(private_key.lines());
        }
        None => {}
    }
    for value in values.into_iter().filter(|value| !value.is_empty()) {
        secrets.extend([
            value.to_owned(),
            credential_segment(value),
            git_auth::encode_basic_credential(value),
        ]);
    }
    secrets
}

/// Forge text is redacted before truncation and kept on one control-free line.
pub(crate) fn display_text(bytes: Vec<u8>, truncated: bool, secrets: &[String]) -> String {
    // Redact every credential before any size bound or lossy UTF-8 conversion can split it.
    let (text, truncated) = command::diagnostic_text_with_secrets(bytes, truncated, secrets);
    let mut text = text.replace(char::is_control, " ");
    if truncated {
        text.push_str(" [truncated]");
    }
    text
}

// Match the request encoder, including slash and percent escapes, without changing its URL.
fn credential_segment(token: &str) -> String {
    let mut url = Url::parse("https://redacted.invalid/").expect("static hierarchical URL");
    url.path_segments_mut()
        .expect("HTTP URL supports path segments")
        .push(token);
    url.path()[1..].to_owned()
}

/// The delivery-owned section of a review body: the description and the issue it closes.
pub(crate) fn generated_description(request: &SourceReviewRequest) -> String {
    let issue = request
        .source_issue
        .as_ref()
        .map_or(String::new(), |issue| {
            format!("\n\nCloses #{}", issue.number)
        });
    format!(
        "{DELIVERY_GENERATED_BODY_START}\n{}{issue}\n{DELIVERY_GENERATED_BODY_END}",
        request.description
    )
}

/// Replaces the one delivery-owned section of `existing`, or appends it when there is none.
/// `review` names the forge's review in the error, for example `GitLab review`.
pub(crate) fn refresh_description(
    existing: &str,
    replacement: &str,
    review: &str,
) -> Result<String, SourceDeliveryError> {
    if !existing.contains(DELIVERY_GENERATED_BODY_START)
        && !existing.contains(DELIVERY_GENERATED_BODY_END)
    {
        return Ok(if existing.is_empty() {
            replacement.to_owned()
        } else {
            format!("{existing}\n\n{replacement}")
        });
    }
    let Some(start) = existing.find(DELIVERY_GENERATED_BODY_START) else {
        return Err(SourceDeliveryError::identity(format!(
            "{review} lacks its generated metadata section"
        )));
    };
    let Some(end) = existing.find(DELIVERY_GENERATED_BODY_END) else {
        return Err(SourceDeliveryError::Rejected);
    };
    if start >= end
        || existing.matches(DELIVERY_GENERATED_BODY_START).count() != 1
        || existing.matches(DELIVERY_GENERATED_BODY_END).count() != 1
    {
        return Err(SourceDeliveryError::Rejected);
    }
    let suffix = end + DELIVERY_GENERATED_BODY_END.len();
    Ok(format!(
        "{}{}{}",
        &existing[..start],
        replacement,
        &existing[suffix..]
    ))
}
