use super::*;
use crate::native_v2_contract::NodeInvocation;
use crate::native_v2_observability::NativeV2Observability;
use crate::native_v2_runner::{
    DriverControl, DriverInvocation, NodeDriver, NodeRunnerError, NodeSession, ResolvedEnvironment,
    SessionFactory,
};
use openengine_cluster_protocol::{RunLogsParams, WorkerOutcome};

struct NeverStarted;

#[async_trait]
impl NodeDriver for NeverStarted {
    async fn run(
        &self,
        _: DriverInvocation,
        _: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        panic!("an early-cancelled ACP run must not start a node")
    }
}

#[async_trait]
impl SessionFactory for NeverStarted {
    async fn open(
        &self,
        _: &NodeInvocation,
        _: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        panic!("an early-cancelled ACP run must not open a provider session")
    }
}

#[tokio::test]
async fn acp_cancelled_before_drive_keeps_its_first_version_record() {
    let root = tempfile::tempdir().assert_value();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let state_root = root.path().join("state");
    let prepared = prepare_session(&state_root, workspace).assert_value();
    let mut profile = acp_profile(runtime("codex", "node_instance", json!({})));
    profile.runtime.size = Some(run_size("host"));
    let admitted = admit_submission(
        submission(&profile, &prepared.source, "cancel me", "seed").assert_value(),
    )
    .await
    .assert_value();
    let driver = Arc::new(NeverStarted);
    let runner = Arc::new(NativeNodeRunner::new(&admitted, driver.clone(), driver).assert_value());
    let environment = Arc::new(
        RunEnvironment::exact(&admitted.runtime, None, RunConnectionValues::new()).assert_value(),
    );
    let session = AcpSession {
        id: acp::SessionId::new("cancel-before-drive"),
        profile,
        environment,
        workspace: prepared.workspace,
        runner,
        resources: SyncMutex::new(Some(prepared.resources)),
        state: SyncMutex::new(SessionState::default()),
        turn: Mutex::new(()),
        state_root: state_root.clone(),
    };
    for _ in 0..3 {
        let active = Arc::new(ActiveTurn::new());
        active.cancel().await;
        let turn = session
            .run_turn_inner("cancel me".to_owned(), active)
            .await
            .assert_value();
        assert_eq!(turn.stop_reason, acp::StopReason::Cancelled);
        assert_eq!(turn.raw_output, json!({"failed":"force_stopped"}));
        let ledger = Arc::new(
            SqliteRunLedger::open_read_only(
                state_root
                    .join("runs")
                    .join(turn.run_id.as_str())
                    .join("runs.sqlite3"),
            )
            .assert_value(),
        );
        let (_, mut logs) = NativeV2Observability::new(ledger)
            .logs(RunLogsParams {
                run_id: turn.run_id,
                from_cursor: None,
                execution: None,
            })
            .await
            .assert_value();
        let first = logs.recv().await.assert_value().assert_value();
        assert_eq!(
            first.record.message.as_str(),
            concat!("Zeroshot version: ", env!("CARGO_PKG_VERSION"))
        );
        let failure = logs.recv().await.assert_value().assert_value();
        assert_eq!(failure.record.message.as_str(), "Run failed: force_stopped");
        assert!(logs.recv().await.assert_value().is_none());
    }
}
