use super::*;
use crate::full_v1_reducer::{DurableExecution, ExecutionBoundary};
use crate::native_v2_supervisor::checkpoints::{
    CheckpointError, CheckpointRestoreSelection, RunCheckpointStore,
};
use openengine_cluster_protocol::{
    RunResumeParams, RunSuspendParams, RunSuspension, RunVector, RunVectorContent,
    RunVectorsParams, UnixTimestampMillis, WorkspaceRecovery,
};
use std::sync::atomic::AtomicU64;

#[derive(Default)]
struct SuspensionStore {
    published: StdMutex<Vec<RunVector>>,
}

#[async_trait]
impl RunCheckpointStore for SuspensionStore {
    async fn retain_delivery_state(
        &self,
        _: &crate::native_v2_contract::DeliveryCheckpoint,
    ) -> Result<(), CheckpointError> {
        Ok(())
    }
    async fn delivery_state(
        &self,
    ) -> Result<Option<crate::native_v2_contract::DeliveryCheckpoint>, CheckpointError> {
        Ok(None)
    }
    async fn enter(
        &self,
        _: &ExecutionBoundary,
        _: &[DurableExecution],
    ) -> Result<(), CheckpointError> {
        Ok(())
    }
    async fn finish(&self, _: &[DurableExecution]) -> Result<(), CheckpointError> {
        Ok(())
    }
    async fn suspend(
        &self,
        history: &[DurableExecution],
        vectors: &[RunVector],
    ) -> Result<(), CheckpointError> {
        assert_eq!(
            history.len(),
            1,
            "settled wait is part of the stored continuation"
        );
        *self.published.lock().assert_value() = vectors.to_vec();
        Ok(())
    }
}

struct SuspensionAllocator {
    runtime: Arc<FakeAllocator>,
    policy: crate::native_v2_wait::WaitPolicy,
    store: Arc<SuspensionStore>,
    successor: StdMutex<Option<RunId>>,
    preparation_gate: Option<watch::Sender<bool>>,
}

#[async_trait]
impl CapsuleAllocator for SuspensionAllocator {
    fn suspension_policy(&self) -> Option<crate::native_v2_wait::WaitPolicy> {
        Some(self.policy.clone())
    }
    async fn claim_controller(
        &self,
        run: &RunId,
    ) -> Result<Arc<dyn ExclusiveControllerClaim>, ControllerClaimUnavailable> {
        self.runtime.claim_controller(run).await
    }
    async fn allocate(
        &self,
        request: CapsuleAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        let mut capsule = self.runtime.allocate(request).await?;
        capsule.checkpoints = Some(self.store.clone());
        Ok(capsule)
    }
    async fn reserve_retained_workspace(
        &self,
        _source: &RunId,
        run_id: &RunId,
        _selection: &CheckpointRestoreSelection,
    ) -> Result<(), RetainedAllocationUnavailable> {
        let mut successor = self.successor.lock().assert_value();
        if successor.is_some() {
            return Err(CapsuleAllocationUnavailable::Runtime.into());
        }
        *successor = Some(run_id.clone());
        Ok(())
    }
    async fn allocate_from_retained(
        &self,
        request: RetainedAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, RetainedAllocationUnavailable> {
        assert!(matches!(
            request.selection,
            CheckpointRestoreSelection::Continue
        ));
        assert_eq!(
            self.successor.lock().assert_value().as_ref(),
            Some(request.run_id)
        );
        if let Some(gate) = &self.preparation_gate {
            let mut released = gate.subscribe();
            released
                .wait_for(|released| *released)
                .await
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        }
        self.allocate(CapsuleAllocationRequest {
            run_id: request.run_id,
            admitted: request.admitted,
            preparation: request.preparation,
            github_token: request.github_token,
            git_credentials: request.git_credentials,
            commit_identity: request.commit_identity,
            commit_co_author: request.commit_co_author,
        })
        .await
        .map_err(Into::into)
    }
    async fn workspace_recovery(&self, _: &RunId) -> WorkspaceRecovery {
        let successor = self.successor.lock().assert_value().clone();
        WorkspaceRecovery {
            recoverable: successor.is_none(),
            successor_run_id: successor,
            ..Default::default()
        }
    }
    async fn destroy_or_confirm_absent(
        &self,
        run: &RunId,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        self.runtime.destroy_or_confirm_absent(run, exit).await
    }
}

fn wait_request() -> RunSubmitParams {
    let mut request = request(Value::Null);
    request.submission.graph = serde_json::from_value(json!({
        "profile":"openengine.graph.full/v1", "initialInput":{"kind":"record","fields":{"durationMs":{"required":true,"type":{"kind":"integer"}}}},
        "policy":{"policy":"policy.native-v2@1","default":"deny"},
        "root":{"kind":"seq","name":"root","state":{"kind":"record","fields":{"durationMs":{"required":true,"type":{"kind":"integer"}}}},"promotedStatePaths":[],"children":[
            {"kind":"verifier","name":"wait","worker":crate::native_v2_wait::WAIT_WORKER_REF,
             "input":crate::native_v2_wait::input_schema().assert_value(),"output":{"kind":"null"},
             "signals":crate::native_v2_wait::signal_labels().assert_value(),
             "diagnostic":crate::native_v2_wait::diagnostic_schema().assert_value(),
             "inputBindings":[{"target":["durationMs"],"value":{"source":"state","path":["durationMs"]}}],"writeBindings":[],"attempts":1},
            {"kind":"step","name":"worker","worker":"agent.worker@1","instructions":"Work",
             "input":{"kind":"null"},"output":{"kind":"null"},"inputBindings":[],"writeBindings":[],"attempts":1},
            {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
        ]}
    })).assert_value();
    request.submission.initial_input = json!({"durationMs":7200000});
    let mut runtime = serde_json::to_value(&request.submission.runtime).assert_value();
    runtime["nodes"]
        .as_object_mut()
        .assert_value()
        .remove("deliver");
    runtime["nodes"]["wait"] = json!({"kind":"request"});
    request.submission.runtime = serde_json::from_value(runtime).assert_value();
    request
}

fn resume_params(run: &RunId) -> RunResumeParams {
    RunResumeParams {
        run_id: run.clone(),
        successor_run_id: RunId::new("wait-successor"),
        from: None,
        connections: BTreeMap::from([(
            ConnectionKey::new("test").assert_value(),
            test_connection_values("fresh-secret"),
        )]),
        connection_resolver: None,
        github_token: None,
        git_credentials: None,
        commit_identity: None,
        commit_co_author: None,
    }
}

#[tokio::test]
async fn pending_continuation_reserves_source_against_duplicate_resume_and_force_stop() {
    let driver = Arc::new(FakeDriver::new(Behavior::Complete));
    let (ledger, cleanup, runtime) = runtime_parts(driver.clone());
    let source = seed_controller_reconstructed_run(&ledger, "reserved-source").await;
    ledger
        .append(
            &source,
            vec![
                RunEvent::RunStarted,
                RunEvent::Suspended {
                    suspension: RunSuspension::Command {
                        node: NodeName::new("worker").assert_value(),
                        created_at: UnixTimestampMillis::new(1).assert_value(),
                    },
                },
            ],
        )
        .await
        .assert_value();
    let gate = watch::channel(false).0;
    let allocator = Arc::new(SuspensionAllocator {
        runtime,
        policy: Default::default(),
        store: Arc::new(SuspensionStore::default()),
        successor: StdMutex::new(None),
        preparation_gate: Some(gate.clone()),
    });
    let controller = NativeV2PlatformController::new_with_delivery_policy(
        ledger.clone(),
        allocator,
        DeliveryPolicy::Optional,
    )
    .await
    .assert_value();
    let first = controller
        .resume(resume_params(&source))
        .await
        .assert_value();
    let before = controller
        .status(RunStatusParams {
            run_id: source.clone(),
        })
        .await
        .assert_value();
    assert_eq!(
        before.workspace_recovery.successor_run_id.as_ref(),
        Some(&first.run_id)
    );
    assert!(!before.workspace_recovery.recoverable);
    let mut second = resume_params(&source);
    second.successor_run_id = RunId::new("competing-successor");
    let error = controller.resume(second).await.unwrap_err();
    assert!(error.to_string().contains(first.run_id.as_str()), "{error}");
    assert_eq!(ledger.list().await.assert_value().len(), 2);
    let forced = controller
        .force(RunForceParams {
            run_id: source.clone(),
        })
        .await
        .assert_value();
    assert_eq!(forced.status, before.status);
    let after = controller
        .status(RunStatusParams { run_id: source })
        .await
        .assert_value();
    assert_eq!(
        after, before,
        "force-stop must not change an accepted continuation's source"
    );
    assert!(
        cleanup.exits().is_empty(),
        "source storage must remain untouched"
    );
    tokio::task::yield_now().await;
    assert_eq!(driver.starts.load(Ordering::SeqCst), 0);
    assert!(
        ledger
            .get(&first.run_id)
            .await
            .assert_value()
            .assert_value()
            .snapshot
            .terminal
            .is_none()
    );
    gate.send_replace(true);
    assert!(matches!(
        terminal(&controller, &first.run_id).await,
        TerminalResult::Succeeded { .. }
    ));
}

#[tokio::test]
async fn suspension_restart_due_guard_and_continuation_preserve_state_and_vectors() {
    let driver = Arc::new(FakeDriver::new(Behavior::Complete));
    let (ledger, cleanup, runtime) = runtime_parts(driver.clone());
    let clock = Arc::new(AtomicU64::new(1_762_905_600_000));
    let read_clock = clock.clone();
    let allocator = Arc::new(SuspensionAllocator {
        runtime,
        store: Arc::new(SuspensionStore::default()),
        successor: StdMutex::new(None),
        preparation_gate: None,
        policy: crate::native_v2_wait::WaitPolicy {
            in_place_threshold: Some(Duration::from_secs(3600)),
            wall_clock: Arc::new(move || {
                UnixTimestampMillis::new(read_clock.load(Ordering::SeqCst)).assert_value()
            }),
        },
    });
    let controller = NativeV2PlatformController::new_with_delivery_policy(
        ledger.clone(),
        allocator.clone(),
        DeliveryPolicy::Optional,
    )
    .await
    .assert_value();
    let receipt = submit_test_request(&controller, wait_request())
        .await
        .unwrap_or_else(|error| panic!("wait admission: {error}"));
    let status = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let status = controller
                .status(RunStatusParams {
                    run_id: receipt.run_id.clone(),
                })
                .await
                .assert_value();
            if matches!(status.status, RunStatus::Suspended { .. }) {
                break status;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .assert_value();
    assert!(matches!(
        status.status,
        RunStatus::Suspended {
            suspension: RunSuspension::Wait { .. },
            ..
        }
    ));
    assert!(!status.workspace_recovery.recoverable);
    assert!(!status.workspace_recovery.connection_requirements.is_empty());
    assert_eq!(driver.starts.load(Ordering::SeqCst), 0);
    assert_eq!(cleanup.exits(), vec![RunRuntimeExit::Suspended]);
    controller.wait_runtime_settled(&receipt.run_id).await;
    drop(controller);
    let controller = NativeV2PlatformController::new_with_delivery_policy(
        ledger.clone(),
        allocator.clone(),
        DeliveryPolicy::Optional,
    )
    .await
    .assert_value();
    let params = resume_params(&receipt.run_id);
    clock.fetch_add(7_199_999, Ordering::SeqCst);
    let mut invalid = params.clone();
    invalid.connections.clear(); // The due guard must precede fresh credential resolution.
    let error = controller.resume(invalid).await.unwrap_err();
    assert!(error.to_string().contains("due at"), "{error}");
    assert_eq!(ledger.list().await.assert_value().len(), 1);
    clock.fetch_add(1, Ordering::SeqCst);
    assert!(
        controller
            .status(RunStatusParams {
                run_id: receipt.run_id.clone()
            })
            .await
            .assert_value()
            .workspace_recovery
            .recoverable
    );
    let resumed = controller.resume(params.clone()).await.assert_value();
    assert!(matches!(
        terminal(&controller, &resumed.run_id).await,
        TerminalResult::Succeeded { .. }
    ));
    assert_eq!(
        driver.starts.load(Ordering::SeqCst),
        1,
        "the settled wait never reaches a provider"
    );
    let page = controller
        .vectors(RunVectorsParams {
            run_id: resumed.run_id,
            after: None,
            limit: None,
        })
        .await
        .assert_value();
    assert_eq!(
        page.vectors[..2],
        *allocator.store.published.lock().assert_value()
    );
    assert!(matches!(
        page.vectors[2].content,
        RunVectorContent::Continuation { .. }
    ));
    let error = controller.resume(params).await.unwrap_err();
    assert!(error.to_string().contains("wait-successor"));
    assert!(matches!(
        controller
            .force(RunForceParams {
                run_id: receipt.run_id
            })
            .await
            .assert_value()
            .status,
        RunStatus::Suspended { .. }
    ));
}

#[tokio::test]
async fn default_host_waits_in_place_until_command_suspension_and_force_is_final() {
    let driver = Arc::new(FakeDriver::new(Behavior::Complete));
    let (ledger, cleanup, runtime) = runtime_parts(driver.clone());
    let allocator = Arc::new(SuspensionAllocator {
        runtime,
        policy: crate::native_v2_wait::WaitPolicy {
            wall_clock: Arc::new(|| UnixTimestampMillis::new(1_762_905_600_000).assert_value()),
            ..Default::default()
        },
        store: Arc::new(SuspensionStore::default()),
        successor: StdMutex::new(None),
        preparation_gate: None,
    });
    let controller = NativeV2PlatformController::new_with_delivery_policy(
        ledger,
        allocator,
        DeliveryPolicy::Optional,
    )
    .await
    .assert_value();
    let mut request = wait_request();
    request.submission.initial_input = json!({"durationMs":86_400_000});
    let run = submit_test_request(&controller, request)
        .await
        .assert_value()
        .run_id;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let status = controller.status(RunStatusParams {run_id:run.clone()}).await.assert_value();
            if matches!(status.status, RunStatus::Running {ref active_executions, ..} if active_executions.iter().any(|execution| execution.node.as_str() == "wait")) {
                break;
            }
            tokio::task::yield_now().await;
        }
    }).await.assert_value();
    assert_eq!(driver.starts.load(Ordering::SeqCst), 0);
    controller
        .suspend(RunSuspendParams {
            run_id: run.clone(),
        })
        .await
        .assert_value();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let status = controller
                .status(RunStatusParams {
                    run_id: run.clone(),
                })
                .await
                .assert_value();
            if matches!(
                status.status,
                RunStatus::Suspended {
                    suspension: RunSuspension::Wait { .. },
                    ..
                }
            ) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .assert_value();
    controller.wait_runtime_settled(&run).await;
    assert_eq!(cleanup.exits(), vec![RunRuntimeExit::Suspended]);
    let mut params = resume_params(&run);
    params.connections.clear();
    assert!(
        controller
            .resume(params.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("due at")
    );
    let forced = controller
        .force(RunForceParams { run_id: run })
        .await
        .assert_value();
    assert!(
        matches!(forced.status, RunStatus::Finished {terminal_result:TerminalResult::Failed {ref reason, ..}, ..} if reason.as_str() == "force_stopped")
    );
    assert!(controller.resume(params).await.is_err());
}

#[tokio::test]
async fn hosts_without_opt_in_refuse_wait_plans_and_suspend_without_effects() {
    let harness = harness(Behavior::Hang).await;
    assert!(matches!(
        submit_test_request(&harness.controller, wait_request()).await,
        Err(NativeV2PlatformError::Admission(_))
    ));
    assert!(harness.ledger.list().await.assert_value().is_empty());
    let receipt = submit_test_request(&harness.controller, request(Value::Null))
        .await
        .assert_value();
    harness.driver.wait_started().await;
    assert!(matches!(
        harness
            .controller
            .suspend(RunSuspendParams {
                run_id: receipt.run_id.clone()
            })
            .await,
        Err(NativeV2PlatformError::InvalidRequest(_))
    ));
    assert!(
        !harness
            .ledger
            .get(&receipt.run_id)
            .await
            .assert_value()
            .assert_value()
            .snapshot
            .suspend_requested
    );
    harness
        .controller
        .force(RunForceParams {
            run_id: receipt.run_id,
        })
        .await
        .assert_value();
}
