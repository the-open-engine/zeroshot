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
    store: FilesystemCheckpointStore,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().assert_value();
        let directory = root.path().join("checkpoints");
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace).assert_value();
        fs::write(workspace.join("source"), "initial").assert_value();
        let store = FilesystemCheckpointStore::new(
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

    fn restore(&self, checkpoint: &RunCheckpoint) -> Vec<DurableExecution> {
        restore(
            &CheckpointRestore {
                directory: self.directory.clone(),
                checkpoint_id: checkpoint.checkpoint_id.clone(),
            },
            &self.workspace,
        )
        .assert_value()
    }

    fn snapshot_count(&self) -> usize {
        fs::read_dir(self.directory.join("snapshots"))
            .assert_value()
            .count()
    }

    fn source(&self) -> String {
        fs::read_to_string(self.workspace.join("source")).assert_value()
    }
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
    assert!(fixture.restore(&points[0]).is_empty());
    assert_eq!(fixture.source(), "initial");
    assert!(!fixture.workspace.join("failed-only").exists());
    assert_eq!(fixture.restore(&points[1]), history);
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
    assert_eq!(fixture.restore(&points[0]), writer_history);
    assert_eq!(fixture.restore(&points[1]), reader_history);
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
    assert!(fixture.restore(&points[0]).is_empty());
    assert_eq!(fixture.source(), "initial");
}

#[tokio::test]
async fn final_partial_workspace_does_not_replace_the_failed_visits_input_checkpoint() {
    let fixture = Fixture::new();
    fixture
        .store
        .enter(&boundary("writer"), &[])
        .await
        .assert_value();
    let point = fixture.points().remove(0);
    fs::write(fixture.workspace.join("source"), "partial failed edits").assert_value();
    let mut writer = settled("writer", 1);
    writer.state = DurableExecutionState::Settled {
        position: HistoryPosition::new(3).assert_value(),
        outcome: WorkerOutcome::declared_failure(WorkerErrorCode::Refusal),
    };
    fixture.store.finish(&[writer]).await.assert_value();
    assert_eq!(fixture.points().len(), 1);
    assert_eq!(fixture.snapshot_count(), 2);
    assert!(fixture.restore(&point).is_empty());
    assert_eq!(fixture.source(), "initial");
    let latest: SnapshotId =
        serde_json::from_slice(&fs::read(fixture.directory.join("latest.json")).assert_value())
            .assert_value();
    filesystem::restore(
        &fixture.directory.join("snapshots"),
        &latest,
        &fixture.workspace,
    )
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
