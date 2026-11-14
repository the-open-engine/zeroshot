//! Checkpoint payloads cross allocation boundaries without a provider process or root supervisor.
use super::*;
use crate::full_v1_reducer::{
    DurableExecution, DurableExecutionState, HistoryPosition, StructuralOccurrence,
};
use crate::native_v2_candidate::test_support::NodeRequestFixture;
use crate::native_v2_contract::{ExecutionId, NodeInstanceId};
use crate::native_v2_runner::{NativeNodeRunner, NodeRunner};
use crate::native_v2_runner::conversations::test_support::{Provider, ID, CONTENT};
use crate::native_v2_supervisor::checkpoints::RunCheckpointStore;

fn history() -> Vec<DurableExecution> {
    vec![DurableExecution {
        delivery_state: None,
        dispatch_position: HistoryPosition::new(1).assert_value(),
        node_instance: NodeInstanceId::new(1).assert_value(),
        execution: ExecutionId::new(1).assert_value(),
        occurrence: StructuralOccurrence {
            node: NodeName::new("work").assert_value(),
            map_indices: Vec::new(),
        },
        attempt: openengine_cluster_protocol::PositiveInteger::new(1).assert_value(),
        input: Value::Null,
        state: DurableExecutionState::Settled {
            position: HistoryPosition::new(2).assert_value(),
            outcome: openengine_cluster_protocol::WorkerOutcome::Verified {
                output: Value::Null,
                artifacts: Vec::new(),
            },
        },
    }]
}

async fn turn(
    fixture: &CheckoutFixture,
    run: &RunId,
    home: PathBuf,
    store: &dyn RunCheckpointStore,
) -> Arc<Provider> {
    let successor = run != &fixture.run_id;
    let provider = Arc::new(Provider {
        home,
        restored: Arc::default(),
    });
    let runner =
        NativeNodeRunner::new(&fixture.admitted, provider.clone(), provider.clone()).assert_value();
    let restored = if successor {
        store.restored_conversations().await
    } else {
        None
    };
    let expect_fallback = successor && restored.is_none();
    if successor {
        runner.restore_conversations(&history(), restored).await;
    }
    let request = NodeRequestFixture {
        run_id: run.as_str(),
        node: "work",
        node_instance: 1,
        execution: if successor { 2 } else { 1 },
        worker: "agent.work@1",
        instructions: "Exercise the hosted node.",
        input: Value::Null,
        binding: fixture.admitted.runtime.nodes[&NodeName::new("work").assert_value()].clone(),
        environment: BTreeMap::new(),
    }
    .into_request();
    let mut handle = runner
        .start(request)
        .await
        .unwrap_or_else(|error| panic!("in-process provider dispatch: {error:?}"));
    let mut output = handle.take_initial_output().assert_value();
    let drain = tokio::spawn(async move {
        let mut messages = Vec::new();
        while let Ok(event) = output.recv().await {
            if let crate::native_v2_runner::DurableNodeEvent::Output { output, .. } = event {
                messages.push(output.text);
            }
        }
        messages
    });
    assert!(matches!(
        handle.completion().await.assert_value().outcome,
        openengine_cluster_protocol::WorkerOutcome::Verified { .. }
    ));
    let messages = drain.await.assert_value();
    assert_eq!(
        messages
            .iter()
            .filter(|line| line
                .contains("node work starts a new conversation: checkpoint has no conversation"))
            .count(),
        usize::from(expect_fallback)
    );
    if !successor {
        let capture = runner.capture_conversations(run).await;
        assert!(capture.notices.is_empty());
        store
            .retain_conversations(capture.files)
            .await
            .assert_value();
    }
    runner.close_run(run).await;
    provider
}

async fn fixture() -> CheckoutFixture {
    let mut fixture = CheckoutFixture::new("").await;
    let mut request = submission(
        runtime(BTreeSet::new()),
        &fixture._repository.main_revision,
        "checkpoint-conversation",
    );
    if let crate::native_v2_contract::NodeRuntimeBinding::Agent { session_scope, .. } = request
        .runtime
        .nodes
        .get_mut(&NodeName::new("work").assert_value())
        .assert_value()
    {
        *session_scope = crate::execution::SessionScope::NodeInstance;
    }
    fixture.admitted = admit(request).await;
    fixture
}

async fn direct(selection: bool) {
    let fixture = fixture().await;
    let capsule = fixture.allocate().await.assert_value();
    let first_home = fixture
        .allocator
        .run_path(&fixture.run_id)
        .join("runtime/provider-home");
    let store = capsule.checkpoints.as_ref().assert_value();
    turn(
        &fixture,
        &fixture.run_id,
        first_home.clone(),
        store.as_ref(),
    )
    .await;
    assert!(
        !first_home.exists(),
        "predecessor session homes must be gone before restoration"
    );
    store
        .enter(
            &ExecutionBoundary {
                node: NodeName::new("work").assert_value(),
                map_indices: Vec::new(),
                loop_iterations: Vec::new(),
                attempt: 1,
            },
            &history(),
        )
        .await
        .assert_value();
    store.suspend(&history(), &[]).await.assert_value();
    let selection = if selection {
        let listed = fixture.checkpoints(&fixture.run_id).await;
        CheckpointRestoreSelection::Checkpoint {
            checkpoint_id: listed.checkpoints[0].checkpoint_id.clone(),
        }
    } else {
        CheckpointRestoreSelection::Continue
    };
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Suspended)
        .await
        .assert_value();
    let successor = RunId::new("conversation-successor");
    let resumed = fixture
        .allocator
        .allocate_from_retained(fixture.retained_request(&fixture.run_id, &successor, selection))
        .await
        .assert_value();
    let next_home = fixture
        .allocator
        .run_path(&successor)
        .join("runtime/provider-home");
    assert_ne!(first_home, next_home);
    assert!(!next_home.exists());
    let provider = turn(
        &fixture,
        &successor,
        next_home.clone(),
        resumed.checkpoints.as_ref().assert_value().as_ref(),
    )
    .await;
    assert_eq!(
        *provider.restored.lock().await,
        [(ID.to_owned(), next_home, CONTENT.to_owned())]
    );
    resumed
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::ForceStopped)
        .await
        .assert_value();
}

#[tokio::test]
async fn direct_suspended_successor_restores_conversation_without_workspace_restore() {
    direct(false).await;
}

#[tokio::test]
async fn direct_selected_checkpoint_restores_conversation_after_session_cleanup() {
    direct(true).await;
}

// This host implements only the original storage contract. Its implementation deliberately does
// not name either of the conversation methods, just as an independently deployed host would not.
#[derive(Clone)]
struct LegacyStorage {
    directory: PathBuf,
    workspace: Arc<tokio::sync::Mutex<Option<PathBuf>>>,
    seed: Arc<tokio::sync::Mutex<Vec<DurableExecution>>>,
}
impl LegacyStorage {
    async fn prepared(
        &self,
        request: HostedWorkspaceRequest<'_>,
        store: Arc<dyn RunCheckpointStore>,
    ) -> Result<HostedWorkspace, crate::native_v2_supervisor::checkpoints::CheckpointError> {
        if self.directory.join("workspace").is_dir() {
            for entry in fs::read_dir(request.workspace)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    fs::remove_dir_all(entry.path())?;
                } else {
                    fs::remove_file(entry.path())?;
                }
            }
            for entry in fs::read_dir(self.directory.join("workspace"))? {
                let entry = entry?;
                crate::native_v2_capsule::provider_process::copy_workspace_entry(
                    &entry.path(),
                    &request.workspace.join(entry.file_name()),
                )?;
            }
        }
        *self.workspace.lock().await = Some(request.workspace.to_owned());
        let seed = self.seed.lock().await.clone();
        Ok(HostedWorkspace {
            resumed_from: (!seed.is_empty()).then(|| RunId::new("checkout-recovery")),
            vector_seed: Vec::new(),
            comment_publications: None,
            checkpoints: store,
            execution_seed: seed,
            delivery_run_id: None,
        })
    }
}
#[async_trait::async_trait]
impl HostedWorkspaceStorage for LegacyStorage {
    async fn retain_delivery_state(
        &self,
        _: &RunId,
        _: &crate::native_v2_contract::DeliveryCheckpoint,
    ) -> Result<(), crate::native_v2_supervisor::checkpoints::CheckpointError> {
        Ok(())
    }
    async fn prepare(
        &self,
        request: HostedWorkspaceRequest<'_>,
    ) -> Result<HostedWorkspace, crate::native_v2_supervisor::checkpoints::CheckpointError> {
        self.prepared(request, Arc::new(self.clone())).await
    }
}
#[async_trait::async_trait]
impl RunCheckpointStore for LegacyStorage {
    async fn retain_delivery_state(
        &self,
        _: &crate::native_v2_contract::DeliveryCheckpoint,
    ) -> Result<(), crate::native_v2_supervisor::checkpoints::CheckpointError> {
        Ok(())
    }
    async fn delivery_state(
        &self,
    ) -> Result<
        Option<crate::native_v2_contract::DeliveryCheckpoint>,
        crate::native_v2_supervisor::checkpoints::CheckpointError,
    > {
        Ok(None)
    }
    async fn enter(
        &self,
        _: &ExecutionBoundary,
        history: &[DurableExecution],
    ) -> Result<(), crate::native_v2_supervisor::checkpoints::CheckpointError> {
        let saved = self.directory.join("workspace");
        if saved.exists() {
            fs::remove_dir_all(&saved)?;
        }
        crate::native_v2_capsule::provider_process::copy_workspace_entry(
            self.workspace.lock().await.as_ref().unwrap(),
            &saved,
        )?;
        *self.seed.lock().await = history.to_vec();
        Ok(())
    }
    async fn finish(
        &self,
        _: &[DurableExecution],
    ) -> Result<(), crate::native_v2_supervisor::checkpoints::CheckpointError> {
        Ok(())
    }
}

#[derive(Clone)]
struct ConversationStorage(LegacyStorage);
#[async_trait::async_trait]
impl HostedWorkspaceStorage for ConversationStorage {
    async fn retain_delivery_state(
        &self,
        _: &RunId,
        _: &crate::native_v2_contract::DeliveryCheckpoint,
    ) -> Result<(), crate::native_v2_supervisor::checkpoints::CheckpointError> {
        Ok(())
    }
    async fn prepare(
        &self,
        request: HostedWorkspaceRequest<'_>,
    ) -> Result<HostedWorkspace, crate::native_v2_supervisor::checkpoints::CheckpointError> {
        self.0.prepared(request, Arc::new(self.clone())).await
    }
}
#[async_trait::async_trait]
impl RunCheckpointStore for ConversationStorage {
    async fn retain_conversations(
        &self,
        files: Option<ConversationFiles>,
    ) -> Result<(), crate::native_v2_supervisor::checkpoints::CheckpointError> {
        let path = self.0.directory.join("conversations");
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        if let Some(files) = files {
            crate::native_v2_capsule::provider_process::copy_workspace_entry(
                files.directory(),
                &path,
            )?;
        }
        Ok(())
    }
    async fn restored_conversations(&self) -> Option<ConversationFiles> {
        let path = self.0.directory.join("conversations");
        path.is_dir().then(|| ConversationFiles::new(path))
    }
    async fn retain_delivery_state(
        &self,
        _: &crate::native_v2_contract::DeliveryCheckpoint,
    ) -> Result<(), crate::native_v2_supervisor::checkpoints::CheckpointError> {
        Ok(())
    }
    async fn delivery_state(
        &self,
    ) -> Result<
        Option<crate::native_v2_contract::DeliveryCheckpoint>,
        crate::native_v2_supervisor::checkpoints::CheckpointError,
    > {
        Ok(None)
    }
    async fn enter(
        &self,
        boundary: &ExecutionBoundary,
        history: &[DurableExecution],
    ) -> Result<(), crate::native_v2_supervisor::checkpoints::CheckpointError> {
        self.0.enter(boundary, history).await
    }
    async fn finish(
        &self,
        history: &[DurableExecution],
    ) -> Result<(), crate::native_v2_supervisor::checkpoints::CheckpointError> {
        self.0.finish(history).await
    }
}

async fn external(carries: bool) {
    let mut fixture = fixture().await;
    let directory = fixture.root.child("host-storage");
    fs::create_dir(&directory).assert_value();
    let directory = directory.canonicalize().assert_value();
    let legacy = LegacyStorage {
        directory: directory.clone(),
        workspace: Arc::default(),
        seed: Arc::default(),
    };
    let storage: Arc<dyn HostedWorkspaceStorage> = if carries {
        Arc::new(ConversationStorage(legacy))
    } else {
        Arc::new(legacy)
    };
    let mut config = (*fixture.config).clone();
    config.workspace_storage = Some(storage);
    fixture.allocator =
        ProductionCapsuleAllocator::new(Arc::new(config), fixture.diagnostics.clone())
            .assert_value()
            .with_test_filesystem_and_source(
                fixture._repository.remote.clone(),
                portable_filesystem,
            );
    let capsule = fixture.allocate().await.assert_value();
    let store = capsule.checkpoints.as_ref().assert_value();
    turn(
        &fixture,
        &fixture.run_id,
        fixture.root.child("old-provider-home"),
        store.as_ref(),
    )
    .await;
    store
        .enter(
            &ExecutionBoundary {
                node: NodeName::new("work").assert_value(),
                map_indices: Vec::new(),
                loop_iterations: Vec::new(),
                attempt: 1,
            },
            &history(),
        )
        .await
        .assert_value();
    assert!(!directory.join("workspace/conversations").exists());
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();
    let successor = RunId::new("external-successor");
    // Host-owned restore is selected by the host, independently of the local retained allocator.
    let resumed = fixture
        .allocator
        .allocate(
            crate::native_v2_candidate::test_support::allocation_request(
                &successor,
                &fixture.admitted,
                Some(CHECKOUT_TOKEN),
            ),
        )
        .await
        .assert_value();
    assert_eq!(resumed.execution_seed, history());
    let provider = turn(
        &fixture,
        &successor,
        fixture.root.child("new-provider-home"),
        resumed.checkpoints.as_ref().assert_value().as_ref(),
    )
    .await;
    assert_eq!(provider.restored.lock().await.len(), usize::from(carries));
    resumed
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::ForceStopped)
        .await
        .assert_value();
}

#[tokio::test]
async fn original_external_storage_contract_starts_a_fresh_conversation_and_logs_why() {
    external(false).await;
}
#[tokio::test]
async fn external_storage_round_trips_private_conversation_files() {
    external(true).await;
}
