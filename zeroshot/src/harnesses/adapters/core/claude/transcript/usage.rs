use std::collections::BTreeMap;

use openengine_cluster_protocol::TokenCount;
use serde_json::Value;

use crate::native_v2_contract::TokenUsageDelta;

pub(super) fn message_identity(message: &serde_json::Map<String, Value>) -> Option<&str> {
    message
        .get("id")
        .or_else(|| message.get("uuid"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}

#[derive(Clone, Copy, Default)]
pub(super) struct UsageSnapshot {
    input_tokens: Option<TokenCount>,
    output_tokens: Option<TokenCount>,
    cache_read_input_tokens: Option<TokenCount>,
    cache_creation_input_tokens: Option<TokenCount>,
}

impl UsageSnapshot {
    fn merge(&mut self, next: Self) {
        if next.input_tokens.is_some() {
            self.input_tokens = next.input_tokens;
        }
        if next.output_tokens.is_some() {
            self.output_tokens = next.output_tokens;
        }
        if next.cache_read_input_tokens.is_some() {
            self.cache_read_input_tokens = next.cache_read_input_tokens;
        }
        if next.cache_creation_input_tokens.is_some() {
            self.cache_creation_input_tokens = next.cache_creation_input_tokens;
        }
    }

    fn complete(self) -> Option<TokenUsageDelta> {
        Some(TokenUsageDelta {
            input_tokens: self.input_tokens?,
            output_tokens: self.output_tokens?,
            cache_read_input_tokens: self.cache_read_input_tokens,
            cache_creation_input_tokens: self.cache_creation_input_tokens,
        })
    }

    fn has_value(self) -> bool {
        self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.cache_read_input_tokens.is_some()
            || self.cache_creation_input_tokens.is_some()
    }
}

#[derive(Default)]
pub(super) struct ProvisionalUsage {
    messages: BTreeMap<String, UsageSnapshot>,
    anonymous: Option<TokenUsageDelta>,
    invalid: bool,
}

impl ProvisionalUsage {
    pub(super) fn invalidate(&mut self) {
        self.invalid = true;
    }

    pub(super) fn replace_message(&mut self, message_id: &str, usage: UsageSnapshot) {
        self.messages.insert(message_id.to_owned(), usage);
    }

    pub(super) fn merge_message(&mut self, message_id: &str, usage: UsageSnapshot) {
        self.messages
            .entry(message_id.to_owned())
            .or_default()
            .merge(usage);
    }

    pub(super) fn add_anonymous(&mut self, usage: UsageSnapshot) {
        if self.invalid {
            return;
        }
        let Some(usage) = usage.complete() else {
            self.invalid = true;
            return;
        };
        self.anonymous = match self.anonymous {
            Some(current) => match TokenUsageDelta::checked_add(current, usage) {
                Some(total) => Some(total),
                None => {
                    self.invalid = true;
                    None
                }
            },
            None => Some(usage),
        };
    }

    fn is_empty(&self) -> bool {
        !self.invalid && self.anonymous.is_none() && self.messages.is_empty()
    }

    pub(super) fn total(&self) -> Option<TokenUsageDelta> {
        if self.invalid {
            return None;
        }
        let mut total = self.anonymous;
        for usage in self.messages.values().copied() {
            let usage = usage.complete()?;
            total = Some(match total {
                Some(current) => TokenUsageDelta::checked_add(current, usage)?,
                None => usage,
            });
        }
        total
    }
}

#[derive(Clone, Copy)]
pub(super) struct UsageKeys {
    input: &'static str,
    output: &'static str,
    cache_read: &'static str,
    cache_creation: &'static str,
}

impl UsageKeys {
    pub(super) const CLAUDE_STREAM: Self = Self {
        input: "input_tokens",
        output: "output_tokens",
        cache_read: "cache_read_input_tokens",
        cache_creation: "cache_creation_input_tokens",
    };
    const CLAUDE_MODEL: Self = Self {
        input: "inputTokens",
        output: "outputTokens",
        cache_read: "cacheReadInputTokens",
        cache_creation: "cacheCreationInputTokens",
    };
}

/// Usage that Claude Code states on its results in `model_usage_since_last_result`.
///
/// The field is keyed like `modelUsage` and states each model call of the process, including
/// subagent calls, on at most one result, so the stated values sum to the invocation's usage.
/// `modelUsage` is a session running total that carries earlier processes on `--resume`, and
/// `usage` covers only the main thread, so neither replaces a missing value. `{}` states
/// nothing; an absent or malformed value leaves the invocation's usage unknown.
#[derive(Clone, Copy, Default)]
pub(super) enum ResultUsage {
    #[default]
    Unwritten,
    Stated(TokenUsageDelta),
    Unknown,
}

impl ResultUsage {
    pub(super) fn record(&mut self, since_last_result: Option<&Value>) {
        let total = match *self {
            Self::Unwritten => Some(nothing_counted()),
            Self::Stated(total) => Some(total),
            Self::Unknown => None,
        };
        *self = total
            .and_then(|total| match since_last_result {
                Some(Value::Object(models)) if models.is_empty() => Some(total),
                stated => TokenUsageDelta::checked_add(total, parse_model_usage(stated)?),
            })
            .map_or(Self::Unknown, Self::Stated);
    }

    /// Calls after the last result are stated nowhere. Their streamed message usage counts as it
    /// does for a process that wrote no result.
    pub(super) fn total(self, since_last_result: &ProvisionalUsage) -> Option<TokenUsageDelta> {
        let stated = match self {
            Self::Unwritten => return since_last_result.total(),
            Self::Stated(total) => total,
            Self::Unknown => return None,
        };
        if since_last_result.is_empty() {
            return Some(stated);
        }
        TokenUsageDelta::checked_add(stated, since_last_result.total()?)
    }
}

fn nothing_counted() -> TokenUsageDelta {
    TokenUsageDelta {
        input_tokens: TokenCount::default(),
        output_tokens: TokenCount::default(),
        cache_read_input_tokens: Some(TokenCount::default()),
        cache_creation_input_tokens: Some(TokenCount::default()),
    }
}

pub(super) fn parse_usage_snapshot(
    value: Option<&Value>,
    keys: UsageKeys,
) -> Option<UsageSnapshot> {
    let usage = value?.as_object()?;
    let snapshot = UsageSnapshot {
        input_tokens: optional_count(usage, keys.input)?,
        output_tokens: optional_count(usage, keys.output)?,
        cache_read_input_tokens: optional_count(usage, keys.cache_read)?,
        cache_creation_input_tokens: optional_count(usage, keys.cache_creation)?,
    };
    snapshot.has_value().then_some(snapshot)
}

fn optional_count(usage: &serde_json::Map<String, Value>, key: &str) -> Option<Option<TokenCount>> {
    match usage.get(key) {
        Some(value) => Some(Some(TokenCount::new(value.as_u64()?).ok()?)),
        None => Some(None),
    }
}

fn parse_model_usage(value: Option<&Value>) -> Option<TokenUsageDelta> {
    let models = value?.as_object()?;
    let mut total = None;
    for model in models.values() {
        let usage = parse_usage_snapshot(Some(model), UsageKeys::CLAUDE_MODEL)?.complete()?;
        total = Some(match total {
            Some(current) => TokenUsageDelta::checked_add(current, usage)?,
            None => usage,
        });
    }
    total
}

#[cfg(test)]
#[path = "usage/tests.rs"]
mod tests;
