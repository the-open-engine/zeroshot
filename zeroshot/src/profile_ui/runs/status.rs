//! Current runtime failure is observation metadata, never a replacement for retained history.
use std::path::PathBuf;
use std::time::Duration;

use openengine_cluster_protocol::{
    Cursor, RunId, RunStatus, RunStatusParams, RunStatusResult, TerminalResult,
};
use serde::Serialize;
use serde_json::{json, Value};

use crate::native_v2_observability::NativeV2Observability;
use crate::v2_run_ledger::{cursor_for, cursor_sequence, RunSnapshot};

const STATUS_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub(super) enum RuntimeStatusReader {
    Local(PathBuf),
    Target(NativeV2Observability),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RuntimeFailure {
    at_cursor: Cursor,
    reason: &'static str,
}

impl RuntimeFailure {
    pub(super) fn apply(&self, value: &mut Value) {
        value["phase"] = json!("finished");
        value["terminal"] = json!({"status":"failed", "reason":self.reason});
        value["runtimeFailure"] = json!(self);
    }
}

impl RuntimeStatusReader {
    pub(super) async fn failure(
        &self,
        snapshot: &RunSnapshot,
    ) -> Result<Option<RuntimeFailure>, ()> {
        if snapshot.terminal.is_some() {
            return Ok(None);
        }
        let status = tokio::time::timeout(STATUS_TIMEOUT, self.status(&snapshot.run_id))
            .await
            .map_err(|_| ())?;
        let Some(status) = status else {
            return if self.local_owner_is_live(&snapshot.run_id) {
                Ok(None)
            } else {
                Err(())
            };
        };
        if !valid_status(snapshot, &status) {
            return Err(());
        }
        Ok(confirmed_failure(snapshot, status))
    }

    fn local_owner_is_live(&self, id: &RunId) -> bool {
        let Self::Local(root) = self else {
            return false;
        };
        let paths = crate::native_v2_portable_controller::PortableControllerPaths::new(
            root.join("runs").join(id.as_str()),
        );
        crate::native_v2_portable_controller::ControllerLease::is_held(&paths.lease())
            .unwrap_or(false)
    }

    async fn status(&self, id: &RunId) -> Option<RunStatusResult> {
        match self {
            Self::Target(observability) => observability
                .status(RunStatusParams { run_id: id.clone() })
                .await
                .ok(),
            Self::Local(root) => local_status(root, id).await,
        }
    }
}

pub(super) fn confirmed_failure(
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
            terminal_result: TerminalResult::Failed { reason },
            ..
        } if reason.as_str() == "runtime_failed" => Some(RuntimeFailure {
            at_cursor: status.at_cursor,
            reason: "runtime_failed",
        }),
        _ => None,
    }
}

fn valid_status(snapshot: &RunSnapshot, status: &RunStatusResult) -> bool {
    status.run_id == snapshot.run_id
        && cursor_sequence(&status.at_cursor)
            .is_ok_and(|at| at <= i64::MAX as u64 && cursor_for(at) == status.at_cursor)
}

#[cfg(any(unix, windows))]
async fn local_status(root: &std::path::Path, id: &RunId) -> Option<RunStatusResult> {
    use crate::native_v2_portable_controller::{connect_transport, read_ready, PortableControllerPaths};
    use openengine_cluster_client::ClusterClient;

    let paths = PortableControllerPaths::new(root.join("runs").join(id.as_str()));
    if read_ready(&paths).ok()?.run_id != *id {
        return None;
    }
    // Connect only to an existing controller. CLI recovery would acquire a lease, create an
    // observer, and possibly persist runtime_lost, none of which belongs to this reader.
    let transport = connect_transport(&paths).await.ok()?;
    ClusterClient::new(transport.as_ref())
        .run_status(RunStatusParams { run_id: id.clone() })
        .await
        .ok()
}

#[cfg(not(any(unix, windows)))]
async fn local_status(_root: &std::path::Path, _id: &RunId) -> Option<RunStatusResult> {
    None
}
