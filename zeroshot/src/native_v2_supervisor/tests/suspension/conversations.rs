use super::*;
use crate::native_v2_runner::conversations::test_support::Provider;
use crate::native_v2_runner::NodeRunner;
use crate::native_v2_supervisor::checkpoints::{
    fake_checkpoint_store, CheckpointRestore, CheckpointRestoreSelection,
};

struct Driver {
    provider: Arc<Provider>,
    fake: FakeDriver,
}
#[async_trait]
impl NodeDriver for Driver {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        if invocation.node.reference.node.as_str() == "worker" {
            self.provider.run(invocation, control).await
        } else {
            self.fake.run(invocation, control).await
        }
    }
}

#[tokio::test]
async fn due_wait_successor_opens_the_checkpoint_conversation() {
    let _root = tempfile::tempdir().assert_value();
    let root = _root.path().canonicalize().assert_value();
    let workspace = root.as_path().join("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let state = json!({"kind":"record","fields":{"durationMs":{"required":true,"type":{"kind":"integer"}}}});
    let spec = graph(
        sequence(
            vec![
                json!({"kind":"loop","name":"twice","state":state,"maxIterations":2,
        "until":signal_guard("check","accepted"),"promotedStatePaths":[],
        "body":{"kind":"seq","name":"round","state":state,"promotedStatePaths":[],
                "children":[step("worker",10000),wait_node(),verifier("check",10000)]}}),
                succeed("done"),
            ],
            state.clone(),
        ),
        state,
    );
    let catalog = root.as_path().join("catalog");
    let writers = BTreeSet::from([
        NodeName::new("worker").assert_value(),
        NodeName::new("check").assert_value(),
    ]);
    let store = Arc::new(fake_checkpoint_store(
        root.as_path(),
        catalog.clone(),
        workspace.clone(),
        writers.clone(),
    ));
    let provider = Arc::new(Provider {
        home: root.as_path().join("first-home"),
        restored: Arc::default(),
    });
    let mut predecessor = harness_with_session_scope(
        spec.clone(),
        json!({"durationMs":7_200_000}),
        FakeDriver::default(),
        SessionScope::NodeInstance,
    )
    .await;
    let admitted = predecessor
        .ledger
        .get(&predecessor.supervisor.run_id)
        .await
        .assert_value()
        .assert_value()
        .admitted;
    predecessor.supervisor.runner = Arc::new(
        NativeNodeRunner::new(
            &admitted,
            Arc::new(Driver {
                provider: provider.clone(),
                fake: FakeDriver::default(),
            }),
            provider.clone(),
        )
        .assert_value(),
    );
    predecessor.supervisor = predecessor
        .supervisor
        .with_checkpoints(Some(store))
        .with_wait_threshold(Some(Duration::from_secs(3600)))
        .with_wall_clock(Arc::new(|| {
            UnixTimestampMillis::new(1_000_000).assert_value()
        }));
    let outcome = predecessor.supervisor.drive().await;
    assert!(
        matches!(outcome, Err(NativeV2SupervisorError::Suspended)),
        "predecessor: {outcome:?}; snapshot: {:?}",
        predecessor.supervisor.snapshot().await.assert_value()
    );
    assert!(!provider.home.exists());
    let history = continuation_history(&predecessor.supervisor.snapshot().await.assert_value())
        .assert_value();
    let next_provider = Arc::new(Provider {
        home: root.as_path().join("second-home"),
        restored: Arc::default(),
    });
    let mut successor = harness_with_session_scope(
        spec,
        json!({"durationMs":7_200_000}),
        FakeDriver::default(),
        SessionScope::NodeInstance,
    )
    .await;
    let ledger = Arc::new(FakeRunLedger::new());
    let run_id = RunId::new("due-successor");
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: openengine_cluster_protocol::IdempotencyKey::new("due-successor")
                .assert_value(),
            submission_digest: Sha256Digest::new("1".repeat(64)).assert_value(),
            admitted: admitted.clone(),
        })
        .await
        .assert_value();
    ledger
        .append(
            &run_id,
            history
                .into_iter()
                .map(|execution| RunEvent::PriorExecution { execution })
                .collect(),
        )
        .await
        .assert_value();
    let driver = Driver {
        provider: next_provider.clone(),
        fake: FakeDriver::scripted([(
            "check",
            vec![Behavior::Complete {
                delay: Duration::ZERO,
                outcome: verifier_outcome("rejected"),
            }],
        )]),
    };
    let runner: Arc<dyn NodeRunner> = Arc::new(
        NativeNodeRunner::new(&admitted, Arc::new(driver), next_provider.clone()).assert_value(),
    );
    successor.supervisor.run_id = run_id;
    successor.supervisor.ledger = ledger;
    successor.supervisor.runner = runner;
    successor.supervisor = successor
        .supervisor
        .with_checkpoints(Some(Arc::new(
            fake_checkpoint_store(
                root.as_path(),
                root.as_path().join("successor-catalog"),
                workspace,
                writers,
            )
            .with_conversation_restore(Some(CheckpointRestore {
                directory: catalog,
                selection: CheckpointRestoreSelection::Continue,
            })),
        )))
        .with_wait_threshold(Some(Duration::from_secs(3600)))
        .with_wall_clock(Arc::new(|| {
            UnixTimestampMillis::new(8_200_000).assert_value()
        }));
    assert!(matches!(
        successor.supervisor.drive().await,
        Err(NativeV2SupervisorError::Suspended)
    ));
    let restored = next_provider.restored.lock().await;
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].1, next_provider.home);
}
