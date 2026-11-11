//! Shared bounds and formatting for forge-owned CI repair evidence.
pub(super) const MAX_FAILURE_DIAGNOSTIC_CHARS: usize = 8 * 1_024;
pub(super) const MAX_FAILURE_FEEDBACK_CHARS: usize = 64 * 1_024;
pub(super) const MAX_FAILURE_ITEM_CHARS: usize = 1_024;
pub(super) const MAX_FAILED_CHECK_LOGS: usize = 8;
pub(super) const MAX_JOB_LOG_BYTES: usize = 64 * 1_024 * 1_024;
pub(super) const MAX_JOB_LOG_TAIL_BYTES: usize = 64 * 1_024;

pub(super) fn include_job_logs(diagnostic: &mut String, label: &str, logs: &[(u64, String)]) {
    if logs.is_empty() {
        return;
    }
    let budget = MAX_FAILURE_FEEDBACK_CHARS.saturating_sub(diagnostic.chars().count()) / logs.len();
    for (job, log) in logs {
        let heading = format!("\n--- {label} job {job} log excerpt ---\n");
        let excerpt = log_tail(log, budget.saturating_sub(heading.chars().count()));
        diagnostic.push_str(&heading);
        diagnostic.push_str(&excerpt);
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

pub(super) fn job_log_tail(output: &[u8]) -> String {
    let start = output.len().saturating_sub(MAX_JOB_LOG_TAIL_BYTES);
    String::from_utf8_lossy(&output[start..]).into_owned()
}

fn log_tail(value: &str, maximum: usize) -> String {
    const TRUNCATED: &str = "[earlier log output truncated]\n";
    let clean = value
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect::<String>();
    if clean.chars().count() <= maximum {
        return clean;
    }
    let retain = maximum.saturating_sub(TRUNCATED.chars().count());
    let mut tail = clean.chars().rev().take(retain).collect::<Vec<_>>();
    tail.reverse();
    bounded_text(
        &format!("{TRUNCATED}{}", tail.into_iter().collect::<String>()),
        maximum,
    )
}
