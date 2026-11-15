//! Force-stop discards the stopped run's whole recovery lineage. Every catalog that names the
//! lineage's shared checkpoint repository goes with it, so no listing outlives the deleted
//! snapshots, while another lineage keeps its catalogs and repository.

use openengine_cluster_testkit::assertions::AssertAt;

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn force_stopped_successor_discards_every_catalog_of_its_lineage() {
    let fixture = LocalFixture::new();
    let separate = fixture.failed_run("Separate lineage").await;
    let original = fixture.failed_run("Force-stopped lineage").await;
    let separate_checkpoints = fixture.checkpoints(&separate).await;
    assert!(!separate_checkpoints.is_empty());
    assert!(!fixture.checkpoints(&original).await.is_empty());
    assert_eq!(fixture.checkpoint_repositories(), 2);

    let resumed = fixture.json(&["resume", &original], "block").await;
    let successor = resumed
        .assert_key("runId")
        .as_str()
        .assert_value_with("successor run identity")
        .to_owned();
    fixture.wait_running(&successor).await;
    assert!(!fixture.checkpoints(&successor).await.is_empty());
    fixture.json(&["force-stop", &successor], "block").await;
    fixture
        .wait_terminal(&successor, "block", "force_stopped")
        .await;
    wait_for_exit(fixture.ready_pid(&successor)).await;

    for run_id in [&original, &successor] {
        assert_eq!(
            fixture.checkpoints(run_id).await,
            Vec::<Value>::new(),
            "run {run_id} still lists checkpoints of the discarded lineage"
        );
    }
    assert_eq!(fixture.checkpoint_repositories(), 1);
    assert_eq!(fixture.checkpoints(&separate).await, separate_checkpoints);
    let checkpoint_id = separate_checkpoints
        .iter()
        .find(|checkpoint| checkpoint.assert_key("node") == "worker")
        .assert_value_with("the separate lineage captured a worker checkpoint")
        .assert_key("checkpointId")
        .as_str()
        .assert_value_with("checkpoint identity");
    let restored = fixture
        .json(
            &[
                "resume",
                &separate,
                "--from-checkpoint",
                checkpoint_id,
                "--discard-workspace-changes",
            ],
            "finish",
        )
        .await;
    let restored = restored
        .assert_key("runId")
        .as_str()
        .assert_value_with("separate successor identity");
    fixture.wait_succeeded(restored, "finish").await;
}

impl LocalFixture {
    pub(super) async fn failed_run(&self, title: &str) -> String {
        let failed = self.run(title, "fail", false).await;
        assert!(!failed.status.success(), "injected local failure succeeded");
        let run_id = json_lines(&failed.stdout)
            .as_slice()
            .assert_at(0)
            .assert_key("runId")
            .as_str()
            .assert_value_with("failed run receipt")
            .to_owned();
        wait_for_exit(self.ready_pid(&run_id)).await;
        run_id
    }

    pub(super) async fn checkpoints(&self, run_id: &str) -> Vec<Value> {
        self.json(&["checkpoints", run_id], "finish")
            .await
            .assert_key("checkpoints")
            .as_array()
            .assert_value_with("checkpoint list")
            .clone()
    }

    fn checkpoint_repositories(&self) -> usize {
        fs::read_dir(self.state.join("checkpoint-repositories"))
            .assert_value_with("list checkpoint repositories")
            .count()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn force_stopped_suspension_discards_checkpoints_and_cannot_continue() {
    let fixture = LocalFixture::new().with_second_worker();
    let run = fixture.submit_detached("hold").await;
    fixture.wait_running(&run).await;
    fixture.json(&["suspend", &run], "hold").await;
    fs::write(fixture.repository.join("release-turn"), b"release").assert_value();
    fixture.wait_suspended(&run).await;
    wait_for_exit(fixture.ready_pid(&run)).await;
    let before = fixture.json(&["status", &run], "finish").await;
    assert!(
        !fixture
            .output(&["suspend", &run], "finish")
            .await
            .status
            .success()
    );
    assert_eq!(fixture.json(&["status", &run], "finish").await, before);
    assert!(!fixture.checkpoints(&run).await.is_empty());
    fixture.json(&["force-stop", &run], "finish").await;
    fixture.wait_terminal(&run, "finish", "force_stopped").await;
    assert!(fixture.checkpoints(&run).await.is_empty());
    assert_eq!(fixture.checkpoint_repositories(), 0);
    assert!(
        !fixture
            .output(&["resume", &run], "finish")
            .await
            .status
            .success()
    );
}
