use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use openengine_cluster_protocol::{FailureDiagnosticKind, WorkerOutcome};
use crate::execution::process::{
    ProcessFrame, ProcessRunnerError, ProcessStdout, MAX_PROCESS_MESSAGE_BYTES,
};
use crate::native_v2_capsule::provider_process::{
    ProviderProcess, ProviderExecutionFiles, provider_error_output, redaction_values,
    report_provider_error, safe_provider_text,
};
use crate::native_v2_runner::{
    AgentResponseState, DriverControl, DriverInvocation, NodeRunnerError, ProviderSchemaDialect,
    resolve_agent_response_with_dialect,
};
use super::{auth, command, events, framing::Frames, provider, session::CopilotSession};

pub(super) fn failure(message: impl Into<String>) -> NodeRunnerError {
    NodeRunnerError::DriverDetail(message.into())
}

pub(super) fn process_error(error: ProcessRunnerError) -> NodeRunnerError {
    failure(format!("Copilot process failed: {error}"))
}

#[derive(Debug, PartialEq)]
enum RpcReply {
    Result(Value),
    Error(String),
    Ended,
}

impl RpcReply {
    fn into_value(self, method: &str) -> Result<Value, NodeRunnerError> {
        match self {
            Self::Result(value) => Ok(value),
            Self::Error(message) => Err(failure(format!("Copilot RPC failed: {message}"))),
            Self::Ended => Err(request_ended(method)),
        }
    }
}

pub(super) async fn write_messages(
    process: &ProviderProcess,
    mut receiver: mpsc::Receiver<Value>,
) -> Result<(), NodeRunnerError> {
    while let Some(message) = receiver.recv().await {
        let body = serde_json::to_vec(&message)
            .map_err(|_| failure("Copilot RPC serialization failed"))?;
        if body.len() > super::framing::MAX_BODY {
            return Err(failure("Copilot RPC request exceeds 64 MiB"));
        }
        let written = write_body(process, &body).await;
        match written {
            Err(ProcessRunnerError::Io(_)) if message["method"] == "session.detach" => {
                // Detach is queued only after acceptance. ProcessSession owns pipe-failure
                // cleanup; let RPC observe EOF and report the non-fatal detach failure.
                return Ok(());
            }
            result => result.map_err(process_error)?,
        }
    }
    Ok(())
}

async fn write_body(process: &ProviderProcess, body: &[u8]) -> Result<(), ProcessRunnerError> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    process
        .send(ProcessFrame::new(header.into_bytes())?)
        .await?;
    for chunk in body.chunks(MAX_PROCESS_MESSAGE_BYTES) {
        process.send(ProcessFrame::new(chunk.to_vec())?).await?;
    }
    Ok(())
}

pub(super) struct CopilotRpc<'a> {
    stdout: ProcessStdout,
    frames: Frames,
    pub(super) invocation: &'a DriverInvocation,
    pub(super) control: &'a DriverControl,
    sender: Option<mpsc::Sender<Value>>,
    next_id: u64,
    pending: BTreeSet<u64>,
    pub(super) session_id: String,
    pub(super) response: Option<String>,
    pub(super) redactions: Vec<String>,
    authentication: auth::CopilotAuthentication<'a>,
    provider: Option<&'a provider::LocalProvider>,
    active_memory: Option<Arc<crate::active_memory::hooks::ActiveMemoryHooks>>,
    workspace: PathBuf,
}

pub(super) struct CopilotRpcNative<'a> {
    pub authentication: auth::CopilotAuthentication<'a>,
    pub provider: Option<&'a provider::LocalProvider>,
    pub redactions: Vec<String>,
}

impl<'a> CopilotRpc<'a> {
    pub(super) fn new(
        stdout: ProcessStdout,
        invocation: &'a DriverInvocation,
        control: &'a DriverControl,
        native: CopilotRpcNative<'a>,
    ) -> Self {
        let redactions = redaction_values(
            invocation
                .environment
                .iter()
                .map(|(_, value)| value)
                .chain(native.redactions.iter().map(String::as_str))
                .chain(native.authentication.token()),
        );
        Self {
            stdout,
            frames: Frames::default(),
            invocation,
            control,
            sender: None,
            next_id: 0,
            pending: BTreeSet::new(),
            session_id: String::new(),
            response: None,
            redactions,
            authentication: native.authentication,
            provider: native.provider,
            active_memory: None,
            workspace: PathBuf::new(),
        }
    }

    pub(super) fn with_active_memory(
        mut self,
        memory: Option<Arc<crate::active_memory::hooks::ActiveMemoryHooks>>,
        workspace: PathBuf,
    ) -> Self {
        self.active_memory = memory;
        self.workspace = workspace;
        self
    }

    pub(super) async fn run(
        &mut self,
        sender: mpsc::Sender<Value>,
        session: &CopilotSession,
        files: &ProviderExecutionFiles,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        self.sender = Some(sender);
        let result = self.run_session(session, files).await;
        self.sender.take();
        result.map_err(|error| match error {
            NodeRunnerError::DriverDetail(message) => {
                failure(safe_provider_text(&message, &self.redactions))
            }
            error => error,
        })
    }

    async fn run_session(
        &mut self,
        session: &CopilotSession,
        files: &ProviderExecutionFiles,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        self.connect().await?;
        let resume = session.id.lock().await.clone();
        self.session_id = resume
            .clone()
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let mut params = command::session_parameters(
            self.invocation,
            files,
            &self.session_id,
            command::SessionProvider {
                authentication: &self.authentication,
                local: self.provider,
            },
        )?;
        if self.active_memory.is_some() {
            params["hooks"] = json!(true);
        }
        let method = if resume.is_some() {
            "session.resume"
        } else {
            "session.create"
        };
        let created = self.request(method, params).await?;
        validate_session_identity(&created, &self.session_id)?;
        *session.id.lock().await = Some(self.session_id.clone());
        let outcome = self.run_response().await?;
        self.detach().await?;
        Ok(outcome)
    }

    /// Detaching only releases a session whose answer is already accepted. Older protocol-3 CLIs
    /// such as 1.0.70 lack the method, so a provider failure is reported and the outcome is kept.
    async fn detach(&mut self) -> Result<(), NodeRunnerError> {
        let reply = self
            .request_reply("session.detach", json!({"sessionId":self.session_id}))
            .await?;
        let detail = match reply {
            RpcReply::Result(detached) => {
                if validate_detach(&detached)? {
                    return Ok(());
                }
                "Copilot session could not be detached".to_owned()
            }
            RpcReply::Error(message) => format!("Copilot RPC failed: {message}"),
            RpcReply::Ended => "Copilot exited before replying to session.detach".to_owned(),
        };
        let error = failure(format!(
            "session detach failed after the accepted answer: {detail}"
        ));
        report_provider_error("Copilot", &error, &self.redactions, self.control).await
    }

    async fn connect(&mut self) -> Result<(), NodeRunnerError> {
        let connected = self
            .request(
                "connect",
                json!({"clientInfo":{"extensionName":"zeroshot"}}),
            )
            .await?;
        validate_protocol_version(&connected)
    }

    async fn run_response(&mut self) -> Result<WorkerOutcome, NodeRunnerError> {
        let prompt = self.invocation.agent_prompt()?;
        let mut response = AgentResponseState::new(prompt);
        loop {
            self.response = None;
            self.request("session.send", json!({
                "sessionId":self.session_id, "prompt":response.prompt(), "wait":true,
                "responseFormat":{"type":"json_schema", "jsonSchema":{
                    "name":"zeroshot_response", "strict":true,
                    "schema":self.invocation.response.provider_schema(ProviderSchemaDialect::OpenAiStrict),
                }},
            })).await?;
            let text = self
                .response
                .take()
                .ok_or_else(|| failure("Copilot completed without an assistant response"))?;
            if let Some(outcome) = self.accept_response(&mut response, &text).await? {
                return Ok(outcome);
            }
        }
    }

    async fn accept_response(
        &self,
        state: &mut AgentResponseState,
        text: &str,
    ) -> Result<Option<WorkerOutcome>, NodeRunnerError> {
        let response = resolve_agent_response_with_dialect(
            &self.invocation.response,
            text,
            ProviderSchemaDialect::OpenAiStrict,
        )?;
        if let Some(error) = response.correction_error() {
            if let Some(mut output) = provider_error_output("Copilot", &error, &self.redactions)? {
                output.diagnostic = state
                    .corrections_exhausted()
                    .then_some(FailureDiagnosticKind::Failure);
                self.control.emit(output).await?;
            }
        }
        state.accept("Copilot", self.control, response).await
    }

    pub(super) fn queue(&self, value: Value) -> Result<(), NodeRunnerError> {
        self.sender
            .as_ref()
            .ok_or_else(|| failure("Copilot RPC is closed"))?
            .try_send(value)
            .map_err(|_| failure("Copilot RPC output queue is full or closed"))
    }

    pub(super) fn send_request(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<u64, NodeRunnerError> {
        let id = reserve_request_id(&mut self.next_id, self.pending.len())?;
        self.queue(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))?;
        self.pending.insert(id);
        Ok(id)
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, NodeRunnerError> {
        self.request_reply(method, params).await?.into_value(method)
    }

    async fn request_reply(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<RpcReply, NodeRunnerError> {
        let expected = self.send_request(method, params)?;
        while let Some(message) = self.next().await? {
            if message.get("method").is_some() {
                self.dispatch(&message).await?;
            } else if let Some(reply) =
                resolve_response_message(&mut self.pending, &message, expected)?
            {
                return Ok(reply);
            }
        }
        Ok(RpcReply::Ended)
    }

    async fn dispatch(&mut self, message: &Value) -> Result<(), NodeRunnerError> {
        match dispatch_action(message) {
            DispatchAction::Event => events::receive(self, &message["params"]).await,
            DispatchAction::AcquireToken => self.acquire_token(message).await,
            DispatchAction::MemoryHook => self.memory_hook(message),
            DispatchAction::Reject => self.queue(json!({"jsonrpc":"2.0", "id":message["id"],
                "error":{"code":-32601,"message":"Method not supported by Zeroshot"}})),
            DispatchAction::Ignore => Ok(()),
        }
    }

    fn memory_hook(&self, message: &Value) -> Result<(), NodeRunnerError> {
        let params = &message["params"];
        let context = if params["sessionId"].as_str() == Some(&self.session_id)
            && params["hookType"].as_str() == Some("preToolUse")
        {
            self.active_memory
                .as_ref()
                .map(|memory| {
                    memory.copilot_context(self.invocation, &params["input"], &self.workspace)
                })
                .filter(|value| !value.is_empty())
        } else {
            None
        };
        let output = context.map_or(Value::Null, |context| json!({"additionalContext":context}));
        self.queue(json!({"jsonrpc":"2.0","id":message["id"],"result":{"output":output}}))
    }

    async fn acquire_token(&mut self, message: &Value) -> Result<(), NodeRunnerError> {
        let acquired = self
            .authentication
            .acquire(&message["params"], &self.session_id)
            .await?;
        for (_, value) in acquired.iter() {
            if !self.redactions.iter().any(|current| current == value) {
                if self.redactions.len() >= 128 {
                    return Err(failure("Copilot credential rotation limit exceeded"));
                }
                self.redactions.push(value.to_owned());
            }
        }
        let result = auth::result(&acquired)?;
        self.queue(json!({"jsonrpc":"2.0", "id":message["id"], "result":result}))
    }

    async fn next(&mut self) -> Result<Option<Value>, NodeRunnerError> {
        loop {
            if let Some(message) = self.frames.pop() {
                if message["jsonrpc"].as_str() != Some("2.0") {
                    return Err(failure("Copilot JSON-RPC version is invalid"));
                }
                return Ok(Some(message));
            }
            let Some(chunk) = self.stdout.recv().await else {
                self.frames.finish()?;
                return Ok(None);
            };
            self.frames.push(chunk.as_slice())?;
        }
    }

    pub(super) fn failure_diagnostic(
        &self,
        error: NodeRunnerError,
        completion: &crate::execution::process::ProcessSessionOutput,
    ) -> NodeRunnerError {
        let mut detail = match failure_detail(error) {
            Ok(detail) => detail,
            Err(error) => return error,
        };
        append_stderr_detail(&mut detail, completion);
        failure(safe_provider_text(&detail, &self.redactions))
    }

    pub(super) async fn drain(&mut self) -> Result<(), NodeRunnerError> {
        self.sender.take();
        let mut error = self.drain_ready().await.err();
        while let Some(chunk) = self.stdout.recv().await {
            if error.is_some() {
                continue;
            }
            error = match self.frames.push(chunk.as_slice()) {
                Ok(()) => self.drain_ready().await.err(),
                Err(error) => Some(error),
            };
        }
        match error {
            Some(error) => Err(error),
            None => self.frames.finish(),
        }
    }

    async fn drain_ready(&mut self) -> Result<(), NodeRunnerError> {
        while let Some(message) = self.frames.pop() {
            if message["method"].as_str() == Some("session.event") {
                events::receive_usage(self, &message["params"]).await?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DispatchAction {
    Event,
    AcquireToken,
    MemoryHook,
    Reject,
    Ignore,
}

fn dispatch_action(message: &Value) -> DispatchAction {
    match message["method"].as_str() {
        Some("session.event") => DispatchAction::Event,
        Some("gitHubToken.getToken") if message.get("id").is_some() => DispatchAction::AcquireToken,
        Some("hooks.invoke") if message.get("id").is_some() => DispatchAction::MemoryHook,
        _ if message.get("id").is_some() => DispatchAction::Reject,
        _ => DispatchAction::Ignore,
    }
}

fn validate_protocol_version(connected: &Value) -> Result<(), NodeRunnerError> {
    (connected["protocolVersion"].as_u64() == Some(3))
        .then_some(())
        .ok_or_else(|| failure("Copilot RPC protocol version is incompatible; install CLI 1.0.86"))
}

fn validate_session_identity(created: &Value, expected: &str) -> Result<(), NodeRunnerError> {
    (created["sessionId"].as_str() == Some(expected))
        .then_some(())
        .ok_or_else(|| failure("Copilot returned a different session identity"))
}

fn validate_detach(detached: &Value) -> Result<bool, NodeRunnerError> {
    detached["success"]
        .as_bool()
        .ok_or_else(|| failure("Copilot session detach result is invalid"))
}

fn request_ended(method: &str) -> NodeRunnerError {
    failure(format!("Copilot exited before replying to {method}"))
}

fn failure_detail(error: NodeRunnerError) -> Result<String, NodeRunnerError> {
    match error {
        NodeRunnerError::Driver => Ok("execution failed".to_owned()),
        NodeRunnerError::DriverDetail(detail) => Ok(detail),
        error => Err(error),
    }
}

fn reserve_request_id(next_id: &mut u64, pending: usize) -> Result<u64, NodeRunnerError> {
    if pending >= 128 {
        return Err(failure("Copilot RPC pending request limit exceeded"));
    }
    *next_id = next_id
        .checked_add(1)
        .ok_or_else(|| failure("Copilot RPC identifier overflow"))?;
    Ok(*next_id)
}

fn resolve_response_message(
    pending: &mut BTreeSet<u64>,
    message: &Value,
    expected: u64,
) -> Result<Option<RpcReply>, NodeRunnerError> {
    let id = message["id"]
        .as_u64()
        .ok_or_else(|| failure("Copilot RPC response has no valid identity"))?;
    if !pending.remove(&id) {
        return Err(failure("Copilot RPC response identity is unexpected"));
    }
    let reply = response_payload(message)?;
    if id == expected {
        return Ok(Some(reply));
    }
    reply.into_value("outstanding request")?;
    Ok(None)
}

fn response_payload(message: &Value) -> Result<RpcReply, NodeRunnerError> {
    match (message.get("result"), message.get("error")) {
        (Some(result), None) => Ok(RpcReply::Result(result.clone())),
        (None, Some(error)) => match (error["code"].as_i64(), error["message"].as_str()) {
            (Some(_), Some(message)) => Ok(RpcReply::Error(message.to_owned())),
            _ => Err(failure("Copilot RPC error is invalid")),
        },
        _ => Err(failure(
            "Copilot RPC response must contain exactly one result or error",
        )),
    }
}

fn append_stderr_detail(
    detail: &mut String,
    completion: &crate::execution::process::ProcessSessionOutput,
) {
    if completion.stderr_tail.is_empty() {
        return;
    }
    let prefix = if completion.stderr_tail_truncated {
        "; stderr (truncated tail): "
    } else {
        "; stderr: "
    };
    detail.push_str(prefix);
    detail.push_str(&String::from_utf8_lossy(&completion.stderr_tail));
}

#[cfg(test)]
#[path = "rpc/tests.rs"]
mod tests;
