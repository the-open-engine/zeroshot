use super::*;
use crate::native_v2_contract::{DeliveryCheckpoint, ExecutionId, ExecutionRef, NodeInstanceId};
use crate::native_v2_runner::test_support::{delivery_checkpoint, node_started};
use crate::native_v2_supervisor::checkpoints::{
    CheckpointError, DeliveryCheckpointStore, RunCheckpointStore,
};
use crate::v2_run_ledger::RunEvent;

struct RecoveryAllocator {
    claims: FakeClaimAuthority,
    store: Arc<DeliveryCheckpointStore>,
    ledger: Arc<FakeRunLedger>,
    successor: RunId,
    retained: AtomicUsize,
    allocations: AtomicUsize,
}

#[async_trait]
impl CapsuleAllocator for RecoveryAllocator {
    async fn claim_controller(
        &self,
        run_id: &RunId,
    ) -> Result<Arc<dyn ExclusiveControllerClaim>, ControllerClaimUnavailable> {
        if run_id == &self.successor {
            assert_eq!(self.retained.load(Ordering::SeqCst), 1);
        }
        self.claims.acquire(run_id)
    }

    async fn allocate(
        &self,
        _request: CapsuleAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        panic!("recovery must use retained allocation");
    }

    async fn allocate_from_retained(
        &self,
        request: RetainedAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, RetainedAllocationUnavailable> {
        assert_eq!(
            self.store
                .delivery_state()
                .await
                .assert_value()
                .assert_value()
                .generation
                .get(),
            2
        );
        let successor = self
            .ledger
            .get(request.run_id)
            .await
            .assert_value()
            .assert_value();
        assert!(successor.snapshot.execution_seed.is_empty());
        assert!(successor.snapshot.executions.is_empty());
        self.allocations.fetch_add(1, Ordering::SeqCst);
        // Even an interrupted environment preparation must leave receipt retention settled.
        Err(CapsuleAllocationUnavailable::Runtime.into())
    }

    async fn destroy_or_confirm_absent(
        &self,
        _run_id: &RunId,
        _exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        Ok(CapsuleDestroyed::confirmed())
    }

    async fn workspace_recovery(
        &self,
        _run_id: &RunId,
    ) -> openengine_cluster_protocol::WorkspaceRecovery {
        openengine_cluster_protocol::WorkspaceRecovery {
            recoverable: true,
            ..Default::default()
        }
    }

    async fn retain_delivery_state(
        &self,
        _run_id: &RunId,
        state: &DeliveryCheckpoint,
    ) -> Result<(), CheckpointError> {
        assert!(
            self.ledger
                .get(&self.successor)
                .await
                .assert_value()
                .is_none(),
            "retention must precede successor creation and ownership transfer"
        );
        self.store.retain_delivery_state(state).await?;
        self.retained.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn dead_controller_reconciles_delivery_before_successor_claim_and_generation_conflicts_block_resume()
 {
    for conflict in [false, true] {
        let root = tempfile::tempdir().assert_value();
        let ledger = Arc::new(FakeRunLedger::new());
        let submitted = request_with_key(Value::Null, "dead-receipt");
        persist_request(ledger.as_ref(), &submitted).await;
        let reference = ExecutionRef {
            run_id: submitted.run_id.clone(),
            node: NodeName::new("deliver").assert_value(),
            node_instance: NodeInstanceId::new(1).assert_value(),
            execution: ExecutionId::new(1).assert_value(),
        };
        let latest = delivery_checkpoint(json!({"owned":"resolved conversation"}), 2);
        ledger
            .append(
                &submitted.run_id,
                vec![
                    RunEvent::RunStarted,
                    node_started(&reference, Value::Null),
                    RunEvent::DeliveryState {
                        execution: reference.execution,
                        state: latest.clone(),
                    },
                ],
            )
            .await
            .assert_value();
        let store = Arc::new(DeliveryCheckpointStore::new(root.path().join("lineage")));
        store
            .retain_delivery_state(&delivery_checkpoint(
                json!({}),
                if conflict { 2 } else { 1 },
            ))
            .await
            .assert_value();
        let successor = RunId::new("dead-receipt-successor");
        let allocator = Arc::new(RecoveryAllocator {
            claims: FakeClaimAuthority::default(),
            store: store.clone(),
            ledger: ledger.clone(),
            successor: successor.clone(),
            retained: AtomicUsize::new(0),
            allocations: AtomicUsize::new(0),
        });
        // Startup terminalizes the lost predecessor. Its receipt is still only in the ledger.
        let controller = NativeV2PlatformController::new(ledger.clone(), allocator.clone())
            .await
            .assert_value();
        let result = controller
            .resume(RunResumeParams {
                run_id: submitted.run_id,
                successor_run_id: successor.clone(),
                from: None,
                connections: BTreeMap::from([(
                    ConnectionKey::new("test").assert_value(),
                    test_connection_values("fresh-node-token"),
                )]),
                connection_resolver: None,
                github_token: None,
                git_credentials: None,
                commit_identity: None,
                commit_co_author: None,
            })
            .await;
        if conflict {
            assert!(matches!(result, Err(NativeV2PlatformError::Checkpoint(_))));
            assert!(ledger.get(&successor).await.assert_value().is_none());
            assert_eq!(allocator.allocations.load(Ordering::SeqCst), 0);
        } else {
            result.assert_value();
            assert!(matches!(
                terminal(&controller, &successor).await,
                TerminalResult::Failed { .. }
            ));
            assert_eq!(allocator.allocations.load(Ordering::SeqCst), 1);
            assert_eq!(store.delivery_state().await.assert_value(), Some(latest));
        }
    }
}
