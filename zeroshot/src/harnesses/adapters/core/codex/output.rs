use crate::harnesses::adapters::core::tool_activity::{ToolActivity, ToolIdentity, ToolEntry};
use serde_json::Value;

use crate::native_v2_capsule::provider_json_lines::{
    ProviderJsonLine, ProviderJsonLines, parse_provider_json,
};
use crate::native_v2_contract::{TokenUsageDelta, parse_token_usage_delta};
use crate::native_v2_capsule::provider_process::{ProviderEmission, text::ProviderText};
use crate::native_v2_runner::{LiveOutputStream, NodeRunnerError};

pub(super) enum CodexEmission {
    AgentMessage(String),
    Progress(String),
    StreamError(String),
    Tool(ToolEntry),
}

impl CodexEmission {
    pub(super) fn emit(&self, publisher: &ProviderText, emissions: &mut Vec<ProviderEmission>) {
        match self {
            Self::AgentMessage(message) => {
                publisher.emit(emissions, LiveOutputStream::Output, message)
            }
            Self::Progress(message) => publisher.emit(emissions, LiveOutputStream::System, message),
            Self::StreamError(message) => publisher.emit_progress(emissions, message),
            Self::Tool(entry) => entry.emit(publisher, emissions),
        }
    }
}

pub(super) struct CodexOutput {
    pub(super) thread_id: Option<String>,
    pub(super) task_started: bool,
    thread_failure: Option<String>,
    final_message: Option<String>,
    terminal: Option<CodexTerminal>,
    failure: Option<String>,
    token_usage: Option<TokenUsageDelta>,
    malformed_records: usize,
    oversized_records: usize,
    tools: ToolActivity,
}

pub(super) struct CodexOutputDecoder {
    lines: ProviderJsonLines,
    output: CodexOutput,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CodexTerminal {
    Completed,
    Failed,
}

enum TerminalUsage {
    Empty,
    Valid(TokenUsageDelta),
    Malformed,
}

enum AcceptedLine {
    Emission(CodexEmission),
    Emissions(Vec<CodexEmission>),
    Ignored,
    Malformed,
    ThreadFailure(String),
}

impl CodexOutputDecoder {
    pub(super) fn is_completed(&self) -> bool {
        self.output.is_completed()
    }

    pub(super) fn new() -> Self {
        Self {
            lines: ProviderJsonLines::new(),
            output: CodexOutput::empty(),
        }
    }

    pub(super) fn push(&mut self, bytes: &[u8]) -> Vec<CodexEmission> {
        if self.output.is_settled() {
            self.lines.discard();
            return Vec::new();
        }
        let mut emitted = Vec::new();
        for line in self.lines.push(bytes) {
            if self.output.is_settled() {
                break;
            }
            emitted.extend(self.accept_line(line));
        }
        if self.output.is_settled() {
            self.lines.discard();
        }
        emitted
    }

    pub(super) fn finish(mut self) -> CodexOutput {
        if self.output.is_settled() {
            self.lines.discard();
        } else if let Some(line) = self.lines.finish() {
            self.accept_line(line);
        }
        self.output.finalize();
        self.output
    }

    pub(super) fn finish_stream(&mut self) -> Vec<CodexEmission> {
        if self.output.is_settled() {
            self.lines.discard();
            return Vec::new();
        }
        self.lines
            .finish()
            .map_or_else(Vec::new, |line| self.accept_line(line))
    }

    fn accept_line(&mut self, line: ProviderJsonLine) -> Vec<CodexEmission> {
        match line {
            ProviderJsonLine::Oversized => {
                self.output.oversized_records = self.output.oversized_records.saturating_add(1);
                Vec::new()
            }
            ProviderJsonLine::Record(line) => match self.output.accept_bytes(&line) {
                AcceptedLine::Emission(emission) => vec![emission],
                AcceptedLine::Emissions(emissions) => emissions,
                AcceptedLine::Ignored => Vec::new(),
                AcceptedLine::Malformed => {
                    self.output.malformed_records = self.output.malformed_records.saturating_add(1);
                    Vec::new()
                }
                AcceptedLine::ThreadFailure(detail) => {
                    if self.output.thread_failure.is_none() {
                        self.output.thread_failure = Some(detail);
                    }
                    Vec::new()
                }
            },
        }
    }
}

impl CodexOutput {
    #[cfg(test)]
    pub(super) fn parse(bytes: &[u8]) -> Self {
        let mut decoder = CodexOutputDecoder::new();
        decoder.push(bytes);
        decoder.finish()
    }

    fn empty() -> Self {
        Self {
            thread_id: None,
            task_started: false,
            thread_failure: None,
            final_message: None,
            terminal: None,
            failure: None,
            token_usage: None,
            malformed_records: 0,
            oversized_records: 0,
            tools: ToolActivity::default(),
        }
    }

    pub(super) fn provider_failure(detail: String) -> Self {
        let mut output = Self::empty();
        output.merge_failure_detail(detail);
        output
    }

    fn is_settled(&self) -> bool {
        self.terminal.is_some()
    }

    pub(super) fn is_completed(&self) -> bool {
        self.terminal == Some(CodexTerminal::Completed)
            && self.final_message.is_some()
            && self.thread_failure.is_none()
            && self.failure.is_none()
    }

    fn finalize(&mut self) {
        self.finalize_terminal();
        self.prepend_thread_failure();
    }

    fn finalize_terminal(&mut self) {
        match self.terminal {
            Some(CodexTerminal::Completed) if self.final_message.is_none() => {
                self.failure = Some(
                    self.incomplete_detail("Codex turn completed without a final agent message"),
                );
            }
            Some(CodexTerminal::Completed) => self.failure = None,
            Some(CodexTerminal::Failed) => {
                if self.failure.is_none() {
                    self.failure =
                        Some(self.incomplete_detail("Codex turn failed without provider detail"));
                }
            }
            None => {
                if self.failure.is_none() {
                    self.failure = Some(
                        self.incomplete_detail("Codex output ended without a terminal turn event"),
                    );
                }
            }
        }
    }

    fn prepend_thread_failure(&mut self) {
        if let Some(thread_failure) = self.thread_failure.take() {
            self.failure = Some(match self.failure.take() {
                Some(failure) if failure == thread_failure => thread_failure,
                Some(failure) => format!("{thread_failure}; {failure}"),
                None => thread_failure,
            });
        }
    }

    fn incomplete_detail(&self, message: &str) -> String {
        match (self.malformed_records, self.oversized_records) {
            (0, 0) => message.to_owned(),
            (malformed, oversized) => format!(
                "{message}; ignored {malformed} malformed and {oversized} oversized output records"
            ),
        }
    }

    pub(super) fn merge_failure_detail(&mut self, detail: String) {
        let detail = detail.trim();
        if detail.is_empty() {
            return;
        }
        self.failure = Some(match self.failure.take() {
            Some(existing) if existing == detail => existing,
            Some(existing) => format!("{existing}; {detail}"),
            None => detail.to_owned(),
        });
        self.terminal = Some(CodexTerminal::Failed);
    }

    pub(super) fn failure_message(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    pub(super) fn token_usage(&self) -> Option<TokenUsageDelta> {
        self.token_usage
    }

    pub(super) fn final_message(&self) -> Result<&str, NodeRunnerError> {
        self.final_message.as_deref().ok_or(NodeRunnerError::Driver)
    }

    fn accept_bytes(&mut self, line: &[u8]) -> AcceptedLine {
        if line.iter().all(u8::is_ascii_whitespace) {
            return AcceptedLine::Ignored;
        }
        let Ok(event) = parse_provider_json(line) else {
            return AcceptedLine::Malformed;
        };
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            return AcceptedLine::Malformed;
        };
        if event_type == "turn.started" {
            self.task_started = true;
        }
        if event_type == "thread.started" {
            return self.accept_thread(&event);
        }
        if matches!(
            event_type,
            "item.started" | "item.updated" | "item.completed"
        ) {
            return self.accept_item(&event, event_type);
        }
        self.accept_turn_event(&event, event_type)
    }

    fn accept_turn_event(&mut self, event: &Value, event_type: &str) -> AcceptedLine {
        if self.is_settled() {
            return AcceptedLine::Ignored;
        }
        let message = match event_type {
            "turn.started" => "Codex turn started",
            "turn.completed" => {
                self.terminal = Some(CodexTerminal::Completed);
                self.failure = None;
                self.record_terminal_usage(event);
                "Codex turn completed"
            }
            "turn.failed" => {
                self.terminal = Some(CodexTerminal::Failed);
                self.record_terminal_usage(event);
                if let Some(detail) = failure_detail(event, event_type) {
                    self.failure = Some(detail);
                }
                "Codex turn failed"
            }
            "error" => {
                if event.get("usage").is_some() {
                    self.token_usage = token_usage(event.get("usage"));
                }
                let detail = failure_detail(event, event_type);
                let message = detail.as_deref().map_or_else(
                    || "Codex stream error".to_owned(),
                    |detail| format!("Codex stream error: {detail}"),
                );
                self.failure = detail
                    .or_else(|| Some("Codex stream error without provider detail".to_owned()));
                return AcceptedLine::Emission(CodexEmission::StreamError(message));
            }
            _ => return AcceptedLine::Ignored,
        };
        AcceptedLine::Emission(CodexEmission::Progress(message.to_owned()))
    }

    fn record_terminal_usage(&mut self, event: &Value) {
        match terminal_usage(event) {
            TerminalUsage::Empty => {}
            TerminalUsage::Valid(usage) => self.token_usage = Some(usage),
            TerminalUsage::Malformed => self.token_usage = None,
        }
    }

    fn accept_thread(&mut self, event: &Value) -> AcceptedLine {
        let Some(thread_id) = event
            .get("thread_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return AcceptedLine::Malformed;
        };
        match self.thread_id.as_deref() {
            Some(current) if current != thread_id => AcceptedLine::ThreadFailure(
                "Codex output contained conflicting thread IDs".to_owned(),
            ),
            Some(_) => AcceptedLine::Ignored,
            None => {
                self.thread_id = Some(thread_id.to_owned());
                AcceptedLine::Ignored
            }
        }
    }

    fn accept_item(&mut self, event: &Value, event_type: &str) -> AcceptedLine {
        let Some(item) = event.get("item") else {
            return AcceptedLine::Malformed;
        };
        let Some(item_type) = item.get("type").and_then(Value::as_str) else {
            return AcceptedLine::Malformed;
        };
        let Some(phase) = event_type.strip_prefix("item.") else {
            return AcceptedLine::Malformed;
        };
        if matches!(
            item_type,
            "command_execution"
                | "file_change"
                | "mcp_tool_call"
                | "collab_tool_call"
                | "web_search"
        ) {
            return self.accept_tool(item, item_type, phase);
        }
        match item_type {
            "agent_message" if event_type == "item.completed" => {
                let Some(message) = item.get("text").and_then(Value::as_str) else {
                    return AcceptedLine::Malformed;
                };
                self.final_message = Some(message.to_owned());
                AcceptedLine::Emission(CodexEmission::AgentMessage(message.to_owned()))
            }
            "agent_message" => AcceptedLine::Ignored,
            _ => AcceptedLine::Emission(CodexEmission::Progress(semantic_item(
                item, item_type, phase,
            ))),
        }
    }
    fn accept_tool(&mut self, item: &Value, kind: &str, phase: &str) -> AcceptedLine {
        if phase == "updated" {
            return AcceptedLine::Ignored;
        }
        let name = match kind {
            "mcp_tool_call" => format!(
                "{}.{}",
                item["server"].as_str().unwrap_or("unknown"),
                item["tool"].as_str().unwrap_or("unknown")
            ),
            "collab_tool_call" => item["tool"].as_str().unwrap_or("unknown").to_owned(),
            _ => kind.to_owned(),
        };
        let identity = ToolIdentity {
            name,
            id: item["id"].as_str().map(str::to_owned),
        };
        let mut emissions = Vec::new();
        if let Some(identity) = self.tools.record(identity.clone(), false) {
            let input = match kind {
                "command_execution" => item["command"].clone(),
                "file_change" => item["changes"].clone(),
                "mcp_tool_call" => item["arguments"].clone(),
                "collab_tool_call" => serde_json::json!({
                    "sender_thread_id":item["sender_thread_id"],
                    "receiver_thread_ids":item["receiver_thread_ids"], "prompt":item["prompt"]
                }),
                // `query` may only summarize the first query; retain the complete native action.
                _ => serde_json::json!({"action":item["action"], "query":item["query"]}),
            };
            emissions.push(CodexEmission::Tool(ToolEntry::call(
                "Codex", identity, input,
            )));
        }
        if phase == "completed" {
            if let Some(identity) = self.tools.record(identity, true) {
                let mut status = item["status"].as_str().unwrap_or("unknown").to_owned();
                if let Some(exit) = item["exit_code"].as_i64() {
                    status.push_str(&format!(" exit={exit}"));
                }
                let result = match kind {
                    "command_execution" => item["aggregated_output"].clone(),
                    "file_change" => item["changes"].clone(),
                    "mcp_tool_call" => {
                        serde_json::json!({"result":item["result"], "error":item["error"]})
                    }
                    "collab_tool_call" => serde_json::json!({
                        "sender_thread_id":item["sender_thread_id"],
                        "receiver_thread_ids":item["receiver_thread_ids"],
                        "agents_states":item["agents_states"]
                    }),
                    _ => serde_json::json!({"action":item["action"], "query":item["query"]}),
                };
                emissions.push(CodexEmission::Tool(ToolEntry::completed(
                    "Codex", identity, status, result,
                )));
            }
        }
        AcceptedLine::Emissions(emissions)
    }
}

fn token_usage(value: Option<&Value>) -> Option<TokenUsageDelta> {
    parse_token_usage_delta(
        value,
        Some("cached_input_tokens"),
        Some("cache_write_input_tokens"),
    )
}

fn terminal_usage(event: &Value) -> TerminalUsage {
    match event.get("usage") {
        None | Some(Value::Null) => TerminalUsage::Empty,
        Some(Value::Object(usage)) if usage.is_empty() => TerminalUsage::Empty,
        value => token_usage(value).map_or(TerminalUsage::Malformed, TerminalUsage::Valid),
    }
}

fn failure_detail(event: &Value, event_type: &str) -> Option<String> {
    let detail = if event_type == "turn.failed" {
        event.get("error").and_then(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
        })
    } else {
        event.get("message").and_then(Value::as_str).or_else(|| {
            event
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
        })
    };
    detail
        .filter(|message| !message.trim().is_empty())
        .map(str::to_owned)
}

fn semantic_item(item: &Value, item_type: &str, phase: &str) -> String {
    match item_type {
        "reasoning" => semantic_reasoning(item, phase),
        "todo_list" => semantic_todo_list(item, phase),
        "error" => format!(
            "Codex activity error: {}",
            item.get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        ),
        _ => format!("Codex activity {phase}: {item_type}"),
    }
}

fn semantic_reasoning(item: &Value, phase: &str) -> String {
    match item.get("text").and_then(Value::as_str) {
        Some(text) if !text.is_empty() => format!("Codex reasoning {phase}: {text}"),
        _ => format!("Codex reasoning {phase}"),
    }
}

fn semantic_todo_list(item: &Value, phase: &str) -> String {
    let items = item
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let text = item.get("text")?.as_str()?;
                    let marker = if item.get("completed").and_then(Value::as_bool) == Some(true) {
                        "done"
                    } else {
                        "pending"
                    };
                    Some(format!("{marker}: {text}"))
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|items| !items.is_empty())
        .unwrap_or_else(|| "empty".to_owned());
    format!("Codex plan {phase}: {items}")
}

#[cfg(test)]
#[path = "output/tests.rs"]
mod tests;
