//! Engine-defined recovery boundaries with host-owned workspace storage.
//!
//! Snapshot bytes and execution prerequisites stay private. Public clients select an immutable
//! checkpoint ID; they never supply paths, execution histories, or provider session state.

#[path = "native_v2_checkpoints/catalog.rs"]
mod catalog;
#[path = "native_v2_checkpoints/filesystem.rs"]
pub mod filesystem;
#[path = "native_v2_checkpoints/restic.rs"]
pub(crate) mod restic;
#[cfg(test)]
#[path = "native_v2_checkpoints/tests.rs"]
mod tests;

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufReader, Write};
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

    /// Best-effort post-terminal garbage collection. A failure must not change run truth.
    async fn discard(&self) -> Result<(), CheckpointError> {
        Ok(())
    }
}

/// Private bootstrap selection. The controller restores it after acquiring workspace ownership.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CheckpointRestore {
    pub directory: PathBuf,
    pub selection: CheckpointRestoreSelection,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", tag = "kind")]
pub enum CheckpointRestoreSelection {
    Latest,
    Checkpoint { checkpoint_id: CheckpointId },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RecoveryPoint {
    version: u32,
    descriptor: RunCheckpoint,
    snapshot: SnapshotId,
    seed_format: u32,
}

const STORAGE_FORMAT: u32 = 1;
const RECOVERY_POINT_FORMAT: u32 = 1;
const CHECKPOINT_SEED_FORMAT: u32 = 1;

/// Versioned private reducer state used only by selected checkpoint continuation.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CheckpointSeed {
    format: u32,
    executions: Vec<DurableExecution>,
}

impl CheckpointSeed {
    #[must_use]
    pub fn new(executions: Vec<DurableExecution>) -> Self {
        Self {
            format: CHECKPOINT_SEED_FORMAT,
            executions,
        }
    }

    pub fn read(path: &Path) -> Result<Self, CheckpointError> {
        let seed: Self =
            serde_json::from_reader(BufReader::new(File::open(path)?)).map_err(|_| {
                catalog::invalid(
                    "checkpoint seed is incompatible; restart from the latest workspace",
                )
            })?;
        if seed.format != CHECKPOINT_SEED_FORMAT {
            return Err(catalog::invalid(
                "checkpoint seed is incompatible; restart from the latest workspace",
            ));
        }
        Ok(seed)
    }

    pub fn write_atomic(&self, path: &Path) -> Result<(), CheckpointError> {
        let parent = path
            .parent()
            .ok_or_else(|| catalog::invalid("checkpoint seed path has no parent"))?;
        crate::execution::platform::private_directory(parent)?;
        let temporary = parent.join(format!(".seed-{}.tmp", uuid::Uuid::now_v7()));
        let mut file = crate::execution::platform::private_file(
            &temporary,
            crate::execution::platform::FileAccess::CreateNew,
        )?;
        let result = (|| {
            serde_json::to_writer(&mut file, self)
                .map_err(|_| catalog::invalid("checkpoint seed cannot be encoded"))?;
            file.flush()?;
            file.sync_all()?;
            drop(file);
            crate::execution::platform::commit_file(&temporary, path, parent)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    #[must_use]
    pub fn into_executions(self) -> Vec<DurableExecution> {
        self.executions
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StorageConfiguration {
    format: u32,
    repository: PathBuf,
    program: restic::ResticProgram,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredSnapshot {
    format: u32,
    restic_snapshot: String,
}

#[derive(Default)]
struct CaptureState {
    boundary: Option<ExecutionBoundary>,
    snapshot: Option<SnapshotId>,
    restic_snapshot: Option<restic::SnapshotId>,
    last_writer: Option<u64>,
    initialized: bool,
}

/// Deduplicated local and direct-target storage backed by one Restic recovery-lineage repository.
pub struct ResticCheckpointStore {
    directory: PathBuf,
    repository: PathBuf,
    program: restic::ResticProgram,
    workspace: PathBuf,
    writers: BTreeSet<NodeName>,
    state: Arc<Mutex<CaptureState>>,
}

#[cfg(test)]
pub(crate) struct ResticCheckpointStoreTestConfig {
    pub(crate) directory: PathBuf,
    pub(crate) repository: PathBuf,
    pub(crate) workspace: PathBuf,
    pub(crate) writers: BTreeSet<NodeName>,
    pub(crate) program: restic::ResticProgram,
}

impl ResticCheckpointStore {
    pub fn new(
        directory: PathBuf,
        repository: PathBuf,
        workspace: PathBuf,
        writers: BTreeSet<NodeName>,
    ) -> Result<Self, CheckpointError> {
        let program = restic::ResticProgram::discover();
        #[cfg(test)]
        let program = program.or_else(|_| {
            crate::execution::platform::private_directory(&repository)?;
            restic::ResticProgram::fake(&repository)
        });
        Ok(Self {
            directory,
            repository,
            program: program?,
            workspace,
            writers,
            state: Arc::default(),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_program(config: ResticCheckpointStoreTestConfig) -> Self {
        Self {
            directory: config.directory,
            repository: config.repository,
            program: config.program,
            workspace: config.workspace,
            writers: config.writers,
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
        if !state.initialized {
            self.prepare_storage().await?;
            state.initialized = true;
        }
        let directory = self.directory.clone();
        let workspace = self.workspace.clone();
        let (stage, snapshot) = tokio::task::spawn_blocking(move || {
            let stages = directory.join("staging");
            crate::execution::platform::private_directory(&stages)?;
            let stage = tempfile::Builder::new()
                .prefix(".capture-")
                .tempdir_in(stages)?;
            let snapshot = filesystem::capture(&workspace, &stage.path().join("snapshots"), &())?;
            Ok::<_, std::io::Error>((stage, snapshot))
        })
        .await
        .map_err(|_| catalog::invalid("checkpoint capture task failed"))??;
        let source = stage.path().join("snapshots").join(snapshot.as_str());
        let restic_snapshot = self
            .restic()
            .backup(&source, state.restic_snapshot.as_ref())
            .await?;
        let record = StoredSnapshot {
            format: STORAGE_FORMAT,
            restic_snapshot: restic_snapshot.as_str().to_owned(),
        };
        catalog::write_atomic(&snapshot_path(&self.directory, &snapshot), &record)?;
        state.snapshot = Some(snapshot.clone());
        state.restic_snapshot = Some(restic_snapshot);
        state.last_writer = last_writer;
        Ok(snapshot)
    }

    async fn prepare_storage(&self) -> Result<(), CheckpointError> {
        crate::execution::platform::private_directory(&self.directory)?;
        let expected = StorageConfiguration {
            format: STORAGE_FORMAT,
            repository: std::path::absolute(&self.repository)?,
            program: self.program.clone(),
        };
        publish_storage_configuration(&self.directory, &expected)?;
        self.restic().initialize().await?;
        Ok(())
    }

    fn restic(&self) -> restic::Repository {
        restic::Repository::new(self.program.clone(), self.repository.clone())
    }
}

fn publish_storage_configuration(
    directory: &Path,
    expected: &StorageConfiguration,
) -> Result<(), CheckpointError> {
    let path = directory.join("storage.json");
    match catalog::read::<StorageConfiguration>(&path) {
        Ok(existing) if &existing == expected => Ok(()),
        Ok(_) => Err(catalog::invalid("checkpoint storage configuration changed")),
        Err(CheckpointError(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            catalog::write_atomic(&path, expected)
        }
        Err(error) => Err(error),
    }
}

#[async_trait]
impl RunCheckpointStore for ResticCheckpointStore {
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

    async fn discard(&self) -> Result<(), CheckpointError> {
        discard_lineage(&self.directory, &self.repository)
    }
}

pub fn discard_lineage(directory: &Path, repository: &Path) -> Result<(), CheckpointError> {
    remove_lineage_catalogs(directory, repository)?;
    remove_if_present(repository)
}

#[must_use]
pub fn restore_selection(
    from: Option<&openengine_cluster_protocol::RunResumeFrom>,
) -> CheckpointRestoreSelection {
    match from {
        Some(openengine_cluster_protocol::RunResumeFrom::Checkpoint { checkpoint_id }) => {
            CheckpointRestoreSelection::Checkpoint {
                checkpoint_id: checkpoint_id.clone(),
            }
        }
        _ => CheckpointRestoreSelection::Latest,
    }
}

pub fn validate_selection(selection: &CheckpointRestore) -> Result<(), CheckpointError> {
    let snapshot = match &selection.selection {
        CheckpointRestoreSelection::Latest => catalog::latest(&selection.directory)?,
        CheckpointRestoreSelection::Checkpoint { checkpoint_id } => {
            catalog::point(&selection.directory, checkpoint_id)?.snapshot
        }
    };
    stored_snapshot(&selection.directory, &snapshot).map(|_| ())
}

pub fn list(
    directory: &Path,
    params: RunCheckpointsParams,
) -> Result<RunCheckpointsResult, CheckpointError> {
    catalog::list(directory, params)
}

pub async fn restore(
    selection: &CheckpointRestore,
    workspace: &Path,
) -> Result<Vec<DurableExecution>, CheckpointError> {
    match &selection.selection {
        CheckpointRestoreSelection::Latest => {
            let snapshot = catalog::latest(&selection.directory)?;
            restore_snapshot(&selection.directory, &snapshot, workspace).await?;
            Ok(Vec::new())
        }
        CheckpointRestoreSelection::Checkpoint { checkpoint_id } => {
            let point = catalog::point(&selection.directory, checkpoint_id)?;
            restore_snapshot(&selection.directory, &point.snapshot, workspace).await?;
            Ok(
                CheckpointSeed::read(&catalog::seed_path(&selection.directory, checkpoint_id))?
                    .into_executions(),
            )
        }
    }
}

async fn restore_snapshot(
    directory: &Path,
    point: &SnapshotId,
    workspace: &Path,
) -> Result<(), CheckpointError> {
    let (repository, snapshot) = stored_snapshot(directory, point)?;
    repository.initialize().await?;
    let restore_root = workspace
        .parent()
        .ok_or_else(|| catalog::invalid("workspace has no parent directory"))?;
    let stage = tempfile::Builder::new()
        .prefix(".zeroshot-restore-")
        .tempdir_in(restore_root)?;
    let snapshots = stage.path().join("snapshots");
    crate::execution::platform::private_directory(&snapshots)?;
    let restored = snapshots.join(point.as_str());
    crate::execution::platform::private_directory(&restored)?;
    repository.restore(&snapshot, &restored).await?;
    filesystem::restore_staged(&restored, workspace)?;
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<(), CheckpointError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn remove_lineage_catalogs(directory: &Path, repository: &Path) -> Result<(), CheckpointError> {
    let expected = std::path::absolute(repository)?;
    let mut candidates = vec![directory.to_owned()];

    if let Some(parent) = directory.parent()
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        candidates.extend(entries.filter_map(Result::ok).filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir() && !kind.is_symlink())
                .map(|_| entry.path())
        }));
    }
    if let Some(root) = repository.parent().and_then(Path::parent)
        && let Ok(entries) = std::fs::read_dir(root.join("runs"))
    {
        candidates.extend(entries.filter_map(Result::ok).filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir() && !kind.is_symlink())
                .map(|_| entry.path().join("checkpoints"))
        }));
    }

    candidates.sort();
    candidates.dedup();
    for candidate in candidates {
        let matches = candidate == directory
            || catalog::read::<StorageConfiguration>(&candidate.join("storage.json"))
                .ok()
                .and_then(|configuration| std::path::absolute(configuration.repository).ok())
                .is_some_and(|configured| configured == expected);
        if matches {
            remove_if_present(&candidate)?;
        }
    }
    Ok(())
}

fn stored_snapshot(
    directory: &Path,
    point: &SnapshotId,
) -> Result<(restic::Repository, restic::SnapshotId), CheckpointError> {
    let configuration: StorageConfiguration = catalog::read(&directory.join("storage.json"))?;
    if configuration.format != STORAGE_FORMAT {
        return Err(catalog::invalid("unsupported checkpoint storage format"));
    }
    let record: StoredSnapshot = catalog::read(&snapshot_path(directory, point))?;
    if record.format != STORAGE_FORMAT {
        return Err(catalog::invalid("unsupported stored snapshot format"));
    }
    let snapshot = restic::SnapshotId::parse(record.restic_snapshot)?;
    Ok((
        restic::Repository::new(configuration.program, configuration.repository),
        snapshot,
    ))
}

#[cfg(test)]
pub(crate) fn fake_checkpoint_store(
    root: &Path,
    directory: PathBuf,
    workspace: PathBuf,
    writers: BTreeSet<NodeName>,
) -> ResticCheckpointStore {
    ResticCheckpointStore::with_program(ResticCheckpointStoreTestConfig {
        directory,
        repository: root.join("repository"),
        workspace,
        writers,
        program: restic::ResticProgram::fake(root).expect("fake restic is available"),
    })
}

fn snapshot_path(directory: &Path, snapshot: &SnapshotId) -> PathBuf {
    directory
        .join("snapshots")
        .join(format!("{}.json", snapshot.as_str()))
}
