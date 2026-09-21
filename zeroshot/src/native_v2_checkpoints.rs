//! Engine-defined recovery boundaries with host-owned workspace storage.
//!
//! Snapshot bytes and execution prerequisites stay private. Public clients select an immutable
//! checkpoint ID; they never supply paths, execution histories, or provider session state.

#[path = "native_v2_checkpoints/catalog.rs"]
mod catalog;
#[path = "native_v2_checkpoints/filesystem.rs"]
pub(crate) mod filesystem;
#[cfg(test)]
#[path = "native_v2_checkpoints/tests.rs"]
mod tests;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{
    CheckpointId, NodeName, RunCheckpoint, RunCheckpointsParams, RunCheckpointsResult,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::full_v1_reducer::{DurableExecution, DurableExecutionState, ExecutionBoundary};
use filesystem::SnapshotId;

#[derive(Debug, Error)]
#[error("workspace checkpoint is unavailable")]
pub struct CheckpointError(#[from] pub std::io::Error);

/// The supervisor calls this port only at an engine-defined boundary with no live executions.
#[async_trait]
pub trait RunCheckpointStore: Send + Sync {
    async fn enter(
        &self,
        boundary: &ExecutionBoundary,
        history: &[DurableExecution],
    ) -> Result<(), CheckpointError>;

    async fn finish(&self, history: &[DurableExecution]) -> Result<(), CheckpointError>;
}

/// Private bootstrap selection. The controller restores it after acquiring workspace ownership.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CheckpointRestore {
    pub directory: PathBuf,
    pub checkpoint_id: CheckpointId,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RecoveryPoint {
    descriptor: RunCheckpoint,
    snapshot: SnapshotId,
    history: Vec<DurableExecution>,
}

#[derive(Default)]
struct CaptureState {
    boundary: Option<ExecutionBoundary>,
    snapshot: Option<SnapshotId>,
    last_writer: Option<u64>,
}

/// Local and direct-target storage. Cloud can implement the same port with an object-store backend.
pub struct FilesystemCheckpointStore {
    directory: PathBuf,
    workspace: PathBuf,
    writers: BTreeSet<NodeName>,
    state: Arc<Mutex<CaptureState>>,
}

impl FilesystemCheckpointStore {
    #[must_use]
    pub fn new(directory: PathBuf, workspace: PathBuf, writers: BTreeSet<NodeName>) -> Self {
        Self {
            directory,
            workspace,
            writers,
            state: Arc::default(),
        }
    }

    fn last_writer(&self, history: &[DurableExecution]) -> Result<Option<u64>, CheckpointError> {
        if history
            .iter()
            .any(|execution| matches!(execution.state, DurableExecutionState::Active))
        {
            return Err(catalog::invalid("cannot snapshot active executions"));
        }
        Ok(history
            .iter()
            .filter(|execution| self.writers.contains(&execution.occurrence.node))
            .map(|execution| execution.execution.get())
            .max())
    }

    async fn capture_if_changed(
        &self,
        state: &mut CaptureState,
        last_writer: Option<u64>,
    ) -> Result<SnapshotId, CheckpointError> {
        if state.last_writer == last_writer {
            if let Some(snapshot) = &state.snapshot {
                return Ok(snapshot.clone());
            }
        }
        let directory = self.directory.clone();
        let workspace = self.workspace.clone();
        let snapshot = tokio::task::spawn_blocking(move || {
            crate::execution::platform::private_directory(&directory)?;
            filesystem::capture(&workspace, &directory.join("snapshots"), &())
        })
        .await
        .map_err(|_| catalog::invalid("checkpoint capture task failed"))??;
        state.snapshot = Some(snapshot.clone());
        state.last_writer = last_writer;
        Ok(snapshot)
    }
}

#[async_trait]
impl RunCheckpointStore for FilesystemCheckpointStore {
    async fn enter(
        &self,
        boundary: &ExecutionBoundary,
        history: &[DurableExecution],
    ) -> Result<(), CheckpointError> {
        let mut state = self.state.lock().await;
        if state.boundary.as_ref() == Some(boundary) {
            return Ok(());
        }
        let last_writer = self.last_writer(history)?;
        let snapshot = self.capture_if_changed(&mut state, last_writer).await?;
        let directory = self.directory.clone();
        let boundary = boundary.clone();
        let history = history.to_vec();
        let recorded_boundary = boundary.clone();
        tokio::task::spawn_blocking(move || {
            catalog::publish(&directory, boundary, snapshot, history)
        })
        .await
        .map_err(|_| catalog::invalid("checkpoint publication task failed"))??;
        state.boundary = Some(recorded_boundary);
        Ok(())
    }

    async fn finish(&self, history: &[DurableExecution]) -> Result<(), CheckpointError> {
        let last_writer = self.last_writer(history)?;
        let mut state = self.state.lock().await;
        let snapshot = self.capture_if_changed(&mut state, last_writer).await?;
        catalog::write_atomic(&self.directory.join("latest.json"), &snapshot)?;
        Ok(())
    }
}

#[must_use]
pub fn selected_checkpoint(
    from: Option<&openengine_cluster_protocol::RunResumeFrom>,
) -> Option<&CheckpointId> {
    match from {
        Some(openengine_cluster_protocol::RunResumeFrom::Checkpoint { checkpoint_id }) => {
            Some(checkpoint_id)
        }
        _ => None,
    }
}

pub fn validate_selection(selection: &CheckpointRestore) -> Result<(), CheckpointError> {
    catalog::point(&selection.directory, &selection.checkpoint_id).map(|_| ())
}

pub fn list(
    directory: &Path,
    params: RunCheckpointsParams,
) -> Result<RunCheckpointsResult, CheckpointError> {
    catalog::list(directory, params)
}

pub fn restore(
    selection: &CheckpointRestore,
    workspace: &Path,
) -> Result<Vec<DurableExecution>, CheckpointError> {
    let point = catalog::point(&selection.directory, &selection.checkpoint_id)?;
    filesystem::restore(
        &selection.directory.join("snapshots"),
        &point.snapshot,
        workspace,
    )?;
    Ok(point.history)
}
