use super::*;
use openengine_cluster_protocol::{NodeName, RunForceParams, SessionScope};
use openengine_cluster_server::{ClusterBackend, ConnectionContext};
use serde_json::json;
use tokio::sync::Notify;

struct WaitingRunner(Arc<Notify>);

#[async_trait]
impl NodeRunner for WaitingRunner {
    async fn start(&self, _request: NodeRunRequest) -> Result<NodeHandle, NodeRunnerError> {
        self.0.notify_one();
        std::future::pending().await
    }

    async fn close_run(&self, _run_id: &RunId) {}
}

#[tokio::test]
async fn force_stop_without_workspace_snapshots_discards_delivery_receipts_and_preserves_other_lineages()
 {
    let root = TestDirectory::new("force-delivery-cleanup");
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let storage = root.child("state");
    let run_id = RunId::new("force-delivery-cleanup");
    let mut submitted = submission("force-delivery-cleanup");
    submitted.runtime.workspace_checkpoints = false;
    submitted.graph = full_graph(vec![
        json!({
            "kind":"step", "name":"worker", "worker":"agent.worker@1",
            "instructions":"Wait for the force-stop fixture.",
            "input":{"kind":"null"}, "output":{"kind":"null"},
            "inputBindings":[], "writeBindings":[], "attempts":1
        }),
        success_node(),
    ]);
    submitted.runtime.nodes.insert(
        NodeName::new("worker").assert_value(),
        crate::native_v2_runner::test_support::binding(SessionScope::Execution),
    );
    let bootstrap = bootstrap(
        run_id.clone(),
        submitted,
        workspace.clone(),
        storage.clone(),
    );
    let repository = bootstrap.checkpoint_repository.clone();
    crate::execution::platform::private_directory(&repository).assert_value();
    // A prior delivery/repair visit retained these facts outside workspace snapshots.
    let retained = serde_json::to_vec(&crate::native_v2_runner::test_support::delivery_checkpoint(
        json!({"github":[], "replies":[]}),
        1,
    ))
    .assert_value();
    std::fs::write(repository.join("delivery-state.json"), &retained).assert_value();
    let unrelated = storage.join("other-lineage");
    std::fs::create_dir(&unrelated).assert_value();
    std::fs::write(unrelated.join("delivery-state.json"), &retained).assert_value();
    let entered = Arc::new(Notify::new());
    let controller = PortableRunController::start(bootstrap, |_| {
        Ok::<_, std::convert::Infallible>(PortableRuntime::new(Arc::new(WaitingRunner(
            entered.clone(),
        ))))
    })
    .await
    .assert_value();
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .assert_value();
    assert!(repository.join("delivery-state.json").exists());
    assert!(
        !storage.join("checkpoints").exists(),
        "metadata retention creates no snapshot catalogs"
    );

    let context = ConnectionContext::default();
    // The backend confirms cleanup before recording force_stopped. Retrying is idempotent.
    for _ in 0..2 {
        let forced = tokio::time::timeout(
            Duration::from_secs(5),
            ClusterBackend::run_force(
                &controller,
                &context,
                RunForceParams {
                    run_id: run_id.clone(),
                },
            ),
        )
        .await
        .assert_value()
        .assert_value();
        assert!(matches!(forced.status, RunStatus::Finished {
            terminal_result: TerminalResult::Failed { reason, .. }, ..
        } if reason.as_str() == "force_stopped"));
        assert!(!repository.exists());
        assert!(!storage.join("checkpoints").exists());
        assert_eq!(
            std::fs::read(unrelated.join("delivery-state.json")).assert_value(),
            retained
        );
        assert!(workspace.exists());
    }
    let ledger = SqliteRunLedger::open(controller.paths().ledger()).assert_value();
    assert!(
        ledger
            .get(&run_id)
            .await
            .assert_value()
            .assert_value()
            .snapshot
            .terminal
            .is_some()
    );
}
