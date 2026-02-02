use crate::commands::VALID_PROVIDERS;

use super::{SpineCompletion, SpineMode};

const COMMAND_CANDIDATES: [&str; 10] = [
    "help",
    "monitor",
    "issue",
    "provider",
    "guide",
    "nudge",
    "interrupt",
    "pin",
    "quit",
    "exit",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompletionContext {
    prefix: String,
    candidates: Vec<String>,
}

pub fn build_spine_completion(
    mode: SpineMode,
    input: &str,
    cursor: usize,
) -> Option<SpineCompletion> {
    if cursor != input.chars().count() {
        return None;
    }

    match mode {
        SpineMode::Command => build_command_completion(input),
        SpineMode::Intent | SpineMode::WhisperCluster | SpineMode::WhisperAgent => None,
    }
}

pub fn select_spine_completion(
    mode: SpineMode,
    input: &str,
    cursor: usize,
    selected: usize,
) -> Option<SpineCompletion> {
    let context = completion_context(mode, input, cursor)?;
    build_completion(&context.prefix, &context.candidates, selected)
}

fn build_command_completion(input: &str) -> Option<SpineCompletion> {
    let context = completion_context(SpineMode::Command, input, input.chars().count())?;
    build_completion(&context.prefix, &context.candidates, 0)
}

fn completion_context(mode: SpineMode, input: &str, cursor: usize) -> Option<CompletionContext> {
    if cursor != input.chars().count() {
        return None;
    }
    match mode {
        SpineMode::Command => command_completion_context(input),
        SpineMode::Intent | SpineMode::WhisperCluster | SpineMode::WhisperAgent => None,
    }
}

fn command_completion_context(input: &str) -> Option<CompletionContext> {
    let trimmed = input.trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }

    let ends_with_space = trimmed.ends_with(' ');
    let mut parts = trimmed.split_whitespace();
    let command = parts.next().unwrap_or("");
    if command.is_empty() {
        return None;
    }
    let args = parts.collect::<Vec<_>>();

    let (prefix, candidates, allow_empty): (&str, &[&str], bool) = if args.is_empty()
        && !ends_with_space
    {
        (command, &COMMAND_CANDIDATES, false)
    } else if command.eq_ignore_ascii_case("provider") && (!args.is_empty() || ends_with_space) {
        let prefix = if ends_with_space {
            ""
        } else {
            args.last().copied().unwrap_or("")
        };
        (prefix, &VALID_PROVIDERS, true)
    } else {
        return None;
    };

    let mut candidates = prefix_matches(prefix, candidates, allow_empty);
    if args.is_empty() && !ends_with_space && prefix.chars().count() == 1 {
        // Keep `/p` completion unambiguous; "pin" still appears for `pi`.
        candidates.retain(|candidate| candidate != "pin");
    }
    if candidates.is_empty() {
        return None;
    }

    Some(CompletionContext {
        prefix: prefix.to_string(),
        candidates,
    })
}

fn prefix_matches(prefix: &str, candidates: &[&str], allow_empty: bool) -> Vec<String> {
    if prefix.is_empty() && !allow_empty {
        return Vec::new();
    }

    let prefix_lower = prefix.to_lowercase();
    candidates
        .iter()
        .filter(|candidate| candidate.starts_with(prefix_lower.as_str()))
        .filter(|candidate| candidate.chars().count() > prefix_lower.chars().count())
        .map(|candidate| candidate.to_string())
        .collect()
}

fn build_completion(
    prefix: &str,
    candidates: &[String],
    selected: usize,
) -> Option<SpineCompletion> {
    if candidates.is_empty() || selected >= candidates.len() {
        return None;
    }

    let candidate = candidates.get(selected)?;
    let ghost = suffix_after_prefix(prefix, candidate);
    if ghost.is_empty() {
        return None;
    }

    Some(SpineCompletion {
        candidates: candidates.to_vec(),
        selected,
        ghost,
    })
}

fn suffix_after_prefix(prefix: &str, candidate: &str) -> String {
    let prefix_len = prefix.chars().count();
    candidate.chars().skip(prefix_len).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_prefix_suggests_provider() {
        let completion = build_spine_completion(SpineMode::Command, "p", 1).expect("completion");
        assert_eq!(completion.ghost, "rovider");
    }

    #[test]
    fn provider_arg_suggests_known_provider() {
        let completion =
            build_spine_completion(SpineMode::Command, "provider c", 10).expect("completion");
        assert_eq!(completion.ghost, "laude");
    }

    #[test]
    fn empty_prefix_does_not_suggest_command_names() {
        let completion = build_spine_completion(SpineMode::Command, "", 0);
        assert!(completion.is_none());
    }

    #[test]
    fn select_completion_cycles_candidates() {
        let completion = build_spine_completion(SpineMode::Command, "i", 1).expect("completion");
        let cycled = select_spine_completion(SpineMode::Command, "i", 1, 1).expect("cycle");
        assert_ne!(completion.ghost, cycled.ghost);
        assert_eq!(cycled.ghost, "nterrupt");
    }
}
