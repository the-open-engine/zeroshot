use std::collections::VecDeque;
use std::time::Duration;

use openengine_cluster_protocol::{TokenCount, WorkerOutcome};
use serde_json::{Value, json};

use crate::execution::process::{
    ProcessFrame, ProcessRunnerError, ProcessStdout, MAX_PROCESS_MESSAGE_BYTES,
};
use crate::native_v2_capsule::provider_json_lines::{ProviderJsonLine, ProviderJsonLines};
use crate::native_v2_capsule::provider_process::{
    ProviderEmission, ProviderFailureRetry, ProviderProcess, require_process_cleanup,
    report_cleanup_failure,
};
use crate::native_v2_capsule::provider_process::text::ProviderText;
use crate::native_v2_contract::TokenUsageDelta;
use crate::native_v2_runner::{
    AgentResponseState, DriverControl, DriverInvocation, LiveOutput, LiveOutputStream,
    NodeRunnerError, ProviderSchemaDialect, resolve_agent_response_with_schema,
};

const STARTUP_BUDGET: Duration = Duration::from_secs(60);
// Native LSP shutdown may consume ten seconds and MCP shutdown another five. Keep one
// absolute bound around all native settlement before falling back to process containment.
const CLEANUP_BUDGET: Duration = Duration::from_secs(30);

pub(super) struct HermesConnection<'a> {
    stdout: ProcessStdout,
    control: &'a DriverControl,
    lines: ProviderJsonLines,
    pending: VecDeque<ProviderJsonLine>,
    output: ProviderText,
    eof: bool,
    activated: bool,
    closed: bool,
    usage_reported: bool,
    redactions: Vec<String>,
    native_failure: Option<String>,
}

impl<'a> HermesConnection<'a> {
    pub(super) fn new(
        stdout: ProcessStdout,
        control: &'a DriverControl,
        redactions: Vec<String>,
    ) -> Self {
        Self {
            stdout,
            control,
            lines: ProviderJsonLines::new(),
            pending: VecDeque::new(),
            output: ProviderText::new(&redactions),
            eof: false,
            activated: false,
            closed: false,
            usage_reported: false,
            redactions,
            native_failure: None,
        }
    }

    pub(super) async fn execute(
        &mut self,
        process: &ProviderProcess,
        invocation: &DriverInvocation,
        init: Value,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let mut cancellation = self.control.cancellation();
        tokio::select! {
            _ = cancellation.cancelled() => Err(NodeRunnerError::Cancelled),
            result = self.converse(process, invocation, init) => result,
        }
    }

    async fn converse(
        &mut self,
        process: &ProviderProcess,
        invocation: &DriverInvocation,
        init: Value,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        self.start(process, init).await?;
        let dialect = ProviderSchemaDialect::Standard;
        let mut response = response_state(invocation)?;
        let feedback = ProviderFailureRetry::new(
            "Hermes",
            response.prompt().to_owned(),
            self.redactions.clone(),
        );
        loop {
            send(process, &json!({"type":"prompt","text":response.prompt()})).await?;
            let message = self.turn().await?;
            let answer =
                resolve_agent_response_with_schema(&invocation.response, &message, dialect)?;
            feedback
                .report_correction(self.control, &answer, response.corrections_exhausted())
                .await?;
            if let Some(outcome) = response.accept("Hermes", self.control, answer).await? {
                return Ok(outcome);
            }
        }
    }

    async fn start(
        &mut self,
        process: &ProviderProcess,
        init: Value,
    ) -> Result<(), NodeRunnerError> {
        tokio::time::timeout(STARTUP_BUDGET, self.initialize(process, init))
            .await
            .map_err(|_| failure("Hermes native runtime startup timed out"))?
    }

    async fn initialize(
        &mut self,
        process: &ProviderProcess,
        init: Value,
    ) -> Result<(), NodeRunnerError> {
        let ready = self.required().await.map_err(|_| failure(
            "Hermes native embedding bootstrap is unavailable; install a compatible Hermes executable"))?;
        if ready["type"] != "ready" || ready["protocol"] != 1 {
            return Err(failure("Hermes native embedding handshake is incompatible"));
        }
        let session_id = init["sessionId"]
            .as_str()
            .ok_or_else(|| failure("Hermes session identity is missing"))?
            .to_owned();
        self.activated = true;
        send(process, &init).await?;
        let initialized = self.required().await?;
        if initialized["type"] != "initialized" || initialized["sessionId"] != session_id {
            return Err(failure("Hermes native session initialization failed"));
        }
        Ok(())
    }

    async fn turn(&mut self) -> Result<String, NodeRunnerError> {
        loop {
            let event = self.required().await?;
            if event["type"] == "result" {
                self.record_usage(event.get("usage")).await?;
                if event["failed"] == true
                    || event["interrupted"] == true
                    || event["completed"] != true
                {
                    return Err(self.turn_failure(&event));
                }
                return event["message"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| failure("Hermes native turn has no response"));
            }
            self.observe(&event, false).await?;
        }
    }

    fn turn_failure(&self, event: &Value) -> NodeRunnerError {
        let detail = event["error"]
            .as_str()
            .filter(|detail| !detail.trim().is_empty());
        match detail {
            Some(detail) => failure(&format!(
                "Hermes native model turn failed: {}",
                self.output.redact(detail)
            )),
            None => failure("Hermes native model turn did not complete"),
        }
    }

    async fn record_usage(&mut self, value: Option<&Value>) -> Result<(), NodeRunnerError> {
        self.usage_reported = true;
        self.control.record_token_usage(value.and_then(usage)).await
    }

    async fn observe(&mut self, event: &Value, final_output: bool) -> Result<(), NodeRunnerError> {
        let mut emissions = Vec::new();
        match event["type"].as_str() {
            Some("text") => {
                if let Some(text) = event["text"].as_str() {
                    self.output
                        .push(&mut emissions, LiveOutputStream::Output, text);
                }
            }
            Some("tool") => {
                if let Some(name) = event["name"].as_str() {
                    self.output.emit(
                        &mut emissions,
                        LiveOutputStream::System,
                        &format!("Hermes tool: {name}"),
                    );
                }
            }
            Some("usage") => self.record_usage(event.get("usage")).await?,
            Some("closed") => self.closed = event["clean"] == true,
            Some("error") => {
                return Err(self.native_error(event));
            }
            _ => {}
        }
        self.publish(emissions, final_output).await
    }

    async fn publish(
        &self,
        emissions: Vec<ProviderEmission>,
        final_output: bool,
    ) -> Result<(), NodeRunnerError> {
        for emission in emissions {
            let output = LiveOutput::new(emission.stream, emission.text)?;
            if final_output {
                self.control.emit_final(output).await?;
            } else {
                self.control.emit(output).await?;
            }
        }
        Ok(())
    }

    async fn next(&mut self) -> Result<Option<Value>, NodeRunnerError> {
        loop {
            if let Some(record) = self.pending.pop_front() {
                return match record {
                    ProviderJsonLine::Record(bytes) => serde_json::from_slice(&bytes)
                        .map(Some)
                        .map_err(|_| failure("Hermes emitted an invalid private event")),
                    ProviderJsonLine::Oversized => {
                        Err(failure("Hermes emitted an oversized private event"))
                    }
                };
            }
            if self.eof {
                return Ok(None);
            }
            if let Some(chunk) = self.stdout.recv().await {
                self.pending.extend(self.lines.push(chunk.as_slice()));
            } else {
                self.eof = true;
                self.pending.extend(self.lines.finish());
            }
        }
    }

    async fn required(&mut self) -> Result<Value, NodeRunnerError> {
        let event = self
            .next()
            .await?
            .ok_or_else(|| failure("Hermes native event stream closed unexpectedly"))?;
        if event["type"] == "error" {
            return Err(self.native_error(&event));
        }
        Ok(event)
    }

    fn native_error(&mut self, event: &Value) -> NodeRunnerError {
        let detail = self.output.redact(
            event["message"]
                .as_str()
                .unwrap_or("Hermes native runtime failed"),
        );
        self.native_failure = Some(detail.clone());
        NodeRunnerError::DriverDetail(detail)
    }

    async fn shutdown(&mut self, process: &mut ProviderProcess) -> Result<(), NodeRunnerError> {
        self.request_stop(process).await?;
        while let Some(event) = self.next().await? {
            if event["type"] == "error" {
                let _ = self.native_error(&event);
            } else {
                self.observe(&event, true).await?;
            }
        }
        if self.activated && !self.closed {
            return Err(NodeRunnerError::CleanupUnconfirmed);
        }
        Ok(())
    }

    async fn request_stop(&mut self, process: &mut ProviderProcess) -> Result<(), NodeRunnerError> {
        if self.activated && !self.eof {
            send(process, &json!({"type":"stop"})).await?;
            process.close_stdin().await.map_err(process_error)?;
        }
        Ok(())
    }

    pub(super) async fn finish(
        &mut self,
        process: &mut ProviderProcess,
        kill: tokio::sync::watch::Sender<bool>,
        outcome: Result<WorkerOutcome, NodeRunnerError>,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let shutdown = tokio::time::timeout(CLEANUP_BUDGET, self.shutdown(process)).await;
        let native_clean = matches!(shutdown, Ok(Ok(())));
        if !native_clean {
            process.retain_execution_files();
            kill.send_replace(true);
        }
        let completion = if native_clean {
            process.wait().await
        } else {
            process.release().await
        };
        let published = self.final_output(native_clean, outcome.is_ok()).await;
        if require_process_cleanup(&completion).is_err() || !native_clean {
            let diagnostic = self
                .output
                .failure_diagnostic("Hermes", self.native_failure.as_deref())
                .unwrap_or_else(|| {
                    "Hermes native or process cleanup could not be confirmed".to_owned()
                });
            report_cleanup_failure(self.control, diagnostic).await;
            return Err(NodeRunnerError::CleanupUnconfirmed);
        }
        let completed = completion.map_err(process_error)?;
        if self.control.is_cancelled() {
            return Err(NodeRunnerError::Cancelled);
        }
        if completed.exit_code != Some(0) && outcome.is_ok() {
            return Err(failure("Hermes native runtime exited unsuccessfully"));
        }
        published?;
        outcome
    }

    async fn final_output(
        &mut self,
        native_clean: bool,
        successful: bool,
    ) -> Result<(), NodeRunnerError> {
        if !self.usage_reported || !native_clean || !successful {
            self.control.record_token_usage(None).await?;
        }
        let mut emissions = Vec::new();
        self.output.finish(&mut emissions);
        self.publish(emissions, true).await
    }
}

async fn send(process: &ProviderProcess, value: &Value) -> Result<(), NodeRunnerError> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| NodeRunnerError::Driver)?;
    bytes.push(b'\n');
    for chunk in bytes.chunks(MAX_PROCESS_MESSAGE_BYTES) {
        process
            .send(ProcessFrame::new(chunk.to_vec()).map_err(process_error)?)
            .await
            .map_err(process_error)?;
    }
    Ok(())
}

fn response_state(invocation: &DriverInvocation) -> Result<AgentResponseState, NodeRunnerError> {
    invocation
        .agent_prompt_with_schema(ProviderSchemaDialect::Standard)
        .map(AgentResponseState::new)
        .map_err(|_| failure("Hermes prompt could not be serialized"))
}

fn usage(value: &Value) -> Option<TokenUsageDelta> {
    Some(TokenUsageDelta {
        input_tokens: TokenCount::new(value["input"].as_u64()?).ok()?,
        output_tokens: TokenCount::new(value["output"].as_u64()?).ok()?,
        cache_read_input_tokens: optional_token(value.get("cacheRead"))?,
        cache_creation_input_tokens: optional_token(value.get("cacheWrite"))?,
    })
}

fn optional_token(value: Option<&Value>) -> Option<Option<TokenCount>> {
    match value.filter(|value| !value.is_null()) {
        Some(value) => TokenCount::new(value.as_u64()?).ok().map(Some),
        None => Some(None),
    }
}

pub(super) fn process_error(error: ProcessRunnerError) -> NodeRunnerError {
    failure(&format!("Hermes process failed: {error}"))
}
fn failure(message: &str) -> NodeRunnerError {
    NodeRunnerError::DriverDetail(message.to_owned())
}
