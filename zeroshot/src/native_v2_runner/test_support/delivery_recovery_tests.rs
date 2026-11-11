use super::*;
use crate::native_v2_admission::NativeV2Admission;
use crate::native_v2_candidate::test_support::{git_delivery_node, success_node};
use crate::native_v2_supervisor::{NativeV2Supervisor, RunEnvironment};
use crate::native_v2_supervisor::checkpoints::{DeliveryCheckpointStore, RunCheckpointStore};
use crate::v2_run_ledger::{CreateRun, RunEvent, RunLedger};
use crate::v2_run_ledger::fake::FakeRunLedger;
use openengine_cluster_protocol::{IdempotencyKey, Sha256Digest, TerminalResult};

struct NoDispatch;

#[async_trait]
impl NodeRunner for NoDispatch {
    async fn start(&self, _request: NodeRunRequest) -> Result<NodeHandle, NodeRunnerError> {
        panic!("interrupted delivery must not be dispatched again");
    }

    async fn close_run(&self, _run_id: &RunId) {}
}

/// A receipt is acknowledged while delivery is still active, before another graph boundary.
pub(crate) async fn interrupted_delivery_state(
    repository: &std::path::Path,
    state: Value,
) -> Value {
    for failed in [false, true] {
        let store = Arc::new(DeliveryCheckpointStore::new(repository.to_owned()));
        let old = delivery_checkpoint(json!({"earlier":"generation"}), 1);
        store.retain_delivery_state(&old).await.assert_value();
        let expected = delivery_checkpoint(state.clone(), 2);
        let mut runtime = crate::native_v2_candidate::test_support::codex_runtime(
            run_size("host"),
            BTreeMap::from([(
                NodeName::new("deliver").assert_value(),
                NodeRuntimeBinding::GitDelivery {
                    connections: DeclaredConnections::empty(),
                    pull_request_feedback: Default::default(),
                },
            )]),
        );
        runtime.workspace_checkpoints = false;
        let admitted = NativeV2Admission
            .admit(crate::native_v2_contract::RunSubmission {
                environment: None,
                title: RunTitle::new("Interrupted delivery").assert_value(),
                graph: full_graph(vec![git_delivery_node(), success_node()]),
                initial_input: Value::Null,
                runtime,
                source: Some(source()),
                submission_key: IdempotencyKey::new("interrupted-delivery").assert_value(),
            })
            .await
            .assert_value();
        let environment =
            RunEnvironment::exact(&admitted.runtime, None, BTreeMap::new()).assert_value();
        let run_id = RunId::new("interrupted-delivery");
        let ledger = Arc::new(FakeRunLedger::new());
        ledger
            .create_or_get(CreateRun {
                run_id: run_id.clone(),
                submission_key: IdempotencyKey::new("interrupted-delivery").assert_value(),
                submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
                admitted,
            })
            .await
            .assert_value();
        let reference = ExecutionRef {
            run_id: run_id.clone(),
            node: NodeName::new("deliver").assert_value(),
            node_instance: NodeInstanceId::new(1).assert_value(),
            execution: ExecutionId::new(1).assert_value(),
        };
        ledger
            .append(
                &run_id,
                vec![
                    RunEvent::RunStarted,
                    node_started(&reference, Value::Null),
                    RunEvent::DeliveryState {
                        execution: reference.execution,
                        state: expected.clone(),
                    },
                ],
            )
            .await
            .assert_value();
        assert!(
            ledger
                .get(&run_id)
                .await
                .assert_value()
                .assert_value()
                .snapshot
                .execution_seed
                .is_empty()
        );
        let supervisor = NativeV2Supervisor::new(
            run_id.clone(),
            ledger.clone(),
            Arc::new(NoDispatch),
            Arc::new(environment),
        )
        .with_checkpoints(Some(store.clone()));
        if failed {
            supervisor
                .fail_runtime("interrupted fixture", || {})
                .await
                .assert_value();
        } else {
            supervisor.drive().await.assert_value();
        }
        assert!(
            matches!(ledger.get(&run_id).await.assert_value().assert_value()
            .snapshot.terminal, Some(TerminalResult::Failed { reason, .. })
                if reason.as_str() == if failed { "runtime_failed" } else { "runtime_lost" })
        );
        assert_eq!(store.delivery_state().await.assert_value(), Some(expected));
        if !failed {
            store.discard().await.assert_value();
        }
    }
    // Reconstruct only from retained lineage facts; no graph seed or worker responses are imported.
    let recovered = DeliveryCheckpointStore::new(repository.to_owned());
    recovered.finish(&[]).await.assert_value();
    let retained = recovered
        .delivery_state()
        .await
        .assert_value()
        .assert_value();
    assert_eq!(retained, delivery_checkpoint(state, 2));
    assert_eq!(std::fs::read_dir(repository).assert_value().count(), 1);
    retained.state
}
