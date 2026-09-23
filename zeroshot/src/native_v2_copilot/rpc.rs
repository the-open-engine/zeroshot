use std::collections::BTreeSet;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use openengine_cluster_protocol::WorkerOutcome;
use crate::execution::process::{
    ProcessFrame, ProcessRunnerError, ProcessStdout, MAX_PROCESS_MESSAGE_BYTES,
};
use crate::native_v2_capsule::provider_process::{
    ProviderProcess, ProviderExecutionFiles, redaction_values, safe_provider_text,
    provider_failure_diagnostic,
};
use crate::native_v2_runner::{
    AgentResponseState, DriverControl, DriverInvocation, NodeRunnerError, ProviderSchemaDialect,
    VerifierWorkspace, render_agent_prompt_for, resolve_agent_response_with_dialect,
};
use super::{auth, command, events, framing::Frames, provider, session::CopilotSession};

pub(super) fn failure(message: impl Into<String>) -> NodeRunnerError {
    NodeRunnerError::DriverDetail(message.into())
}

pub(super) fn process_error(error: ProcessRunnerError) -> NodeRunnerError {
    failure(format!("Copilot process failed: {error}"))
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
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        process
            .send(ProcessFrame::new(header.into_bytes()).map_err(process_error)?)
            .await
            .map_err(process_error)?;
        for chunk in body.chunks(MAX_PROCESS_MESSAGE_BYTES) {
            process
                .send(ProcessFrame::new(chunk.to_vec()).map_err(process_error)?)
                .await
                .map_err(process_error)?;
        }
    }
    Ok(())
}

pub(super) struct CopilotRpc<'a> {
    stdout: ProcessStdout,
    frames: Frames,
    pub(super) invocation: &'a DriverInvocation,
    pub(super) control: &'a DriverControl,
    verifier_workspace: VerifierWorkspace,
    sender: Option<mpsc::Sender<Value>>,
    next_id: u64,
    pending: BTreeSet<u64>,
    pub(super) session_id: String,
    pub(super) response: Option<String>,
    pub(super) provider_error: Option<String>,
    pub(super) redactions: Vec<String>,
    authentication: auth::CopilotAuthentication<'a>,
    provider: Option<&'a provider::LocalProvider>,
}

pub(super) struct CopilotRpcNative<'a> {
    pub verifier_workspace: VerifierWorkspace,
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
            verifier_workspace: native.verifier_workspace,
            sender: None,
            next_id: 0,
            pending: BTreeSet::new(),
            session_id: String::new(),
            response: None,
            provider_error: None,
            redactions,
            authentication: native.authentication,
            provider: native.provider,
        }
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
        let params = command::session_parameters(
            self.invocation,
            files,
            &self.session_id,
            command::SessionProvider {
                authentication: &self.authentication,
                local: self.provider,
            },
        )?;
        let method = if resume.is_some() {
            "session.resume"
        } else {
            "session.create"
        };
        let created = self.request(method, params).await?;
        if created["sessionId"].as_str() != Some(self.session_id.as_str()) {
            return Err(failure("Copilot returned a different session identity"));
        }
        *session.id.lock().await = Some(self.session_id.clone());
        let outcome = self.run_response().await?;
        let detached = self
            .request("session.detach", json!({"sessionId":self.session_id}))
            .await?;
        if detached["success"].as_bool() != Some(true) {
            return Err(failure("Copilot session could not be detached"));
        }
        Ok(outcome)
    }

    async fn connect(&mut self) -> Result<(), NodeRunnerError> {
        let connected = self
            .request(
                "connect",
                json!({"clientInfo":{"extensionName":"zeroshot"}}),
            )
            .await?;
        if connected["protocolVersion"].as_u64() != Some(3) {
            return Err(failure(
                "Copilot RPC protocol version is incompatible; install CLI 1.0.86",
            ));
        }
        Ok(())
    }

    async fn run_response(&mut self) -> Result<WorkerOutcome, NodeRunnerError> {
        let prompt = render_agent_prompt_for(
            self.invocation.agent_instructions()?,
            &self.invocation.node.input,
            &self.invocation.response,
            self.verifier_workspace,
        )?;
        let mut response = AgentResponseState::new(prompt);
        loop {
            self.response = None;
            self.provider_error = None;
            self.request("session.send", json!({
                "sessionId":self.session_id, "prompt":response.prompt(), "wait":true,
                "responseFormat":{"type":"json_schema", "jsonSchema":{
                    "name":"zeroshot_response", "strict":true,
                    "schema":self.invocation.response.provider_schema(ProviderSchemaDialect::OpenAiStrict),
                }},
            })).await?;
            if let Some(error) = self.provider_error.take() {
                return Err(failure(error));
            }
            let text = self
                .response
                .take()
                .ok_or_else(|| failure("Copilot completed without an assistant response"))?;
            let value = resolve_agent_response_with_dialect(
                &self.invocation.response,
                &text,
                ProviderSchemaDialect::OpenAiStrict,
            )?;
            if let Some(outcome) = response.accept("Copilot", self.control, value).await? {
                return Ok(outcome);
            }
        }
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
        if self.pending.len() >= 128 {
            return Err(failure("Copilot RPC pending request limit exceeded"));
        }
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| failure("Copilot RPC identifier overflow"))?;
        let id = self.next_id;
        self.queue(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))?;
        self.pending.insert(id);
        Ok(id)
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, NodeRunnerError> {
        let expected = self.send_request(method, params)?;
        while let Some(message) = self.next().await? {
            if message.get("method").is_some() {
                self.dispatch(&message).await?;
            } else if let Some(result) = self.response_message(&message, expected)? {
                return Ok(result);
            }
        }
        Err(failure(format!(
            "Copilot exited before replying to {method}"
        )))
    }

    fn response_message(
        &mut self,
        message: &Value,
        expected: u64,
    ) -> Result<Option<Value>, NodeRunnerError> {
        let id = message["id"]
            .as_u64()
            .ok_or_else(|| failure("Copilot RPC response has no valid identity"))?;
        if !self.pending.remove(&id) {
            return Err(failure("Copilot RPC response identity is unexpected"));
        }
        if let Some(error) = message.get("error") {
            return Err(failure(format!(
                "Copilot RPC failed: {}",
                error["message"].as_str().unwrap_or("unknown error")
            )));
        }
        let result = message
            .get("result")
            .ok_or_else(|| failure("Copilot RPC response has no result"))?;
        Ok((id == expected).then(|| result.clone()))
    }

    async fn dispatch(&mut self, message: &Value) -> Result<(), NodeRunnerError> {
        match message["method"].as_str() {
            Some("session.event") => events::receive(self, &message["params"]).await,
            Some("gitHubToken.getToken") if message.get("id").is_some() => {
                self.acquire_token(message).await
            }
            _ if message.get("id").is_some() => self
                .queue(json!({"jsonrpc":"2.0", "id":message["id"],
                "error":{"code":-32601,"message":"Method not supported by Zeroshot"}})),
            _ => Ok(()),
        }
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
        let mut detail = match error {
            NodeRunnerError::Driver => "execution failed".to_owned(),
            NodeRunnerError::DriverDetail(detail) => detail,
            error => return error,
        };
        if !completion.stderr_tail.is_empty() {
            let prefix = if completion.stderr_tail_truncated {
                "; stderr (truncated tail): "
            } else {
                "; stderr: "
            };
            detail.push_str(prefix);
            detail.push_str(&String::from_utf8_lossy(&completion.stderr_tail));
        }
        failure(provider_failure_diagnostic(
            "Copilot",
            Some(&detail),
            None,
            &self.redactions,
        ))
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
