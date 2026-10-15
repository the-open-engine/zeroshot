//! Trusted local delivery validation, separate from graph-owned agents and credentials.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::execution::WorkspaceAccessMode;
use crate::execution::driver::{DriverCancellation, WorkspaceCapability};
use crate::execution::platform::{FileAccess, private_file};
use crate::execution::process::{
    LocalProcessRunner, ProcessFrame, ProcessLaunchEvidence, ProcessSession, ProcessSessionCommand,
    ProcessSessionOutput, ProcessStdout,
};
use crate::native_v2_runner::NodeRunnerError;

const VALIDATION_KIND: &str = "openengine.delivery-validation/v1";
const MAX_VALIDATION_BYTES: usize = 1024 * 1024;
const MAX_REASON_BYTES: usize = 8 * 1024;

/// Controller-owned executable outside the checkout, never supplied to provider children.
#[derive(Clone)]
pub struct TrustedDeliveryValidator {
    path: PathBuf,
    workspace: PathBuf,
    active: std::sync::Arc<tokio::sync::Mutex<Option<ValidationTask>>>,
}

type ValidationTask = tokio::task::JoinHandle<Result<DeliveryValidationDecision, NodeRunnerError>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryValidationStage {
    Publish,
    Published,
    Review,
    Merge,
}

/// Public candidate facts only; private configuration belongs to the caller-owned executable.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryValidationRequest<'a> {
    pub stage: DeliveryValidationStage,
    pub repository: &'a str,
    pub target_branch: &'a str,
    pub head_branch: &'a str,
    pub head_revision: &'a str,
    pub tree: &'a str,
    pub review_base_revision: Option<&'a str>,
    pub title: &'a str,
    pub description: &'a str,
    pub review_id: Option<&'a str>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum DeliveryValidationDecision {
    Accepted,
    Repair { reason: String },
    Pending,
}

#[derive(Serialize)]
struct ValidationMessage<'a, 'request> {
    kind: &'static str,
    #[serde(flatten)]
    request: &'a DeliveryValidationRequest<'request>,
}

#[derive(Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case", deny_unknown_fields)]
enum ValidationResponse {
    Accepted {},
    Repair { reason: String },
    Refused { reason: String },
    Pending {},
}

impl TrustedDeliveryValidator {
    /// Resolve directory aliases and require a private executable outside the workspace.
    pub fn new(path: PathBuf, workspace: &Path) -> io::Result<Self> {
        if !path.is_absolute() || path.to_str().is_none() {
            return Err(invalid_path());
        }
        let file = private_file(&path, FileAccess::Read)?;
        let path = path.canonicalize()?;
        let workspace = workspace.canonicalize()?;
        if path.starts_with(&workspace) {
            return Err(invalid_path());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            if file.metadata()?.permissions().mode() & 0o100 == 0 {
                return Err(invalid_path());
            }
        }
        #[cfg(not(unix))]
        let _ = file;
        Ok(Self {
            path,
            workspace,
            active: std::sync::Arc::default(),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// No turn deadline is imposed. Cancellation and every exchange exit await containment cleanup.
    pub async fn validate(
        &self,
        request: DeliveryValidationRequest<'_>,
        cancellation: DriverCancellation,
    ) -> Result<DeliveryValidationDecision, NodeRunnerError> {
        let frame = encode_request(&request)?;
        let executable = self.clone();
        let mut active = self.active.lock().await;
        if active.is_some() {
            return Err(failure("previous execution has not settled"));
        }
        // The adapter can drop its operation future on cancellation. Retain the owned helper
        // task so its process-tree cleanup is awaited before delivery releases the workspace.
        *active = Some(tokio::spawn(async move {
            executable.validate_frame(frame, cancellation).await
        }));
        let outcome = match active.as_mut() {
            Some(task) => task.await,
            None => return Err(NodeRunnerError::CleanupUnconfirmed),
        };
        *active = None;
        outcome.map_err(|_| NodeRunnerError::CleanupUnconfirmed)?
    }

    pub(crate) async fn drain(&self) -> Result<(), NodeRunnerError> {
        let mut active = self.active.lock().await;
        if let Some(task) = active.take() {
            match task.await {
                Err(_) | Ok(Err(NodeRunnerError::CleanupUnconfirmed)) => {
                    return Err(NodeRunnerError::CleanupUnconfirmed);
                }
                Ok(_) => {}
            }
        }
        Ok(())
    }

    async fn validate_frame(
        &self,
        frame: ProcessFrame,
        cancellation: DriverCancellation,
    ) -> Result<DeliveryValidationDecision, NodeRunnerError> {
        if cancellation.is_cancelled() {
            return Err(NodeRunnerError::Cancelled);
        }
        let executable = Self::new(self.path.clone(), &self.workspace)
            .map_err(|_| failure("executable is unavailable or no longer private"))?;
        let command = executable.command()?;
        let mut process = LocalProcessRunner::new()
            .open(command, cancellation)
            .await
            .map_err(launch_failure)?;
        let mut stdout = process.detach_stdout();
        let exchanged = tokio::try_join!(
            send_request(&mut process, frame),
            receive_response(&mut stdout),
        );
        drop(stdout); // Input/size failures must unblock the bounded stdout pump before release.
        let completion = finish_process(&mut process, exchanged.is_err()).await?;
        if completion.cancelled {
            return Err(NodeRunnerError::Cancelled);
        }
        let (_, bytes) = exchanged?;
        validate_completion(&completion)?;
        parse_response(&bytes)
    }

    fn command(&self) -> Result<ProcessSessionCommand, NodeRunnerError> {
        let program = self
            .path
            .to_str()
            .ok_or_else(|| failure("invalid executable path"))?;
        let mut environment = BTreeMap::from([("LANG".to_owned(), "C".to_owned())]);
        crate::execution::platform::process_environment(&mut environment);
        Ok(ProcessSessionCommand {
            program: program.to_owned(),
            argv: Vec::new(),
            environment,
            workspace: WorkspaceCapability {
                current_dir: self.workspace.clone(),
                mode: WorkspaceAccessMode::ReadWrite,
            },
            deadline: None,
        })
    }
}

impl fmt::Debug for TrustedDeliveryValidator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TrustedDeliveryValidator([PRIVATE])")
    }
}

fn encode_request(
    request: &DeliveryValidationRequest<'_>,
) -> Result<ProcessFrame, NodeRunnerError> {
    let bytes = serde_json::to_vec(&ValidationMessage {
        kind: VALIDATION_KIND,
        request,
    })
    .map_err(|_| failure("request could not be encoded"))?;
    if bytes.len() > MAX_VALIDATION_BYTES {
        return Err(failure("request exceeds the protocol byte bound"));
    }
    ProcessFrame::new(bytes).map_err(|_| failure("request exceeds the process message bound"))
}

async fn send_request(
    process: &mut ProcessSession,
    frame: ProcessFrame,
) -> Result<(), NodeRunnerError> {
    process
        .send(frame)
        .await
        .map_err(|_| failure("request could not be delivered"))?;
    process
        .close_stdin()
        .await
        .map_err(|_| failure("request stream could not be closed"))
}

async fn receive_response(stdout: &mut ProcessStdout) -> Result<Vec<u8>, NodeRunnerError> {
    let mut response = Vec::new();
    while let Some(chunk) = stdout.recv().await {
        if response.len().saturating_add(chunk.as_slice().len()) > MAX_VALIDATION_BYTES {
            return Err(failure("response exceeds the protocol byte bound"));
        }
        response.extend_from_slice(chunk.as_slice());
    }
    Ok(response)
}

async fn finish_process(
    process: &mut ProcessSession,
    release: bool,
) -> Result<ProcessSessionOutput, NodeRunnerError> {
    let output = if release {
        process.release().await
    } else {
        process.wait().await
    }
    .map_err(|_| NodeRunnerError::CleanupUnconfirmed)?;
    if !output.cleanup.proves_tree_empty() {
        return Err(NodeRunnerError::CleanupUnconfirmed);
    }
    Ok(output)
}

fn validate_completion(output: &ProcessSessionOutput) -> Result<(), NodeRunnerError> {
    if output.exit_code != Some(0) || output.timed_out || output.post_launch_error.is_some() {
        return Err(failure("executable did not complete successfully"));
    }
    Ok(())
}

fn parse_response(bytes: &[u8]) -> Result<DeliveryValidationDecision, NodeRunnerError> {
    let response: ValidationResponse =
        serde_json::from_slice(bytes).map_err(|_| failure("response is malformed"))?;
    match response {
        ValidationResponse::Accepted {} => Ok(DeliveryValidationDecision::Accepted),
        ValidationResponse::Pending {} => Ok(DeliveryValidationDecision::Pending),
        ValidationResponse::Repair { reason } => {
            validate_reason(&reason)?;
            Ok(DeliveryValidationDecision::Repair { reason })
        }
        ValidationResponse::Refused { reason } => {
            validate_reason(&reason)?;
            Err(failure(&format!("refused: {reason}")))
        }
    }
}

fn validate_reason(reason: &str) -> Result<(), NodeRunnerError> {
    if reason.trim().is_empty()
        || reason.len() > MAX_REASON_BYTES
        || reason
            .chars()
            .any(|value| value.is_control() && value != '\n' && value != '\t')
    {
        return Err(failure("response reason is malformed"));
    }
    Ok(())
}

fn launch_failure(error: crate::execution::process::ProcessRunnerError) -> NodeRunnerError {
    match error.launch_evidence() {
        ProcessLaunchEvidence::DefinitelyNotStarted => failure("executable could not be started"),
        ProcessLaunchEvidence::MayHaveStarted => NodeRunnerError::CleanupUnconfirmed,
    }
}

fn failure(reason: &str) -> NodeRunnerError {
    NodeRunnerError::DriverDetail(format!("Trusted delivery validation {reason}"))
}

fn invalid_path() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "trusted delivery validator must be a private absolute executable outside the workspace",
    )
}

#[cfg(test)]
#[path = "validation/tests.rs"]
mod tests;
