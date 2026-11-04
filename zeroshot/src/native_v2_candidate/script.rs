//! Local deterministic steps with JSON stdin/stdout and shared process containment.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{WorkerErrorCode, WorkerOutcome};
use serde_json::json;

use super::stateless::StatelessSession;

use crate::execution::WorkspaceAccessMode;
use crate::execution::driver::WorkspaceCapability;
use crate::execution::process::{
    LocalProcessRunner, MAX_PROCESS_MESSAGE_BYTES, ProcessLaunchEvidence, ProcessSessionCommand,
    ProcessStdout,
};
use crate::native_v2_capsule::provider_process::{
    LocalHarnessEnvironment, apply_execution_environment, current_process_environment,
    safe_provider_text, send_process_input,
};
use crate::native_v2_contract::{
    NodeCompletion, NodeInvocation, NodeRuntimeBinding, parse_token_usage_delta,
};
use crate::native_v2_runner::{
    DriverControl, DriverInvocation, LiveOutput, LiveOutputStream, NodeDriver,
    NodeResponseContract, NodeRunnerError, NodeSession, ResolvedEnvironment, SessionFactory,
    bounded_log_text,
};
use crate::v2_run_ledger::{RunEvent, validate_event};

pub(crate) struct ScriptAdapter {
    workspace: PathBuf,
    environment: BTreeMap<String, String>,
}

fn script_environment(captured: &LocalHarnessEnvironment) -> BTreeMap<String, String> {
    let mut environment = [
        "PATH",
        "USERPROFILE",
        "TMPDIR",
        "TEMP",
        "TMP",
        "SystemRoot",
        "SYSTEMROOT",
        "WINDIR",
        "PATHEXT",
        "LANG",
        "LC_ALL",
    ]
    .into_iter()
    .filter_map(|name| {
        captured
            .get(name)
            .map(|value| (name.to_owned(), value.clone()))
    })
    .collect::<BTreeMap<_, _>>();
    if let Some(home) = captured.user_home() {
        environment.insert("HOME".to_owned(), home.clone());
        #[cfg(windows)]
        environment
            .entry("USERPROFILE".to_owned())
            .or_insert_with(|| home.clone());
    }
    environment
}

impl ScriptAdapter {
    pub(crate) fn new(workspace: PathBuf, captured: Option<&BTreeMap<String, String>>) -> Self {
        let captured = LocalHarnessEnvironment::new(
            captured
                .cloned()
                .unwrap_or_else(current_process_environment),
        );
        let environment = script_environment(&captured);
        Self {
            workspace,
            environment,
        }
    }

    fn specification(
        &self,
        invocation: &DriverInvocation,
    ) -> Result<ProcessSessionCommand, NodeRunnerError> {
        let NodeRuntimeBinding::Script { command, .. } = &invocation.node.binding else {
            return Err(NodeRunnerError::InvalidRole);
        };
        if !matches!(invocation.response, NodeResponseContract::Worker { .. }) {
            return Err(NodeRunnerError::InvalidRole);
        }
        let mut environment = self.environment.clone();
        environment.extend(
            invocation
                .environment
                .iter()
                .map(|(key, value)| (key.as_str().to_owned(), value.to_owned())),
        );
        apply_execution_environment(&mut environment, invocation)?;
        let argv = command.as_slice();
        Ok(ProcessSessionCommand {
            program: argv[0].clone(),
            argv: argv[1..].to_vec(),
            environment,
            workspace: WorkspaceCapability {
                current_dir: self.workspace.clone(),
                mode: WorkspaceAccessMode::ReadWrite,
            },
            deadline: None,
        })
    }

    async fn execute(
        &self,
        invocation: &DriverInvocation,
        control: &DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let specification = self.specification(invocation)?;
        let input =
            serde_json::to_vec(&invocation.node.input).map_err(|_| NodeRunnerError::Driver)?;
        if input.len() > MAX_PROCESS_MESSAGE_BYTES {
            return fail(
                control,
                WorkerErrorCode::Malformed,
                "script input exceeds 8 MiB",
            )
            .await;
        }
        let output = run_process(specification, &input, invocation, control).await?;
        publish_stderr(&output.process, &invocation.environment, control).await?;
        let bytes = match output.transfer {
            Ok(bytes) => bytes,
            Err((code, message)) => return fail(control, code, message).await,
        };
        if let Some((code, message)) = process_failure(&output.process) {
            return fail(control, code, message).await;
        }
        parse_output(&bytes, invocation, control).await
    }
}

async fn parse_output(
    bytes: &[u8],
    invocation: &DriverInvocation,
    control: &DriverControl,
) -> Result<WorkerOutcome, NodeRunnerError> {
    let value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => {
            return fail(
                control,
                WorkerErrorCode::Malformed,
                "script stdout must contain one JSON value",
            )
            .await;
        }
    };
    let outcome = match invocation.response.parse_agent_value(value) {
        Ok(outcome) => outcome,
        Err(error) => {
            return fail(
                control,
                WorkerErrorCode::Malformed,
                &redact(
                    &format!(
                        "script output violates declared schema: {}",
                        error.diagnostic()
                    ),
                    &invocation.environment,
                ),
            )
            .await;
        }
    };
    let event = RunEvent::NodeCompleted {
        completion: NodeCompletion {
            reference: invocation.node.reference.clone(),
            outcome,
        },
    };
    match (validate_event(&event), event) {
        (Ok(()), RunEvent::NodeCompleted { completion }) => Ok(completion.outcome),
        _ => {
            fail(
                control,
                WorkerErrorCode::Malformed,
                "script output exceeds the 1 MiB durable event limit",
            )
            .await
        }
    }
}

type ScriptTransfer = Result<Vec<u8>, (WorkerErrorCode, &'static str)>;

struct ScriptOutput {
    process: crate::execution::process::ProcessSessionOutput,
    transfer: ScriptTransfer,
}

async fn run_process(
    specification: ProcessSessionCommand,
    input: &[u8],
    invocation: &DriverInvocation,
    control: &DriverControl,
) -> Result<ScriptOutput, NodeRunnerError> {
    let mut process = LocalProcessRunner::new()
        .open(specification, control.cancellation())
        .await
        .map_err(|error| {
            if error.launch_evidence() == ProcessLaunchEvidence::MayHaveStarted {
                NodeRunnerError::CleanupUnconfirmed
            } else {
                NodeRunnerError::DriverDetail(redact(
                    &format!("script launch failed: {error}"),
                    &invocation.environment,
                ))
            }
        })?;
    let stdout = process.detach_stdout();
    let transfer = async {
        send_process_input(&mut process, input).await.map_err(|_| {
            (
                WorkerErrorCode::Crash,
                "script stdin did not accept complete JSON input",
            )
        })
    };
    let exchange = tokio::try_join!(transfer, read_stdout(stdout));
    let completion = if exchange.is_ok() {
        process.wait().await
    } else {
        process.release().await
    };
    let completion = completion.map_err(|_| NodeRunnerError::CleanupUnconfirmed)?;
    if !completion.cleanup.proves_tree_empty() {
        return Err(NodeRunnerError::CleanupUnconfirmed);
    }
    if control.is_cancelled() || completion.cancelled {
        return Err(NodeRunnerError::Cancelled);
    }
    Ok(ScriptOutput {
        process: completion,
        transfer: exchange.map(|((), bytes)| bytes),
    })
}

async fn publish_stderr(
    output: &crate::execution::process::ProcessSessionOutput,
    environment: &ResolvedEnvironment,
    control: &DriverControl,
) -> Result<(), NodeRunnerError> {
    if output.stderr_tail_truncated {
        // A byte-truncated tail can start inside a credential. Omit it entirely.
        return log(
            control,
            "script stderr exceeded the retained diagnostic tail (diagnostics omitted)",
        )
        .await;
    }
    let stderr = redact(&String::from_utf8_lossy(&output.stderr_tail), environment);
    if !stderr.is_empty() {
        log(control, &stderr).await?;
    }
    Ok(())
}

fn process_failure(
    output: &crate::execution::process::ProcessSessionOutput,
) -> Option<(WorkerErrorCode, &'static str)> {
    if output.timed_out {
        return Some((WorkerErrorCode::Timeout, "script execution timed out"));
    }
    if output.exit_code != Some(0) || output.post_launch_error.is_some() {
        return Some((WorkerErrorCode::Crash, "script process failed"));
    }
    None
}

async fn read_stdout(
    mut stdout: ProcessStdout,
) -> Result<Vec<u8>, (WorkerErrorCode, &'static str)> {
    let mut bytes = Vec::new();
    while let Some(chunk) = stdout.recv().await {
        let chunk = chunk.as_slice();
        if chunk.len() > MAX_PROCESS_MESSAGE_BYTES.saturating_sub(bytes.len()) {
            return Err((WorkerErrorCode::Malformed, "script output exceeds 8 MiB"));
        }
        bytes.extend_from_slice(chunk);
    }
    Ok(bytes)
}

fn redact(text: &str, environment: &ResolvedEnvironment) -> String {
    let redactions = environment
        .iter()
        .map(|(_, value)| value.to_owned())
        .collect::<Vec<_>>();
    safe_provider_text(text, &redactions)
}

async fn log(control: &DriverControl, message: &str) -> Result<(), NodeRunnerError> {
    control
        .emit_final(LiveOutput::new(
            LiveOutputStream::Error,
            bounded_log_text(message.to_owned(), 8 * 1024),
        )?)
        .await
}

async fn fail(
    control: &DriverControl,
    code: WorkerErrorCode,
    message: &str,
) -> Result<WorkerOutcome, NodeRunnerError> {
    log(control, message).await?;
    Ok(match code {
        WorkerErrorCode::Malformed => WorkerOutcome::malformed(),
        code => WorkerOutcome::declared_failure(code),
    })
}

#[async_trait]
impl SessionFactory for ScriptAdapter {
    async fn open(
        &self,
        invocation: &NodeInvocation,
        _environment: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        if !matches!(invocation.binding, NodeRuntimeBinding::Script { .. }) {
            return Err(NodeRunnerError::InvalidRole);
        }
        Ok(Arc::new(StatelessSession::default()))
    }
}

#[async_trait]
impl NodeDriver for ScriptAdapter {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        control
            .record_token_usage(parse_token_usage_delta(
                Some(&json!({"input_tokens": 0, "output_tokens": 0})),
                None,
                None,
            ))
            .await?;
        let timer = super::timing::ExecutionTiming::start()?;
        let outcome = self.execute(&invocation, &control).await;
        super::timing::publish(
            &control,
            timer.finish(
                "script_timing",
                outcome
                    .as_ref()
                    .is_ok_and(|outcome| matches!(outcome, WorkerOutcome::Verified { .. })),
            ),
        )
        .await?;
        outcome
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod environment_tests;
