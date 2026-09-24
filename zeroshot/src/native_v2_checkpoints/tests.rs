use std::fs;
use std::path::PathBuf;

use openengine_cluster_protocol::{PositiveInteger, RunId, WorkerErrorCode, WorkerOutcome};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;
use crate::full_v1_reducer::{ExecutionId, HistoryPosition, NodeInstanceId, StructuralOccurrence};

struct Fixture {
    _root: tempfile::TempDir,
    directory: PathBuf,
    workspace: PathBuf,
    store: ResticCheckpointStore,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().assert_value();
        let directory = root.path().join("checkpoints");
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace).assert_value();
        fs::write(workspace.join("source"), "initial").assert_value();
        let store = fake_checkpoint_store(
            root.path(),
            directory.clone(),
            workspace.clone(),
            BTreeSet::from([NodeName::new("writer").assert_value()]),
        );
        Self {
            _root: root,
            directory,
            workspace,
            store,
        }
    }

    fn points(&self) -> Vec<RunCheckpoint> {
        list(
            &self.directory,
            RunCheckpointsParams {
                run_id: RunId::new("checkpoint-store-tests"),
                after: None,
                limit: None,
            },
        )
        .assert_value()
        .checkpoints
    }

    async fn restore(&self, checkpoint: &RunCheckpoint) -> Vec<DurableExecution> {
        restore(
            &CheckpointRestore {
                directory: self.directory.clone(),
                selection: CheckpointRestoreSelection::Checkpoint {
                    checkpoint_id: checkpoint.checkpoint_id.clone(),
                },
            },
            &self.workspace,
        )
        .await
        .unwrap_or_else(|error| panic!("checkpoint restore failed: {error:?}"))
    }

    async fn input_checkpoint(&self, node: &str) -> RunCheckpoint {
        self.store.enter(&boundary(node), &[]).await.assert_value();
        self.points().remove(0)
    }

    async fn restore_latest(&self) -> Vec<DurableExecution> {
        restore(
            &CheckpointRestore {
                directory: self.directory.clone(),
                selection: CheckpointRestoreSelection::Latest,
            },
            &self.workspace,
        )
        .await
        .unwrap_or_else(|error| panic!("latest restore failed: {error:?}"))
    }

    fn snapshot_count(&self) -> usize {
        fs::read_dir(self.directory.join("snapshots"))
            .assert_value()
            .count()
    }

    fn source(&self) -> String {
        fs::read_to_string(self.workspace.join("source")).assert_value()
    }

    fn restic_control(&self, name: &str) -> PathBuf {
        self._root.path().join("repository").join(name)
    }
}

#[tokio::test]
async fn catalog_pages_exclusively_and_rejects_invalid_or_unknown_cursors() {
    let fixture = Fixture::new();
    let first = fixture.input_checkpoint("writer").await;
    let snapshot = catalog::point(&fixture.directory, &first.checkpoint_id)
        .assert_value()
        .snapshot;
    for node in ["reader", "writer"] {
        catalog::publish(
            &fixture.directory,
            boundary(node),
            snapshot.clone(),
            Vec::new(),
        )
        .assert_value();
    }

    let run_id = RunId::new("checkpoint-pages");
    assert_eq!(
        catalog::list(
            &fixture.directory,
            RunCheckpointsParams {
                run_id: run_id.clone(),
                after: None,
                limit: Some(0),
            },
        )
        .unwrap_err()
        .0
        .kind(),
        std::io::ErrorKind::InvalidData
    );

    let first_page = catalog::list(
        &fixture.directory,
        RunCheckpointsParams {
            run_id: run_id.clone(),
            after: None,
            limit: Some(1),
        },
    )
    .assert_value();
    assert_eq!(first_page.checkpoints, vec![first.clone()]);
    assert_eq!(first_page.next_after, Some(first.checkpoint_id.clone()));

    let tail = catalog::list(
        &fixture.directory,
        RunCheckpointsParams {
            run_id: run_id.clone(),
            after: first_page.next_after,
            limit: Some(100),
        },
    )
    .assert_value();
    assert_eq!(tail.run_id, run_id);
    assert_eq!(tail.checkpoints.len(), 2);
    assert!(tail.next_after.is_none());

    let error = catalog::list(
        &fixture.directory,
        RunCheckpointsParams {
            run_id: RunId::new("checkpoint-pages"),
            after: Some(CheckpointId::new("unknown-checkpoint").assert_value()),
            limit: Some(1),
        },
    )
    .unwrap_err();
    assert_eq!(error.0.kind(), std::io::ErrorKind::InvalidData);
}

#[tokio::test]
async fn catalog_rejects_tampered_point_identity_and_format_and_malformed_metadata() {
    let fixture = Fixture::new();
    let checkpoint = fixture.input_checkpoint("writer").await;
    let point_path = fs::read_dir(fixture.directory.join("points"))
        .assert_value()
        .next()
        .assert_value()
        .assert_value()
        .path();
    let original: serde_json::Value =
        serde_json::from_slice(&fs::read(&point_path).assert_value()).assert_value();

    let mut incompatible = original.clone();
    incompatible["version"] = json!(RECOVERY_POINT_FORMAT + 1);
    fs::write(
        &point_path,
        serde_json::to_vec(&incompatible).assert_value(),
    )
    .assert_value();
    assert_eq!(
        catalog::point(&fixture.directory, &checkpoint.checkpoint_id)
            .unwrap_err()
            .0
            .kind(),
        std::io::ErrorKind::InvalidData
    );

    let mut mismatched = original;
    mismatched["descriptor"]["checkpointId"] = json!("different-checkpoint");
    fs::write(&point_path, serde_json::to_vec(&mismatched).assert_value()).assert_value();
    assert_eq!(
        catalog::point(&fixture.directory, &checkpoint.checkpoint_id)
            .unwrap_err()
            .0
            .kind(),
        std::io::ErrorKind::InvalidData
    );

    fs::write(fixture.directory.join("index.json"), b"{").assert_value();
    assert_eq!(
        catalog::list(
            &fixture.directory,
            RunCheckpointsParams {
                run_id: RunId::new("malformed-catalog"),
                after: None,
                limit: None,
            },
        )
        .unwrap_err()
        .0
        .kind(),
        std::io::ErrorKind::InvalidData
    );
}

#[test]
fn catalog_atomic_write_removes_temporary_data_when_serialization_fails() {
    let root = tempfile::tempdir().assert_value();
    let directory = root.path().join("catalog");
    let destination = directory.join("point.json");
    let invalid_json_object = std::collections::BTreeMap::from([(vec![0_u8], ())]);
    let error = catalog::write_atomic(&destination, &invalid_json_object).unwrap_err();
    assert_eq!(error.0.kind(), std::io::ErrorKind::InvalidData);
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(directory).assert_value().count(), 0);
}

#[tokio::test]
async fn restart_restores_latest_workspace_without_reading_the_checkpoint_seed() {
    let fixture = Fixture::new();
    let point = fixture.input_checkpoint("writer").await;
    fs::write(
        catalog::seed_path(&fixture.directory, &point.checkpoint_id),
        "incompatible seed",
    )
    .assert_value();
    fs::write(fixture.workspace.join("source"), "partial failed write").assert_value();

    let seed = fixture.restore_latest().await;

    assert!(seed.is_empty());
    assert_eq!(fixture.source(), "initial");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn restore_stages_beside_a_workspace_on_another_filesystem() {
    use std::os::unix::fs::MetadataExt as _;

    let state = tempfile::tempdir().assert_value();
    let Ok(workspace_root) = tempfile::Builder::new()
        .prefix("zeroshot-restore-")
        .tempdir_in("/dev/shm")
    else {
        return;
    };
    if state.path().metadata().assert_value().dev()
        == workspace_root.path().metadata().assert_value().dev()
    {
        return;
    }
    let workspace = workspace_root.path().join("workspace");
    let directory = state.path().join("checkpoints");
    fs::create_dir(&workspace).assert_value();
    fs::write(workspace.join("source"), "saved").assert_value();
    let store = fake_checkpoint_store(
        state.path(),
        directory.clone(),
        workspace.clone(),
        BTreeSet::from([NodeName::new("writer").assert_value()]),
    );
    store.enter(&boundary("writer"), &[]).await.assert_value();
    let point = list(
        &directory,
        RunCheckpointsParams {
            run_id: RunId::new("cross-filesystem"),
            after: None,
            limit: None,
        },
    )
    .assert_value()
    .checkpoints
    .remove(0);
    fs::write(workspace.join("source"), "corrupt").assert_value();

    restore(
        &CheckpointRestore {
            directory,
            selection: CheckpointRestoreSelection::Checkpoint {
                checkpoint_id: point.checkpoint_id,
            },
        },
        &workspace,
    )
    .await
    .assert_value();

    assert_eq!(
        fs::read_to_string(workspace.join("source")).assert_value(),
        "saved"
    );
}

fn boundary(node: &str) -> ExecutionBoundary {
    ExecutionBoundary {
        node: NodeName::new(node).assert_value(),
        map_indices: Vec::new(),
        loop_iterations: Vec::new(),
        attempt: 1,
    }
}

fn settled(node: &str, sequence: u64) -> DurableExecution {
    let occurrence = StructuralOccurrence {
        node: boundary(node).node,
        map_indices: Vec::new(),
    };
    DurableExecution {
        dispatch_position: HistoryPosition::new(sequence * 2).assert_value(),
        node_instance: NodeInstanceId::new(sequence).assert_value(),
        execution: ExecutionId::new(sequence).assert_value(),
        occurrence,
        attempt: PositiveInteger::new(1).assert_value(),
        input: json!({"input": node}),
        state: DurableExecutionState::Settled {
            position: HistoryPosition::new(sequence * 2 + 1).assert_value(),
            outcome: WorkerOutcome::Verified {
                output: json!({"output": node}),
                artifacts: Vec::new(),
            },
        },
    }
}

#[tokio::test]
async fn input_checkpoints_restore_matching_workspace_bytes_and_execution_history() {
    let fixture = Fixture::new();
    assert!(!fixture.directory.exists());
    fixture
        .store
        .enter(&boundary("writer"), &[])
        .await
        .assert_value();
    fs::write(fixture.workspace.join("source"), "writer output").assert_value();
    let history = vec![settled("writer", 1)];
    fixture
        .store
        .enter(&boundary("reader"), &history)
        .await
        .assert_value();
    let points = fixture.points();
    assert_eq!(points.len(), 2);
    assert_eq!(fixture.snapshot_count(), 2);
    fs::write(fixture.workspace.join("failed-only"), "partial").assert_value();
    assert!(fixture.restore(&points[0]).await.is_empty());
    assert_eq!(fixture.source(), "initial");
    assert!(!fixture.workspace.join("failed-only").exists());
    assert_eq!(fixture.restore(&points[1]).await, history);
    assert_eq!(fixture.source(), "writer output");
}

#[tokio::test]
async fn readonly_visits_share_snapshot_bytes_but_keep_distinct_input_histories() {
    let fixture = Fixture::new();
    let writer_history = vec![settled("writer", 1)];
    fixture
        .store
        .enter(&boundary("reader"), &writer_history)
        .await
        .assert_value();
    let mut reader_history = writer_history.clone();
    reader_history.push(settled("reader", 2));
    fixture
        .store
        .enter(&boundary("reviewer"), &reader_history)
        .await
        .assert_value();
    let points = fixture.points();
    assert_eq!(points.len(), 2);
    assert_eq!(fixture.snapshot_count(), 1);
    assert_eq!(fixture.restore(&points[0]).await, writer_history);
    assert_eq!(fixture.restore(&points[1]).await, reader_history);
}

#[tokio::test]
async fn repeated_map_batches_publish_one_input_checkpoint_for_the_atomic_group() {
    let fixture = Fixture::new();
    let mut group = boundary("map-group");
    group.loop_iterations = vec![2];
    fixture.store.enter(&group, &[]).await.assert_value();
    fs::write(fixture.workspace.join("source"), "first batch output").assert_value();
    let mut writer = settled("writer", 1);
    writer.occurrence.map_indices = vec![0];
    fixture.store.enter(&group, &[writer]).await.assert_value();
    let points = fixture.points();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].loop_iterations, vec![2]);
    assert_eq!(fixture.snapshot_count(), 1);
    assert!(fixture.restore(&points[0]).await.is_empty());
    assert_eq!(fixture.source(), "initial");
}

#[tokio::test]
async fn final_partial_workspace_does_not_replace_the_failed_visits_input_checkpoint() {
    let fixture = Fixture::new();
    let point = fixture.input_checkpoint("writer").await;
    fs::write(fixture.workspace.join("source"), "partial failed edits").assert_value();
    let mut writer = settled("writer", 1);
    writer.state = DurableExecutionState::Settled {
        position: HistoryPosition::new(3).assert_value(),
        outcome: WorkerOutcome::declared_failure(WorkerErrorCode::Refusal),
    };
    fixture.store.finish(&[writer]).await.assert_value();
    assert_eq!(fixture.points().len(), 1);
    assert_eq!(fixture.snapshot_count(), 2);
    assert!(fixture.restore(&point).await.is_empty());
    assert_eq!(fixture.source(), "initial");
    let latest: SnapshotId =
        serde_json::from_slice(&fs::read(fixture.directory.join("latest.json")).assert_value())
            .assert_value();
    restore_snapshot(&fixture.directory, &latest, &fixture.workspace)
        .await
        .assert_value();
    assert_eq!(fixture.source(), "partial failed edits");
}

#[tokio::test]
async fn active_executions_cannot_publish_or_finish_a_checkpoint() {
    let fixture = Fixture::new();
    let mut active = settled("writer", 1);
    active.state = DurableExecutionState::Active;
    assert!(
        fixture
            .store
            .enter(&boundary("reader"), std::slice::from_ref(&active))
            .await
            .is_err()
    );
    assert!(fixture.store.finish(&[active]).await.is_err());
    assert!(!fixture.directory.exists());
}

#[tokio::test]
async fn failed_backup_does_not_publish_and_the_prior_checkpoint_stays_restorable() {
    let fixture = Fixture::new();
    let point = fixture.input_checkpoint("writer").await;
    fs::write(fixture.workspace.join("source"), "uncommitted update").assert_value();
    fs::write(fixture.restic_control("fail-backup"), "fail").assert_value();

    assert!(
        fixture
            .store
            .enter(&boundary("reader"), &[settled("writer", 1)])
            .await
            .is_err()
    );
    assert_eq!(fixture.points(), std::slice::from_ref(&point));
    assert_eq!(fixture.snapshot_count(), 1);

    fs::remove_file(fixture.restic_control("fail-backup")).assert_value();
    fixture.restore(&point).await;
    assert_eq!(fixture.source(), "initial");
}

#[tokio::test]
async fn failed_restore_does_not_touch_the_live_workspace_and_can_be_retried() {
    let fixture = Fixture::new();
    let point = fixture.input_checkpoint("writer").await;
    fs::write(fixture.workspace.join("source"), "live edits").assert_value();
    fs::write(fixture.restic_control("fail-restore"), "fail").assert_value();

    assert!(
        restore(
            &CheckpointRestore {
                directory: fixture.directory.clone(),
                selection: CheckpointRestoreSelection::Checkpoint {
                    checkpoint_id: point.checkpoint_id.clone(),
                },
            },
            &fixture.workspace,
        )
        .await
        .is_err()
    );
    assert_eq!(fixture.source(), "live edits");

    fs::remove_file(fixture.restic_control("fail-restore")).assert_value();
    fixture.restore(&point).await;
    assert_eq!(fixture.source(), "initial");
}

#[tokio::test]
async fn invalid_restic_snapshot_mapping_is_rejected_before_workspace_changes() {
    let fixture = Fixture::new();
    let point = fixture.input_checkpoint("writer").await;
    let mapping = fs::read_dir(fixture.directory.join("snapshots"))
        .assert_value()
        .next()
        .assert_value()
        .assert_value()
        .path();
    fs::write(mapping, br#"{"format":1,"resticSnapshot":"../repository"}"#).assert_value();
    fs::write(fixture.workspace.join("source"), "live edits").assert_value();

    assert!(
        restore(
            &CheckpointRestore {
                directory: fixture.directory.clone(),
                selection: CheckpointRestoreSelection::Checkpoint {
                    checkpoint_id: point.checkpoint_id,
                },
            },
            &fixture.workspace,
        )
        .await
        .is_err()
    );
    assert_eq!(fixture.source(), "live edits");
}

#[tokio::test]
async fn storage_identity_and_snapshot_formats_fail_closed_without_losing_the_prior_point() {
    let fixture = Fixture::new();
    let point = fixture.input_checkpoint("writer").await;
    let storage_path = fixture.directory.join("storage.json");
    let storage: serde_json::Value =
        serde_json::from_slice(&fs::read(&storage_path).assert_value()).assert_value();
    let mapping_path = fs::read_dir(fixture.directory.join("snapshots"))
        .assert_value()
        .next()
        .assert_value()
        .assert_value()
        .path();
    let mapping: serde_json::Value =
        serde_json::from_slice(&fs::read(&mapping_path).assert_value()).assert_value();

    let mut changed_storage = storage.clone();
    changed_storage["repository"] = json!(fixture._root.path().join("other-repository"));
    fs::write(
        &storage_path,
        serde_json::to_vec(&changed_storage).assert_value(),
    )
    .assert_value();
    let reopened = fake_checkpoint_store(
        fixture._root.path(),
        fixture.directory.clone(),
        fixture.workspace.clone(),
        BTreeSet::from([NodeName::new("writer").assert_value()]),
    );
    assert!(
        reopened
            .enter(&boundary("reader"), &[settled("writer", 1)])
            .await
            .is_err()
    );
    assert_eq!(fixture.points(), vec![point.clone()]);

    let restore_metadata = || {
        fs::write(&storage_path, serde_json::to_vec(&storage).assert_value()).assert_value();
        fs::write(&mapping_path, serde_json::to_vec(&mapping).assert_value()).assert_value();
    };
    for (path, original) in [(&storage_path, &storage), (&mapping_path, &mapping)] {
        restore_metadata();
        let mut incompatible = original.clone();
        incompatible["format"] = json!(STORAGE_FORMAT + 1);
        fs::write(path, serde_json::to_vec(&incompatible).assert_value()).assert_value();
        assert!(
            validate_selection(&CheckpointRestore {
                directory: fixture.directory.clone(),
                selection: CheckpointRestoreSelection::Checkpoint {
                    checkpoint_id: point.checkpoint_id.clone(),
                },
            })
            .is_err()
        );
    }

    restore_metadata();
    fs::write(fixture.workspace.join("source"), "live edits").assert_value();
    fixture.restore(&point).await;
    assert_eq!(fixture.source(), "initial");
}

#[tokio::test]
async fn real_restic_deduplicates_incremental_stages_and_restores_the_latest_bytes() {
    let Some(executable) = std::env::var_os("ZEROSHOT_RESTIC") else {
        return;
    };
    let root = tempfile::tempdir().assert_value();
    let directory = root.path().join("checkpoints");
    let repository = root.path().join("lineage");
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).assert_value();
    let mut bytes = vec![0_u8; 32 * 1024 * 1024];
    let mut value = 0x9e37_79b9_7f4a_7c15_u64;
    for chunk in bytes.chunks_mut(8) {
        value ^= value << 7;
        value ^= value >> 9;
        value ^= value << 8;
        chunk.copy_from_slice(&value.to_le_bytes()[..chunk.len()]);
    }
    fs::write(workspace.join("large.bin"), &bytes).assert_value();
    let program = restic::ResticProgram::test(PathBuf::from(executable), Vec::new()).assert_value();
    let store = ResticCheckpointStore::with_program(ResticCheckpointStoreTestConfig {
        directory: directory.clone(),
        repository: repository.clone(),
        workspace: workspace.clone(),
        writers: BTreeSet::from([NodeName::new("writer").assert_value()]),
        program,
    });

    store.enter(&boundary("writer"), &[]).await.assert_value();
    bytes[..4096].fill(0xa5);
    fs::write(workspace.join("large.bin"), &bytes).assert_value();
    store
        .enter(&boundary("reader"), &[settled("writer", 1)])
        .await
        .assert_value();

    let points = list(
        &directory,
        RunCheckpointsParams {
            run_id: RunId::new("real-restic-test"),
            after: None,
            limit: None,
        },
    )
    .assert_value()
    .checkpoints;
    fs::write(workspace.join("large.bin"), "corrupt live workspace").assert_value();
    restore(
        &CheckpointRestore {
            directory: directory.clone(),
            selection: CheckpointRestoreSelection::Checkpoint {
                checkpoint_id: points[1].checkpoint_id.clone(),
            },
        },
        &workspace,
    )
    .await
    .assert_value();
    assert_eq!(fs::read(workspace.join("large.bin")).assert_value(), bytes);
    assert_eq!(
        fs::read_dir(directory.join("staging"))
            .assert_value()
            .count(),
        0
    );

    fn stored_bytes(path: &std::path::Path) -> u64 {
        let metadata = fs::symlink_metadata(path).assert_value();
        if metadata.is_dir() {
            fs::read_dir(path)
                .assert_value()
                .map(|entry| stored_bytes(&entry.assert_value().path()))
                .sum()
        } else {
            metadata.len()
        }
    }
    let repository_bytes = stored_bytes(&repository);
    let two_full_copies = (bytes.len() * 2) as u64;
    assert!(
        repository_bytes < two_full_copies * 3 / 4,
        "Restic used {repository_bytes} bytes for {two_full_copies} bytes of full copies"
    );
}
