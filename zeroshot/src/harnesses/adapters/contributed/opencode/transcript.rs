//! OpenCode v1 native JSONL events, independently settled from process exit status.

use std::collections::BTreeSet;

use openengine_cluster_protocol::TokenCount;
use serde_json::Value;

use crate::native_v2_capsule::provider_json_lines::{ProviderJsonLine, ProviderJsonLines};
use crate::native_v2_capsule::provider_process::text::ProviderText;
use crate::native_v2_capsule::provider_process::turn::ProviderTranscript;
use crate::native_v2_capsule::provider_process::{
    ProviderAttempt, ProviderEmission, ProviderProcessFailure, ProviderProcessFailureKind,
    ProviderResult, ProviderTurnFailure,
};
use crate::native_v2_contract::TokenUsageDelta;
use crate::native_v2_runner::{LiveOutputStream, NodeRunnerError};

const MAX_EVENT_IDENTITIES: usize = 16_384;
const MAX_NATIVE_SESSION_BYTES: usize = 1024;

pub(super) struct OpenCodeTranscript {
    lines: ProviderJsonLines,
    output: ProviderText,
    session_id: Option<String>,
    message_id: Option<String>,
    text: String,
    complete: bool,
    failure: Option<String>,
    seen: BTreeSet<String>,
    usage: Option<TokenUsageDelta>,
    usage_invalid: bool,
}

impl OpenCodeTranscript {
    pub(super) fn new(session_id: Option<String>, redactions: Vec<String>) -> Self {
        Self {
            lines: ProviderJsonLines::new(),
            output: ProviderText::new(&redactions),
            session_id,
            message_id: None,
            text: String::new(),
            complete: false,
            failure: None,
            seen: BTreeSet::new(),
            usage: None,
            usage_invalid: false,
        }
    }

    fn accept(
        &mut self,
        records: impl IntoIterator<Item = ProviderJsonLine>,
    ) -> Vec<ProviderEmission> {
        let mut emissions = Vec::new();
        for record in records {
            match record {
                ProviderJsonLine::Record(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                    Ok(value) => self.accept_record(&value, &mut emissions),
                    Err(_) if bytes.iter().any(|byte| !byte.is_ascii_whitespace()) => {
                        self.usage_invalid = true
                    }
                    Err(_) => {}
                },
                _ => {
                    self.failure = Some("OpenCode emitted an oversized native event".to_owned());
                    self.usage_invalid = true;
                }
            }
        }
        emissions
    }

    fn accept_record(&mut self, value: &Value, emissions: &mut Vec<ProviderEmission>) {
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            return;
        };
        // Future events cannot acquire or invalidate a session through unfamiliar fields.
        if !known_event(kind) {
            return;
        }
        if !self.observe_session(value) {
            return;
        }
        if kind == "error" {
            self.failure = Some(
                value
                    .pointer("/error/data/message")
                    .and_then(Value::as_str)
                    .or_else(|| value.pointer("/error/name").and_then(Value::as_str))
                    .unwrap_or("OpenCode reported a native session error")
                    .to_owned(),
            );
            return;
        }
        let Some(part) = self.part_for_record(value) else {
            return;
        };
        self.dispatch_part(kind, part, emissions);
    }

    fn dispatch_part(&mut self, kind: &str, part: &Value, emissions: &mut Vec<ProviderEmission>) {
        match kind {
            "text" => self.accept_text(part, emissions),
            "step_start" => self.complete = false,
            "step_finish" => self.accept_finish(part, emissions),
            "tool_use" => {
                // Tool failure is evidence for the agent, not a terminal model failure.
                if let Some(name) = part.get("tool").and_then(Value::as_str) {
                    self.output.emit(
                        emissions,
                        LiveOutputStream::System,
                        &format!("OpenCode tool: {name}"),
                    );
                }
            }
            _ => {}
        }
    }

    fn accept_finish(&mut self, part: &Value, emissions: &mut Vec<ProviderEmission>) {
        self.record_usage(part.get("tokens"));
        self.complete = matches!(
            part.get("reason").and_then(Value::as_str),
            Some("stop" | "end-turn")
        );
        self.output.boundary(emissions);
    }

    fn part_for_record<'a>(&mut self, value: &'a Value) -> Option<&'a Value> {
        let Some(part) = value.get("part") else {
            self.failure = Some("OpenCode native event is missing its part".to_owned());
            return None;
        };
        let Some(id) = part.get("id").and_then(Value::as_str) else {
            self.failure = Some("OpenCode native event is missing its identity".to_owned());
            return None;
        };
        if self.seen.contains(id) {
            return None;
        }
        if self.seen.len() >= MAX_EVENT_IDENTITIES {
            self.failure = Some("OpenCode native event identity budget was exceeded".to_owned());
            self.usage_invalid = true;
            return None;
        }
        self.seen.insert(id.to_owned());
        let Some(message_id) = part.get("messageID").and_then(Value::as_str) else {
            self.failure = Some("OpenCode native event is missing its message identity".to_owned());
            return None;
        };
        if self.message_id.as_deref() != Some(message_id) {
            self.text.clear();
            self.complete = false;
            self.message_id = Some(message_id.to_owned());
        }
        Some(part)
    }

    fn accept_text(&mut self, part: &Value, emissions: &mut Vec<ProviderEmission>) {
        let Some(text) = part.get("text").and_then(Value::as_str) else {
            self.failure = Some("OpenCode text event is malformed".to_owned());
            return;
        };
        self.text.push_str(text);
        self.output.push(emissions, LiveOutputStream::Output, text);
    }

    fn observe_session(&mut self, value: &Value) -> bool {
        let Some(id) = value.get("sessionID").and_then(Value::as_str).filter(|id| {
            !id.is_empty()
                && id.len() <= MAX_NATIVE_SESSION_BYTES
                && !id.chars().any(char::is_control)
        }) else {
            self.failure = Some("OpenCode native session identity is malformed".to_owned());
            return false;
        };
        if self
            .session_id
            .as_deref()
            .is_some_and(|expected| expected != id)
        {
            self.failure = Some("OpenCode changed its native session identity".to_owned());
            return false;
        }
        self.session_id = Some(id.to_owned());
        true
    }

    fn record_usage(&mut self, value: Option<&Value>) {
        if self.usage_invalid {
            return;
        }
        let parsed = value.and_then(usage);
        match (self.usage, parsed) {
            (None, Some(current)) => self.usage = Some(current),
            (Some(total), Some(current)) => {
                self.usage = total.checked_add(current);
                self.usage_invalid = self.usage.is_none();
            }
            (_, None) => {
                self.usage = None;
                self.usage_invalid = true;
            }
        }
    }
}

fn known_event(kind: &str) -> bool {
    matches!(
        kind,
        "text" | "step_start" | "step_finish" | "tool_use" | "error"
    )
}

fn token(value: &Value) -> Option<TokenCount> {
    TokenCount::new(value.as_u64()?).ok()
}

fn usage(value: &Value) -> Option<TokenUsageDelta> {
    Some(TokenUsageDelta {
        input_tokens: token(value.get("input")?)?,
        // Native getUsage subtracts reasoning from output and exposes it separately. The
        // neutral output counter includes both generated categories, with checked arithmetic.
        output_tokens: token(value.get("output")?)?.checked_add(token(value.get("reasoning")?)?)?,
        cache_read_input_tokens: optional_token(value.pointer("/cache/read"))?,
        cache_creation_input_tokens: optional_token(value.pointer("/cache/write"))?,
    })
}

fn optional_token(value: Option<&Value>) -> Option<Option<TokenCount>> {
    match value {
        Some(value) => token(value).map(Some),
        None => Some(None),
    }
}

impl ProviderTranscript for OpenCodeTranscript {
    fn push(&mut self, bytes: &[u8]) -> Vec<ProviderEmission> {
        let records = self.lines.push(bytes);
        self.accept(records)
    }

    fn finish_stream(&mut self) -> Vec<ProviderEmission> {
        let mut emissions = self
            .lines
            .finish()
            .map_or_else(Vec::new, |record| self.accept([record]));
        self.output.finish(&mut emissions);
        emissions
    }

    fn token_usage(&self) -> Option<TokenUsageDelta> {
        if self.usage_invalid { None } else { self.usage }
    }

    fn is_success(&self) -> bool {
        self.complete && self.failure.is_none() && !self.text.is_empty()
    }

    fn failure_diagnostic(&self, process_detail: Option<&str>) -> Option<String> {
        self.output.failure_diagnostic(
            "OpenCode",
            self.failure.as_deref().into_iter().chain(process_detail),
        )
    }

    fn finish(
        mut self,
        process_failure: Option<ProviderProcessFailure>,
    ) -> Result<ProviderAttempt, NodeRunnerError> {
        let _ = self.finish_stream();
        if self.is_success() && process_failure.is_none() {
            return Ok(ProviderAttempt::Complete(ProviderResult {
                session_id: self.session_id,
                message: self.text,
            }));
        }
        let mut detail = self.failure.take().unwrap_or_else(|| {
            if self.complete {
                "OpenCode ended without final answer text".to_owned()
            } else {
                "OpenCode ended without a completed native step".to_owned()
            }
        });
        if let Some(failure) = process_failure.filter(|failure| {
            failure.kind != ProviderProcessFailureKind::Stderr || !failure.detail.is_empty()
        }) {
            detail.push_str("; ");
            detail.push_str(&failure.detail);
        }
        let diagnostic = self
            .output
            .failure_diagnostic("OpenCode", [detail.as_str()])
            .unwrap_or_else(|| "OpenCode provider execution failed".to_owned());
        Ok(ProviderAttempt::Failed(ProviderTurnFailure {
            session_id: self.session_id,
            retryable: false,
            diagnostic,
        }))
    }
}
