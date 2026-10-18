//! Pi JSONL event stream reader.
//!
//! Pi's `message_update` records are delta-only, so live output accumulates deltas while the
//! response value itself is taken from the authoritative final assistant message. Only the last
//! assistant message is the node's response: Pi can emit several per run.

use crate::native_v2_capsule::{
    provider_json_lines::{ProviderJsonLine, ProviderJsonLines},
    provider_process::{
        text::ProviderText, turn::ProviderTranscript, ProviderProcessFailure,
        ProviderProcessFailureKind,
    },
};
use serde_json::Value;

use openengine_cluster_protocol::TokenCount;

use crate::native_v2_contract::TokenUsageDelta;
use crate::native_v2_runner::{LiveOutputStream, NodeRunnerError};

pub(super) use crate::native_v2_capsule::provider_process::{
    ProviderResult as PiResult, ProviderTurnFailure as PiFailure, ProviderAttempt as PiAttempt,
    ProviderEmission as PiEmission,
};

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
    /// Malformed counts or overflow make the turn total permanently incomplete.
    usage_invalid: bool,
    output: ProviderText,
}

impl PiTranscript {
    pub(super) fn new(redactions: Vec<String>) -> Self {
        Self {
            lines: ProviderJsonLines::new(),
            session_id: None,
            terminal: None,
            usage: None,
            current_usage: None,
            usage_invalid: false,
            output: ProviderText::new(&redactions),
        }
    }
}

impl ProviderTranscript for PiTranscript {
    fn push(&mut self, chunk: &[u8]) -> Vec<PiEmission> {
        let records = self.lines.push(chunk);
        self.accept(records)
    }

    fn finish_stream(&mut self) -> Vec<PiEmission> {
        let mut emissions = self
            .lines
            .finish()
            .map_or_else(Vec::new, |record| self.accept([record]));
        self.output.finish(&mut emissions);
        emissions
    }

    fn token_usage(&self) -> Option<TokenUsageDelta> {
        if self.usage_invalid {
            return None;
        }
        match (self.usage, self.current_usage) {
            (Some(total), Some(current)) => TokenUsageDelta::checked_add(total, current),
            (Some(total), None) => Some(total),
            (None, current) => current,
        }
    }

    /// True only when the provider reported a usable answer. A failed or aborted model response is
    /// reported in the event stream while Pi still exits zero, so exit status alone is insufficient.
    fn is_success(&self) -> bool {
        matches!(self.terminal, Some(Terminal::Complete(_)))
    }

    /// Settles the attempt from the last assistant message. A recovered retry is a success, and a
    /// failure reported after the final answer is a failure, because the last message is
    /// authoritative.
    fn finish(
        mut self,
        process_failure: Option<ProviderProcessFailure>,
    ) -> Result<PiAttempt, NodeRunnerError> {
        let process_failure = process_failure
            .as_ref()
            .filter(|failure| !failure.detail.trim().is_empty());
        // A final record may arrive without its newline when the process exits, so drain the
        // pending record before settling.
        let _ = self.finish_stream();
        let terminal = self.terminal.take();
        let session_id = self.session_id.clone();
        match terminal {
            // Stderr alone may be startup noise, but failed input or completion must remain visible
            // alongside the native error. Incomplete input cannot be continued in the session.
            Some(Terminal::Failed(mut detail)) => {
                if let Some(failure) = process_failure
                    .filter(|failure| failure.kind != ProviderProcessFailureKind::Stderr)
                {
                    if !detail.trim().is_empty() {
                        detail.push_str("; ");
                    }
                    detail.push_str(failure.detail.trim());
                }
                Ok(PiAttempt::Failed(PiFailure {
                    session_id,
                    retryable: !process_failure.is_some_and(|failure| {
                        failure.kind == ProviderProcessFailureKind::IncompleteInput
                    }),
                    diagnostic: self.safe(&detail),
                }))
            }
            Some(Terminal::Complete(message)) => {
                // A process that failed after producing an answer must not be treated as usable.
                if let Some(detail) = process_failure {
                    return Ok(PiAttempt::Failed(PiFailure {
                        session_id,
                        retryable: false,
                        diagnostic: self.safe(&detail.detail),
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
                diagnostic: self.safe(
                    process_failure.map_or("Pi ended without a settled answer", |failure| {
                        failure.detail.as_str()
                    }),
                ),
            })),
        }
    }
}

impl PiTranscript {
    fn accept(&mut self, records: impl IntoIterator<Item = ProviderJsonLine>) -> Vec<PiEmission> {
        let mut emissions = Vec::new();
        for record in records {
            let ProviderJsonLine::Record(bytes) = record else {
                // Drop the oversized record; a later valid terminal message can still settle the turn.
                continue;
            };
            if bytes.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                // A malformed record never fails the turn on its own: Pi may interleave records a
                // future version adds. A turn with no terminal event still settles as a failure.
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
                    self.output.push(emissions, LiveOutputStream::Output, &text);
                }
            }
            "message_end" => {
                self.output.boundary(emissions);
                self.record_message_end(value);
            }
            "compaction_start" => {
                self.output.emit(
                    emissions,
                    LiveOutputStream::System,
                    "Pi compacted the session context",
                );
            }
            "auto_retry_start" => {
                let attempt = value.get("attempt").and_then(Value::as_i64).unwrap_or(0);
                self.output.emit(
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
        let Some(usage) = usage else {
            return;
        };
        let Some(current) = parse_usage(usage) else {
            self.usage_invalid = true;
            return;
        };
        self.current_usage = Some(current);
        if let Some(total) = self.usage {
            self.usage_invalid |= TokenUsageDelta::checked_add(total, current).is_none();
        }
    }

    /// Folds the in-flight assistant message's usage into the turn total.
    fn commit_usage(&mut self) {
        if let Some(current) = self.current_usage.take() {
            self.usage = match self.usage {
                Some(total) => TokenUsageDelta::checked_add(total, current),
                None => Some(current),
            };
            self.usage_invalid |= self.usage.is_none();
        }
    }

    fn safe(&self, text: &str) -> String {
        self.output.redact(text)
    }
}

/// Pi usage records are complete cumulative snapshots in provider-native field names.
fn parse_usage(value: &Value) -> Option<TokenUsageDelta> {
    let usage = value.as_object()?;
    let count = |name: &str| {
        usage
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|value| TokenCount::new(value).ok())
    };
    let optional = |name: &str| {
        usage
            .get(name)
            .map_or(Some(None), |_| count(name).map(Some))
    };
    Some(TokenUsageDelta {
        input_tokens: count("input")?,
        output_tokens: count("output")?,
        cache_read_input_tokens: optional("cacheRead")?,
        cache_creation_input_tokens: optional("cacheWrite")?,
    })
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

#[cfg(test)]
#[path = "transcript/tests.rs"]
mod tests;
