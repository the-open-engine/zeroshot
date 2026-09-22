use super::*;
use crate::full_v1_reducer::ExecutionBoundary;
use crate::native_v2_supervisor::checkpoints::{CheckpointError, CheckpointRestore, RunCheckpointStore};
use openengine_cluster_protocol::RunCheckpointsParams;

#[derive(Clone, Debug)]
struct Observation {
    boundary: Option<ExecutionBoundary>,
    history: Vec<DurableExecution>,
}

struct RecordingCheckpointStore {
    driver: Arc<FakeDriver>,
    ledger: Arc<FakeRunLedger>,
    records: StdMutex<Vec<Observation>>,
}

impl RecordingCheckpointStore {
    fn attach(harness: &Harness) -> (NativeV2Supervisor, Arc<Self>) {
        let store = Arc::new(Self {
            driver: harness.driver.clone(),
            ledger: harness.ledger.clone(),
            records: StdMutex::default(),
        });
        (
            harness
                .supervisor
                .clone()
                .with_checkpoints(Some(store.clone())),
            store,
        )
    }

    async fn record(&self, boundary: Option<ExecutionBoundary>, history: &[DurableExecution]) {
        assert_eq!(
            self.driver.state().active,
            0,
            "workspace capture overlapped a provider"
        );
        assert!(
            !history
                .iter()
                .any(|entry| matches!(entry.state, DurableExecutionState::Active))
        );
        let stored = stored_run(&self.ledger).await;
        assert!(stored.snapshot.active_executions().next().is_none());
        assert!(
            stored.snapshot.terminal.is_none(),
            "checkpoint must precede durable terminalization"
        );
        assert_eq!(durable_history(&stored.snapshot).assert_value(), history);
        self.records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Observation {
                boundary,
                history: history.to_vec(),
            });
    }

    fn observations(&self) -> Vec<Observation> {
        self.records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl RunCheckpointStore for RecordingCheckpointStore {
    async fn enter(
        &self,
        boundary: &ExecutionBoundary,
        history: &[DurableExecution],
    ) -> Result<(), CheckpointError> {
        self.record(Some(boundary.clone()), history).await;
        Ok(())
    }

    async fn finish(&self, history: &[DurableExecution]) -> Result<(), CheckpointError> {
        self.record(None, history).await;
        Ok(())
    }
}

fn completed_after(millis: u64) -> Behavior {
    Behavior::Complete {
        delay: Duration::from_millis(millis),
        outcome: success_for(NodeRole::Worker),
    }
}

fn mapped_waves_graph() -> GraphSpec {
    let state = record_type();
    let item_work = json!({
        "kind":"seq", "name":"item_work", "state":state,
        "children":[step("first", 1_000), step("second", 1_000)], "promotedStatePaths":[]
    });
    let batch = json!({
        "kind":"map", "name":"batch", "state":state, "body":item_work,
        "over":{"source":"state", "path":["items"]}, "maxItems":3, "promotedStatePaths":[]
    });
    graph(
        sequence(
            vec![
                step("before", 1_000),
                batch,
                step("after", 1_000),
                succeed("done"),
            ],
            state.clone(),
        ),
        state,
    )
}

#[tokio::test(start_paused = true)]
async fn map_stage_waves_only_offer_the_same_group_checkpoint_when_quiescent() {
    let harness = harness(
        mapped_waves_graph(),
        json!({"items":["a","b","c"]}),
        FakeDriver::scripted([(
            "first",
            vec![
                completed_after(0),
                completed_after(25),
                completed_after(100),
            ],
        )]),
    )
    .await;
    let (supervisor, store) = RecordingCheckpointStore::attach(&harness);
    supervisor.drive().await.assert_value();
    assert!(harness.driver.max_active() >= 2);
    assert_eq!(harness.driver.starts("first"), 3);
    assert_eq!(harness.driver.starts("second"), 3);
    let observations = store.observations();
    let entries = observations
        .iter()
        .filter(|record| record.boundary.is_some())
        .collect::<Vec<_>>();
    let mut names = entries
        .iter()
        .map(|record| record.boundary.as_ref().assert_value().node.as_str())
        .collect::<Vec<_>>();
    names.dedup();
    assert_eq!(names, ["before", "batch", "after"]);
    assert!(entries.first().assert_value().history.is_empty());
    assert_eq!(entries.last().assert_value().history.len(), 7);
    let mapped = entries
        .iter()
        .filter(|record| record.boundary.as_ref().assert_value().node.as_str() == "batch")
        .collect::<Vec<_>>();
    assert!(
        mapped.len() >= 2,
        "fixture must reach a later map stage with no active task"
    );
    let first = mapped.first().assert_value();
    assert_eq!(first.history.len(), 1);
    assert!(mapped.last().assert_value().history.len() > first.history.len());
    assert!(mapped.iter().all(|point| point.boundary == first.boundary));
    let finished = observations.last().assert_value();
    assert!(finished.boundary.is_none());
    assert_eq!(finished.history.len(), 8);
}

pub(super) fn prerequisite(id: u64, node: &str, outcome: WorkerOutcome) -> DurableExecution {
    // Model the private, serialized checkpoint history with positions from the source attempt.
    serde_json::from_value(json!({
        "dispatch_position": id * 100, "node_instance": id, "execution": id,
        "occurrence": {"node": node, "mapIndices": []}, "attempt": 1, "input": null,
        "state": {"Settled": {"position": id * 100 + 1, "outcome": outcome}}
    }))
    .assert_value()
}

fn entry_boundary(name: &str, attempt: u64) -> ExecutionBoundary {
    ExecutionBoundary {
        node: name.parse().assert_value(),
        map_indices: Vec::new(),
        loop_iterations: Vec::new(),
        attempt,
    }
}

#[tokio::test]
async fn repeated_group_entry_keeps_one_catalog_point_and_its_original_workspace() {
    let root = tempfile::tempdir().assert_value();
    let workspace = root.path().join("workspace");
    let directory = root.path().join("catalog");
    std::fs::create_dir(&workspace).assert_value();
    std::fs::write(workspace.join("state"), "before map").assert_value();
    let store = crate::native_v2_supervisor::checkpoints::fake_checkpoint_store(
        root.path(),
        directory.clone(),
        workspace.clone(),
        BTreeSet::from([
            NodeName::new("first").assert_value(),
            NodeName::new("second").assert_value(),
        ]),
    );
    let boundary = entry_boundary("batch", 0);
    store.enter(&boundary, &[]).await.assert_value();
    std::fs::write(workspace.join("state"), "partial map").assert_value();
    let mut history = vec![prerequisite(1, "first", success_for(NodeRole::Worker))];
    store.enter(&boundary, &history).await.assert_value();
    std::fs::write(workspace.join("state"), "finished map").assert_value();
    history.push(prerequisite(2, "second", success_for(NodeRole::Worker)));
    store
        .enter(&entry_boundary("after", 1), &history)
        .await
        .assert_value();
    let points = crate::native_v2_supervisor::checkpoints::list(
        &directory,
        RunCheckpointsParams {
            run_id: RunId::new("catalog-test"),
            after: None,
            limit: None,
        },
    )
    .assert_value()
    .checkpoints;
    assert_eq!(
        points
            .iter()
            .map(|entry| entry.node.as_str())
            .collect::<Vec<_>>(),
        ["batch", "after"]
    );
    let first = points.first().assert_value();
    let restored = crate::native_v2_supervisor::checkpoints::restore(
        &CheckpointRestore {
            directory: directory.clone(),
            checkpoint_id: first.checkpoint_id.clone(),
        },
        &workspace,
    )
    .await
    .assert_value();
    assert!(restored.is_empty());
    assert_eq!(
        std::fs::read_to_string(workspace.join("state")).assert_value(),
        "before map"
    );
    let last = points.last().assert_value();
    let restored = crate::native_v2_supervisor::checkpoints::restore(
        &CheckpointRestore {
            directory,
            checkpoint_id: last.checkpoint_id.clone(),
        },
        &workspace,
    )
    .await
    .assert_value();
    assert_eq!(restored, history);
    assert_eq!(
        std::fs::read_to_string(workspace.join("state")).assert_value(),
        "finished map"
    );
}
