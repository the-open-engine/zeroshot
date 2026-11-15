//! Current runtime failure is observation metadata, never a replacement for retained history.
#[cfg(feature = "ui")]
use std::path::PathBuf;
use std::time::Duration;

use openengine_cluster_protocol::{
    Cursor, FailureContext, RunId, RunStatus, RunStatusParams, RunStatusResult, TerminalResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::native_v2_observability::NativeV2Observability;
use crate::v2_run_ledger::{cursor_for, cursor_sequence, RunSnapshot};

const STATUS_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub(crate) enum RuntimeStatusReader {
    #[cfg(feature = "ui")]
    Local(PathBuf),
    Target(NativeV2Observability),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeFailure {
    pub at_cursor: Cursor,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Box<FailureContext>>,
}

impl RuntimeFailure {
    pub(crate) fn apply(&self, value: &mut Value) {
        value["phase"] = json!("finished");
        value["terminal"] = json!({"status":"failed", "reason":self.reason});
        if let Some(context) = &self.context {
            value["terminal"]["context"] = json!(context);
        }
        value["runtimeFailure"] = json!(self);
    }
}

pub(crate) async fn runtime_observation(
    reader: Option<&RuntimeStatusReader>,
    snapshot: &RunSnapshot,
) -> Result<Option<RuntimeFailure>, ()> {
    match reader {
        Some(reader) => reader.failure(snapshot).await,
        None => Ok(None),
    }
}

impl RuntimeStatusReader {
    pub(crate) async fn failure(
        &self,
        snapshot: &RunSnapshot,
    ) -> Result<Option<RuntimeFailure>, ()> {
        if snapshot.terminal.is_some() || snapshot.suspension.is_some() {
            return Ok(None);
        }
        #[cfg(feature = "ui")]
        if let Self::Local(root) = self {
            use crate::native_v2_portable_controller::availability::{
                check_controller, ControllerAvailability,
            };
            let paths = crate::native_v2_portable_controller::PortableControllerPaths::new(
                root.join("runs").join(snapshot.run_id.as_str()),
            );
            return match check_controller(&paths, &snapshot.run_id).await {
                ControllerAvailability::Responding(_, status)
                    if valid_status(snapshot, &status) =>
                {
                    Ok(confirmed_failure(snapshot, *status))
                }
                ControllerAvailability::Embedded(_) => Ok(None),
                _ => Err(()),
            };
        }
        let status = tokio::time::timeout(STATUS_TIMEOUT, self.status(&snapshot.run_id))
            .await
            .map_err(|_| ())?
            .ok_or(())?;
        if !valid_status(snapshot, &status) {
            return Err(());
        }
        Ok(confirmed_failure(snapshot, status))
    }

    pub(crate) async fn status(&self, id: &RunId) -> Option<RunStatusResult> {
        match self {
            Self::Target(observability) => observability
                .status(RunStatusParams { run_id: id.clone() })
                .await
                .ok(),
            #[cfg(feature = "ui")]
            Self::Local(root) => {
                use crate::native_v2_cli::NativeV2CliBackend;
                let backend = crate::native_v2_cli::local::LocalCliBackend::new(
                    root.clone(),
                    PathBuf::new(),
                    PathBuf::new(),
                    PathBuf::new(),
                );
                let status = backend
                    .run_status(None, RunStatusParams { run_id: id.clone() })
                    .await
                    .ok()?;
                let crate::native_v2_cli::CliRunStatus::Target(status_body) = status.status else {
                    return None;
                };
                Some(RunStatusResult {
                    run_id: status.run_id,
                    title: status.title,
                    source: status.source,
                    size: status.size,
                    at_cursor: status.at_cursor,
                    status: status_body,
                    controller: status.controller,
                    workspace_recovery: status.workspace_recovery,
                })
            }
        }
    }
}

pub(crate) fn confirmed_failure(
    snapshot: &RunSnapshot,
    status: RunStatusResult,
) -> Option<RuntimeFailure> {
    if !valid_status(snapshot, &status) || snapshot.terminal.is_some() {
        return None;
    }
    let at = cursor_sequence(&status.at_cursor).ok()?;
    let head = cursor_sequence(&snapshot.cursor).ok()?;
    if at > head {
        return None;
    }
    match status.status {
        RunStatus::Finished {
            terminal_result: TerminalResult::Failed { reason, context },
            ..
        } if reason.as_str() == "runtime_failed" => Some(RuntimeFailure {
            at_cursor: status.at_cursor,
            reason: "runtime_failed".into(),
            context,
        }),
        _ => None,
    }
}

fn valid_status(snapshot: &RunSnapshot, status: &RunStatusResult) -> bool {
    status.run_id == snapshot.run_id
        && cursor_sequence(&status.at_cursor)
            .is_ok_and(|at| at <= i64::MAX as u64 && cursor_for(at) == status.at_cursor)
}
