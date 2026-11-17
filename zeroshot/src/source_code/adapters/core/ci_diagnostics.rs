//! Shared bounds and formatting for forge-owned CI repair evidence.
pub(super) const MAX_FAILURE_DIAGNOSTIC_CHARS: usize = 8 * 1_024;
pub(super) const MAX_FAILURE_FEEDBACK_CHARS: usize = 64 * 1_024;
pub(super) const MAX_FAILURE_ITEM_CHARS: usize = 1_024;
pub(super) const MAX_FAILED_CHECK_LOGS: usize = 8;
pub(super) const MAX_JOB_LOG_BYTES: usize = 64 * 1_024 * 1_024;
pub(super) const MAX_JOB_LOG_TAIL_BYTES: usize = 64 * 1_024;

/// Redacted evidence retains its raw failure classification through both budgets.
#[derive(Clone, Debug)]
pub(super) struct JobLog {
    pub(super) text: String,
    failure: Option<usize>,
}

impl JobLog {
    pub(super) fn unavailable(text: String) -> Self {
        Self {
            text,
            failure: Some(0),
        }
    }
}

pub(super) fn job_log_tail(
    value: &str,
    end: usize,
    credential: crate::source_code_provider::SourceDeliveryCredential<'_>,
    extra_secrets: &[String],
) -> JobLog {
    use crate::native_v2_delivery::command::redact_prefix;
    let secrets = diagnostic_secrets(extra_secrets.to_vec(), credential);
    let failure = first_failure(&value[..end]);
    // Match against the full input even when a secret spans a selected boundary.
    // A failure inside a secret maps to the end of its redacted placeholder.
    let text = redact_prefix(value, &secrets, end);
    let failure = failure.map(|offset| redact_prefix(value, &secrets, offset).len());
    log_excerpt(
        &JobLog { text, failure },
        MAX_JOB_LOG_TAIL_BYTES,
        usize::MAX,
    )
}

pub(super) fn redact_log(
    value: &str,
    credential: crate::source_code_provider::SourceDeliveryCredential<'_>,
    extra_secrets: &[String],
) -> String {
    crate::native_v2_delivery::command::redact(
        value,
        &diagnostic_secrets(extra_secrets.to_vec(), credential),
    )
}

pub(super) fn diagnostic_secrets(
    mut secrets: Vec<String>,
    credential: crate::source_code_provider::SourceDeliveryCredential<'_>,
) -> Vec<String> {
    use crate::native_v2_delivery::git_auth;
    use crate::source_code_provider::SourceGitCredential;

    let mut values = vec![credential.expose()];
    values.extend(credential.github_git_token());
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
    for value in values {
        secrets.extend([value.to_owned(), git_auth::encode_basic_credential(value)]);
    }
    secrets
}

pub(super) fn include_job_logs(diagnostic: &mut String, label: &str, logs: &[(u64, JobLog)]) {
    if logs.is_empty() {
        return;
    }
    let budget = MAX_FAILURE_FEEDBACK_CHARS.saturating_sub(diagnostic.chars().count()) / logs.len();
    for (job, log) in logs {
        let heading = format!("\n--- {label} job {job} log excerpt ---\n");
        let excerpt = log_tail(log, budget.saturating_sub(heading.chars().count()));
        diagnostic.push_str(&heading);
        diagnostic.push_str(&excerpt.text);
    }
}

pub(super) fn bounded_text(value: &str, maximum: usize) -> String {
    const TRUNCATED: &str = "[diagnostic truncated]";
    if value.chars().count() <= maximum {
        return value.to_owned();
    }
    let mut text = value
        .chars()
        .take(maximum.saturating_sub(TRUNCATED.len()))
        .collect::<String>();
    text.extend(TRUNCATED.chars().take(maximum));
    text
}

fn log_tail(log: &JobLog, maximum: usize) -> JobLog {
    let keep = |character: &char| !character.is_control() || matches!(character, '\n' | '\t');
    let text = log.text.chars().filter(keep).collect::<String>();
    let failure = log.failure.map(|offset| {
        log.text[..offset]
            .chars()
            .filter(keep)
            .map(char::len_utf8)
            .sum()
    });
    log_excerpt(&JobLog { text, failure }, usize::MAX, maximum)
}

fn log_excerpt(log: &JobLog, maximum_bytes: usize, maximum_chars: usize) -> JobLog {
    const EARLIER: &str = "[earlier log output truncated]\n";
    const MIDDLE: &str = "\n[intermediate log output truncated]\n";
    let value = &log.text;
    if value.len() <= maximum_bytes && value.chars().count() <= maximum_chars {
        return log.clone();
    }
    let marker = log_prefix(EARLIER, maximum_bytes, maximum_chars);
    let tail = log_suffix(
        value,
        maximum_bytes.saturating_sub(marker.len()),
        maximum_chars.saturating_sub(marker.len()),
    );
    let tail_only = || JobLog {
        text: format!("{marker}{tail}"),
        failure: log
            .failure
            .filter(|offset| *offset >= value.len() - tail.len())
            .map(|offset| marker.len() + offset - (value.len() - tail.len())),
    };
    let Some(failure) = log.failure else {
        return tail_only();
    };
    // Keep nearby setup and error details, not just the matching line. Reserve
    // the other half for the final failure/exit summary at both truncation stages.
    let overhead = EARLIER.len() + MIDDLE.len();
    let context_bytes = maximum_bytes.saturating_sub(overhead) / 2;
    let context_chars = maximum_chars.saturating_sub(overhead) / 2;
    let before = log_suffix(
        &value[..failure],
        1_024.min(context_bytes / 2),
        context_chars / 2,
    );
    let line_start = value[..failure]
        .rmatch_indices('\n')
        .nth(2)
        .map_or(0, |(offset, _)| offset + 1);
    let start = line_start.max(failure - before.len());
    if start >= value.len() - tail.len() || context_bytes == 0 || context_chars == 0 {
        return tail_only();
    }
    let context = log_prefix(&value[start..], context_bytes.min(8 * 1_024), context_chars);
    let earlier = if start == 0 { "" } else { EARLIER };
    let tail = log_suffix(
        value,
        maximum_bytes.saturating_sub(earlier.len() + context.len() + MIDDLE.len()),
        maximum_chars.saturating_sub(earlier.len() + context.chars().count() + MIDDLE.len()),
    );
    let failure = if failure < start + context.len() {
        Some(earlier.len() + failure - start)
    } else if failure >= value.len() - tail.len() {
        Some(earlier.len() + context.len() + MIDDLE.len() + failure - (value.len() - tail.len()))
    } else {
        None
    };
    JobLog {
        text: format!("{earlier}{context}{MIDDLE}{tail}"),
        failure,
    }
}

fn first_failure(value: &str) -> Option<usize> {
    let mut offset = 0;
    let mut fallback = None;
    for line in value.split_inclusive('\n') {
        let lower = line.to_ascii_lowercase();
        let failure = [
            "panicked at",
            "panic: ",
            "assertion failed",
            "assertion:",
            "assertion `",
            "assertion '",
            "assertionerror",
            "traceback (most recent call last)",
        ]
        .iter()
        .filter_map(|marker| lower.find(marker))
        .min();
        if let Some(failure) = failure {
            return Some(offset + failure);
        }
        // Passing negative tests can emit generic errors. Prefer concrete
        // failure evidence anywhere in the log before using their first match.
        if fallback.is_none() {
            fallback = ["error: ", "error[", "fatal: ", "##[error]"]
                .iter()
                .filter_map(|marker| lower.find(marker))
                .min()
                .map(|failure| offset + failure);
        }
        offset += line.len();
    }
    fallback
}

fn log_prefix(value: &str, maximum_bytes: usize, maximum_chars: usize) -> &str {
    let end = value
        .char_indices()
        .take(maximum_chars)
        .map(|(offset, character)| offset + character.len_utf8())
        .take_while(|end| *end <= maximum_bytes)
        .last()
        .unwrap_or(0);
    &value[..end]
}

fn log_suffix(value: &str, maximum_bytes: usize, maximum_chars: usize) -> &str {
    let start = value
        .char_indices()
        .rev()
        .take(maximum_chars)
        .map(|(offset, _)| offset)
        .take_while(|start| value.len() - start <= maximum_bytes)
        .last()
        .unwrap_or(value.len());
    &value[start..]
}
