use crate::execution::process::ProcessSessionOutput;
use crate::native_v2_runner::{NodeRunnerError, bounded_log_text};

use super::process_failure_detail;

pub(super) const MAX_PROVIDER_DIAGNOSTIC_BYTES: usize = 8 * 1024;
const TRUNCATED_STDERR_DETAIL_PREFIX: &str = "stderr (truncated tail): ";

pub(crate) fn safe_provider_text(text: &str, redactions: &[String]) -> String {
    let mut ordered = redactions
        .iter()
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>();
    ordered.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    ordered.dedup();
    let redacted = ordered.iter().fold(text.to_owned(), |safe, value| {
        safe.replace(value, "[REDACTED]")
    });
    redact_stderr_boundary(redacted, &ordered).replace('\0', "\u{fffd}")
}

fn redact_stderr_boundary(mut text: String, redactions: &[&str]) -> String {
    let boundaries = text
        .match_indices(TRUNCATED_STDERR_DETAIL_PREFIX)
        .map(|(index, _)| index + TRUNCATED_STDERR_DETAIL_PREFIX.len())
        .collect::<Vec<_>>();
    for boundary in boundaries.into_iter().rev() {
        let Some(tail) = text.get(boundary..) else {
            continue;
        };
        let replacement_bytes = tail
            .chars()
            .take_while(|character| *character == '\u{fffd}')
            .map(char::len_utf8)
            .sum::<usize>();
        let Some(candidate) = tail.get(replacement_bytes..) else {
            continue;
        };
        let Some(secret_bytes) = redactions
            .iter()
            .filter_map(|secret| leading_secret_suffix_bytes(candidate, secret))
            .max()
        else {
            continue;
        };
        let Some(end) = boundary
            .checked_add(replacement_bytes)
            .and_then(|value| value.checked_add(secret_bytes))
        else {
            continue;
        };
        if text.get(boundary..end).is_some() {
            text.replace_range(boundary..end, "[REDACTED]");
        }
    }
    text
}

fn leading_secret_suffix_bytes(candidate: &str, secret: &str) -> Option<usize> {
    secret
        .char_indices()
        .skip(1)
        .filter_map(|(start, _)| secret.get(start..))
        .filter(|suffix| candidate.starts_with(suffix))
        .map(str::len)
        .max()
}

pub(crate) fn provider_failure_diagnostic(
    provider: &str,
    detail: Option<&str>,
    output: Option<&ProcessSessionOutput>,
    redactions: &[String],
) -> String {
    let mut details = detail
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .into_iter()
        .collect::<Vec<_>>();
    if let Some(process) = output {
        let process_detail = match process_failure_detail(process, false, true) {
            Ok(detail) => detail,
            Err(NodeRunnerError::Cancelled) => Some("provider process was cancelled".to_owned()),
            Err(_) => None,
        };
        if let Some(process_detail) = process_detail {
            details.push(process_detail);
        }
    }
    let detail = if details.is_empty() {
        "execution failed without provider detail".to_owned()
    } else {
        details.join("; ")
    };
    let detail = safe_provider_text(&detail, redactions);
    bounded_log_text(
        format!("{provider} provider failure: {}", detail.trim()),
        MAX_PROVIDER_DIAGNOSTIC_BYTES,
    )
}

pub(crate) fn redact_provider_error(
    error: NodeRunnerError,
    redactions: &[String],
) -> NodeRunnerError {
    match error {
        NodeRunnerError::DriverDetail(detail) => NodeRunnerError::DriverDetail(bounded_log_text(
            safe_provider_text(&detail, redactions),
            MAX_PROVIDER_DIAGNOSTIC_BYTES,
        )),
        error => error,
    }
}
