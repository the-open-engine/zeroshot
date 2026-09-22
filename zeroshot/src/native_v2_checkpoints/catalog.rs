use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use openengine_cluster_protocol::{PositiveInteger, UnixTimestampMillis};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::*;
use crate::execution::platform::{self, FileAccess};

const MAX_METADATA_BYTES: u64 = 64 * 1024 * 1024;

pub(super) fn invalid(message: &'static str) -> CheckpointError {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message).into()
}

fn point_path(directory: &Path, id: &CheckpointId) -> PathBuf {
    let digest = Sha256::digest(id.as_str().as_bytes());
    directory.join("points").join(format!("{digest:x}.json"))
}

pub(super) fn read<T: DeserializeOwned>(path: &Path) -> Result<T, CheckpointError> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err(invalid("checkpoint metadata exceeds its size limit"));
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid("checkpoint metadata is invalid"))
}

fn index(directory: &Path) -> Result<Vec<RunCheckpoint>, CheckpointError> {
    match read(&directory.join("index.json")) {
        Err(CheckpointError(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(Vec::new())
        }
        result => result,
    }
}

pub(super) fn publish(
    directory: &Path,
    boundary: ExecutionBoundary,
    snapshot: SnapshotId,
    history: Vec<DurableExecution>,
) -> Result<(), CheckpointError> {
    let mut entries = index(directory)?;
    let sequence = PositiveInteger::new(entries.len() as u64 + 1)
        .map_err(|_| invalid("checkpoint sequence is exhausted"))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| invalid("checkpoint clock is unavailable"))?
        .as_millis();
    let descriptor = RunCheckpoint {
        checkpoint_id: CheckpointId::new(uuid::Uuid::now_v7().to_string())
            .map_err(|_| invalid("checkpoint identity is invalid"))?,
        sequence,
        node: boundary.node,
        map_indices: boundary.map_indices,
        loop_iterations: boundary.loop_iterations,
        created_at: UnixTimestampMillis::new(
            u64::try_from(timestamp).map_err(|_| invalid("checkpoint clock is out of range"))?,
        )
        .map_err(|_| invalid("checkpoint clock is out of range"))?,
    };
    let point = RecoveryPoint {
        descriptor: descriptor.clone(),
        snapshot,
        history,
    };
    write_atomic(&point_path(directory, &descriptor.checkpoint_id), &point)?;
    entries.push(descriptor);
    write_atomic(&directory.join("index.json"), &entries)?;
    write_atomic(&directory.join("latest.json"), &point.snapshot)
}

pub(super) fn point(directory: &Path, id: &CheckpointId) -> Result<RecoveryPoint, CheckpointError> {
    let point: RecoveryPoint = read(&point_path(directory, id))?;
    if &point.descriptor.checkpoint_id != id {
        return Err(invalid("checkpoint identity does not match"));
    }
    Ok(point)
}

pub(super) fn list(
    directory: &Path,
    params: RunCheckpointsParams,
) -> Result<RunCheckpointsResult, CheckpointError> {
    params
        .validate()
        .map_err(|_| invalid("invalid checkpoint page"))?;
    let entries = index(directory)?;
    let start = match &params.after {
        None => 0,
        Some(id) => entries
            .iter()
            .position(|entry| &entry.checkpoint_id == id)
            .map(|position| position + 1)
            .ok_or_else(|| invalid("checkpoint cursor was not found"))?,
    };
    let limit = params.page_limit() as usize;
    let checkpoints = entries
        .iter()
        .skip(start)
        .take(limit)
        .cloned()
        .collect::<Vec<_>>();
    let next_after = if start + checkpoints.len() < entries.len() {
        checkpoints.last().map(|entry| entry.checkpoint_id.clone())
    } else {
        None
    };
    Ok(RunCheckpointsResult {
        run_id: params.run_id,
        checkpoints,
        next_after,
    })
}

pub(super) fn write_atomic(path: &Path, value: &impl Serialize) -> Result<(), CheckpointError> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("checkpoint path has no parent"))?;
    platform::private_directory(parent)?;
    let temporary = parent.join(format!(".checkpoint-{}.tmp", uuid::Uuid::now_v7()));
    let mut file = platform::private_file(&temporary, FileAccess::CreateNew)?;
    let result = (|| {
        serde_json::to_writer(&mut file, value)
            .map_err(|_| invalid("checkpoint metadata cannot be encoded"))?;
        if file.metadata()?.len() > MAX_METADATA_BYTES {
            return Err(invalid("checkpoint metadata exceeds its size limit"));
        }
        file.flush()?;
        file.sync_all()?;
        drop(file);
        platform::commit_file(&temporary, path, parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
