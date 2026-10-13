//! Pi JSONL event stream reader.
//!
//! Pi's `message_update` records are delta-only, so live output accumulates deltas while the
//! response value itself is taken from the authoritative final assistant message. Only the last
//! assistant message is the node's response: Pi can emit several per run.

use serde_json::Value;

use crate::native_v2_capsule::provider_json_lines::{ProviderJsonLine, ProviderJsonLines};
use crate::native_v2_capsule::provider_process::safe_provider_text;
use openengine_cluster_protocol::TokenCount;

use crate::native_v2_contract::TokenUsageDelta;
use crate::native_v2_runner::{LiveOutputStream, NodeRunnerError};

/// One readable piece of a turn, already mapped to its output stream.
pub(super) struct PiEmission {
    pub(super) stream: LiveOutputStream,
    pub(super) text: String,
}

pub(super) struct PiResult {
    pub(super) session_id: Option<String>,
    pub(super) message: String,
}

pub(super) struct PiFailure {
    pub(super) session_id: Option<String>,
    pub(super) retryable: bool,
    pub(super) diagnostic: String,
}

pub(super) enum PiAttempt {
    Complete(PiResult),
    Failed(PiFailure),
}

enum Terminal {
    Complete(String),
    Failed(String),
}

pub(crate) struct PiTranscript {
    lines: ProviderJsonLines,
    session_id: Option<String>,
    terminal: Option<Terminal>,
    /// Usage already committed from assistant messages that have ended this turn. Pi resets usage
    /// for each assistant message, and one turn runs one model request per tool step, so the turn
    /// total is the sum across messages.
    usage: Option<TokenUsageDelta>,
    /// Latest cumulative usage for the assistant message currently streaming. Pi reports usage as
    /// the running total of its message, so each update replaces this rather than adding to it.
    current_usage: Option<TokenUsageDelta>,
    malformed: usize,
    redactions: Vec<String>,
}

impl PiTranscript {
    pub(super) fn new(mut redactions: Vec<String>) -> Self {
        redactions.retain(|value| !value.is_empty());
        // Longest first, so a redaction that contains another is applied before its substring.
        redactions
            .sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
        redactions.dedup();
        Self {
            lines: ProviderJsonLines::new(),
            session_id: None,
            terminal: None,
            usage: None,
            current_usage: None,
            malformed: 0,
            redactions,
        }
    }

    pub(super) fn push(&mut self, chunk: &[u8]) -> Vec<PiEmission> {
        let records = self.lines.push(chunk);
        self.accept(records)
    }

    pub(super) fn finish_stream(&mut self) -> Vec<PiEmission> {
        self.lines
            .finish()
            .map_or_else(Vec::new, |record| self.accept([record]))
    }

    pub(super) fn token_usage(&self) -> Option<TokenUsageDelta> {
        match (self.usage, self.current_usage) {
            (Some(total), Some(current)) => add_usage(total, current).or(Some(total)),
            (Some(total), None) => Some(total),
            (None, current) => current,
        }
    }

    /// True only when the provider reported a usable answer. A failed or aborted model response is
    /// reported in the event stream while Pi still exits zero, so exit status alone is insufficient.
    pub(super) fn is_success(&self) -> bool {
        matches!(self.terminal, Some(Terminal::Complete(_)))
    }

    /// Settles the attempt from the last assistant message. A recovered retry is a success, and a
    /// failure reported after the final answer is a failure, because the last message is
    /// authoritative.
    pub(super) fn finish(
        mut self,
        process_failure: Option<&str>,
    ) -> Result<PiAttempt, NodeRunnerError> {
        let process_failure = process_failure.filter(|detail| !detail.trim().is_empty());
        // A final record may arrive without its newline when the process exits, so drain the
        // pending record before settling.
        let _ = self.finish_stream();
        let terminal = self.terminal.take();
        let session_id = self.session_id.clone();
        match terminal {
            // A provider-reported failure carries the real explanation, such as the upstream HTTP
            // error that survived Pi's own retries. Prefer it over the process tail, which for a
            // zero-exit Pi holds only startup noise.
            Some(Terminal::Failed(detail)) => Ok(PiAttempt::Failed(PiFailure {
                session_id,
                retryable: true,
                diagnostic: self.safe(&detail),
            })),
            Some(Terminal::Complete(message)) => {
                // A process that failed after producing an answer must not be treated as usable.
                if let Some(detail) = process_failure {
                    return Ok(PiAttempt::Failed(PiFailure {
                        session_id,
                        retryable: false,
                        diagnostic: self.safe(detail),
                    }));
                }
                Ok(PiAttempt::Complete(PiResult {
                    session_id,
                    message,
                }))
            }
            None => Ok(PiAttempt::Failed(PiFailure {
                session_id,
                retryable: false,
                diagnostic: self
                    .safe(process_failure.unwrap_or("Pi ended without a settled answer")),
            })),
        }
    }

    fn accept(&mut self, records: impl IntoIterator<Item = ProviderJsonLine>) -> Vec<PiEmission> {
        let mut emissions = Vec::new();
        for record in records {
            let ProviderJsonLine::Record(bytes) = record else {
                // One oversized record is dropped; the turn continues and ends as a failure.
                self.malformed = self.malformed.saturating_add(1);
                continue;
            };
            if bytes.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                // A malformed record never fails the turn on its own: Pi may interleave records a
                // future version adds. A turn with no terminal event still settles as a failure.
                self.malformed = self.malformed.saturating_add(1);
                continue;
            };
            self.accept_record(&value, &mut emissions);
        }
        emissions
    }

    fn accept_record(&mut self, value: &Value, emissions: &mut Vec<PiEmission>) {
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            return;
        };
        match kind {
            "session" => {
                if let Some(id) = value.get("id").and_then(Value::as_str) {
                    self.session_id = Some(id.to_owned());
                }
            }
            "message_start" => {
                // A new assistant message is a new model request whose usage starts over, so the
                // previous message's latest usage becomes part of the turn total. A `message_end`
                // normally commits it already; this also covers a stream that omitted one.
                if value
                    .get("message")
                    .and_then(|message| message.get("role"))
                    .and_then(Value::as_str)
                    == Some("assistant")
                {
                    self.commit_usage();
                    // The last assistant message is the node's answer, so a message that starts
                    // after one already settled must not leave that earlier text in place.
                    self.terminal = None;
                }
            }
            "message_update" => {
                self.record_usage(value.get("usage"));
                if let Some(text) = self.record_delta(value) {
                    push_text(emissions, LiveOutputStream::Output, &text);
                }
            }
            "message_end" => self.record_message_end(value),
            "compaction_start" => {
                push_text(
                    emissions,
                    LiveOutputStream::System,
                    "Pi compacted the session context",
                );
            }
            "auto_retry_start" => {
                let attempt = value.get("attempt").and_then(Value::as_i64).unwrap_or(0);
                push_text(
                    emissions,
                    LiveOutputStream::System,
                    &format!("Pi is retrying the model turn (attempt {attempt})"),
                );
            }
            // Unknown future event types are ignored before any provider-owned field is validated.
            _ => {}
        }
    }

    fn record_message_end(&mut self, value: &Value) {
        let Some(message) = value.get("message") else {
            return;
        };
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            return;
        }
        let text = assistant_text(message);
        // The settled message carries the authoritative usage for this model request, so replace
        // any streaming value with it before committing the message to the turn total.
        self.record_usage(message.get("usage"));
        self.commit_usage();
        match message.get("stopReason").and_then(Value::as_str) {
            // An error or aborted stop reason means the turn produced no usable answer, even
            // though Pi exits successfully and reports it only in the event stream.
            Some("error") | Some("aborted") => {
                let detail = message
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .unwrap_or("Pi reported a failed model response");
                self.terminal = Some(Terminal::Failed(detail.to_owned()));
            }
            // A tool-use step is narration, never the node's answer: Pi emits one assistant
            // message per tool call, so the answer is whatever follows the last tool result.
            Some("toolUse") => {}
            // The last assistant message is authoritative, including when it carries no text. An
            // empty answer is reported as such for the contract to reject, never replaced by an
            // earlier message's text.
            _ => self.terminal = Some(Terminal::Complete(text.unwrap_or_default())),
        }
    }

    fn record_delta(&self, value: &Value) -> Option<String> {
        let event = value.get("assistantMessageEvent")?;
        match event.get("type").and_then(Value::as_str)? {
            // Pi streams a model response as many small `text_delta` events. The final assistant
            // message, not these deltas, is the node's response, so only live output is kept here.
            "text_delta" => event
                .get("delta")
                .and_then(Value::as_str)
                .map(str::to_owned),
            _ => None,
        }
    }

    fn record_usage(&mut self, usage: Option<&Value>) {
        let Some(usage) = usage.filter(|usage| usage.is_object()) else {
            return;
        };
        // Pi reports provider-native key names, so they are mapped into the protocol shape rather
        // than parsed through the wire helper.
        let count = |name: &str| {
            usage
                .get(name)
                .and_then(Value::as_u64)
                .and_then(|value| TokenCount::new(value).ok())
        };
        // A record that omits the required counters is ignored rather than treated as zero usage,
        // so a future partial usage shape cannot invent a settled total.
        let (Some(input), Some(output)) = (count("input"), count("output")) else {
            return;
        };
        self.current_usage = Some(TokenUsageDelta {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: count("cacheRead"),
            cache_creation_input_tokens: count("cacheWrite"),
        });
    }

    /// Folds the in-flight assistant message's usage into the turn total.
    fn commit_usage(&mut self) {
        if let Some(current) = self.current_usage.take() {
            self.usage = Some(match self.usage {
                Some(total) => add_usage(total, current).unwrap_or(total),
                None => current,
            });
        }
    }

    fn safe(&self, text: &str) -> String {
        safe_provider_text(text, &self.redactions)
    }
}

/// Live assistant text of one message, concatenated in content order.
fn assistant_text(message: &Value) -> Option<String> {
    let content = message.get("content")?;
    let blocks = match content {
        Value::Array(blocks) => blocks,
        Value::String(text) => return Some(text.clone()),
        _ => return None,
    };
    let mut text = String::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        if let Some(part) = block.get("text").and_then(Value::as_str) {
            text.push_str(part);
        }
    }
    (!text.is_empty()).then_some(text)
}

/// Emits one non-empty live fragment. Deltas are emitted verbatim: Pi, like Claude, streams a
/// model response as many small `text_delta` events, and a fragment must never be dropped merely
/// because its characters happen to appear earlier in the same response.
fn push_text(emissions: &mut Vec<PiEmission>, stream: LiveOutputStream, text: &str) {
    if text.is_empty() {
        return;
    }
    emissions.push(PiEmission {
        stream,
        text: text.to_owned(),
    });
}

/// Adds two usage deltas, failing closed if any counter would exceed its bound.
fn add_usage(left: TokenUsageDelta, right: TokenUsageDelta) -> Option<TokenUsageDelta> {
    Some(TokenUsageDelta {
        input_tokens: left.input_tokens.checked_add(right.input_tokens)?,
        output_tokens: left.output_tokens.checked_add(right.output_tokens)?,
        cache_read_input_tokens: add_optional_usage(
            left.cache_read_input_tokens,
            right.cache_read_input_tokens,
        )?,
        cache_creation_input_tokens: add_optional_usage(
            left.cache_creation_input_tokens,
            right.cache_creation_input_tokens,
        )?,
    })
}

/// Cache counters are reported only when every message reported them, so a partial pair stays
/// absent rather than inventing a total for the turn.
fn add_optional_usage(
    left: Option<TokenCount>,
    right: Option<TokenCount>,
) -> Option<Option<TokenCount>> {
    match (left, right) {
        (Some(left), Some(right)) => left.checked_add(right).map(Some),
        _ => Some(None),
    }
}

#[cfg(test)]
#[path = "transcript/tests.rs"]
mod tests;
